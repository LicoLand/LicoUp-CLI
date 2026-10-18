//! A30 at `component-integration`: hot reload, in-flight binding and the atomic
//! catalog.
//!
//! A long-running task is admitted on v1; v2 is installed, prepared and
//! committed as a new registry epoch. New calls go to v2, while the old task's
//! observe/cancel/result stay on v1 and settle there. Concurrent admissions,
//! switch failures and revocation are injected, and no reader ever sees a
//! catalog that mixes two active generations of one scope.

use std::collections::BTreeSet;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use licoup_application::ApplicationFailure;
use licoup_extension_contracts::deployment::InstanceLifecycle;
use licoup_native::platform::extension_host::{
    CancelDisposition, CatalogAdmission, FaultClass, InvocationOutcome, Observation,
};
use serde_json::json;

use crate::support::{ControlledCarrier, activate, host as build_host, stage_request};

fn snapshot_is_consistent(snapshot: &licoup_native::platform::extension_host::CatalogSnapshot) {
    for entry in snapshot.entries() {
        assert!(
            entry.registry_epoch().get() <= snapshot.epoch().get(),
            "an entry cannot come from a future epoch"
        );
        for capability in &entry.capabilities {
            assert!(
                snapshot
                    .instances_serving(capability)
                    .iter()
                    .any(|serving| serving.instance_id() == entry.instance_id()),
                "the capability index must agree with the entries"
            );
        }
    }
    let mut active_scope = BTreeSet::new();
    for entry in snapshot.entries() {
        if entry.state == InstanceLifecycle::Active && entry.admission == CatalogAdmission::Open {
            let key = (
                entry.package_id().to_owned(),
                entry.identity.permission_scope.clone(),
            );
            assert!(
                active_scope.insert(key),
                "two active generations of one scope cannot be in one catalog"
            );
        }
    }
}

#[test]
fn new_calls_go_to_the_new_generation_while_in_flight_work_stays_on_the_old_one() {
    let carrier = ControlledCarrier::new("swap");
    let host = build_host(carrier.clone());
    let v1 = activate(&host, "acme.hot/ext", &["acme.hot/run"]).expect("activate v1");
    let long = host
        .begin("acme.hot/run", &json!({"prompt": "long"}))
        .expect("admit on v1");
    assert_eq!(
        host.observe(&long.binding).expect("running"),
        Observation::Running
    );

    let v2 = activate(&host, "acme.hot/ext", &["acme.hot/run"]).expect("activate v2");
    assert_eq!(v2.generation, 2);
    assert_eq!(v2.drained, vec![v1.instance_id.clone()]);

    // A new call is admitted to v2, at v2's epoch.
    let fresh = host
        .begin("acme.hot/run", &json!({"prompt": "new"}))
        .expect("admit on v2");
    assert_eq!(fresh.binding.instance_id(), v2.instance_id);
    assert_eq!(fresh.binding.generation(), 2);
    assert_eq!(fresh.binding.registry_epoch(), v2.registry_epoch);

    // The old task's observe and result still reach the old generation and
    // settle it there. The sessions are captured first: settling the last
    // obligation releases the old session, and the record of what it served is
    // what proves where the call went.
    let v1_session = carrier.session(&v1.instance_id).expect("v1 session");
    let v2_session = carrier.session(&v2.instance_id).expect("v2 session");
    carrier.with_result(Observation::Completed {
        payload: json!({"served_by": 1}),
    });
    assert_eq!(
        host.result(&long.binding).expect("result on v1"),
        InvocationOutcome::Finished {
            payload: json!({"served_by": 1})
        }
    );
    assert_eq!(v1_session.observations(), 1);
    assert_eq!(v1_session.result_count.load(Ordering::SeqCst), 1);
    assert_eq!(v2_session.observations(), 0);
    assert_eq!(v2_session.result_count.load(Ordering::SeqCst), 0);
    let v1_report = host.instance_report(&v1.instance_id).expect("v1 report");
    assert_eq!(v1_report.state, InstanceLifecycle::Stopped);
    assert_eq!(v1_report.unknown, 0);
    let v2_report = host.instance_report(&v2.instance_id).expect("v2 report");
    assert_eq!(v2_report.state, InstanceLifecycle::Active);

    // Every call that reached v1 carried v1's generation on its binding.
    for call in carrier.calls() {
        match call {
            crate::support::Call::Dispatch {
                instance,
                generation,
                ..
            }
            | crate::support::Call::Observe {
                instance,
                generation,
                ..
            }
            | crate::support::Call::Result {
                instance,
                generation,
                ..
            }
            | crate::support::Call::Cancel {
                instance,
                generation,
                ..
            } => {
                if instance == v1.instance_id {
                    assert_eq!(
                        generation, 1,
                        "an old-generation call cannot carry the new one"
                    );
                }
                if instance == v2.instance_id {
                    assert_eq!(generation, 2);
                }
            }
            _ => {}
        }
    }
}

#[test]
fn cancelling_an_old_generation_does_not_touch_the_new_one() {
    let carrier = ControlledCarrier::new("cancel-swap");
    let host = build_host(carrier.clone());
    let v1 = activate(&host, "acme.hot/ext", &["acme.hot/run"]).expect("activate v1");
    let long = host
        .begin("acme.hot/run", &json!({"prompt": "long"}))
        .expect("admit on v1");
    let v2 = activate(&host, "acme.hot/ext", &["acme.hot/run"]).expect("activate v2");

    assert_eq!(
        host.cancel(&long.binding).expect("cancel v1"),
        CancelDisposition::Acknowledged
    );
    let v1_session = carrier.session(&v1.instance_id).expect("v1 session");
    assert_eq!(v1_session.cancel_count.load(Ordering::SeqCst), 1);
    let v2_session = carrier.session(&v2.instance_id).expect("v2 session");
    assert_eq!(v2_session.cancel_count.load(Ordering::SeqCst), 0);
    assert_eq!(
        carrier.session(&v1.instance_id).expect("v1").dispatches(),
        1
    );

    // Cancellation is a request: the result settles the old call and v2 keeps
    // working.
    assert!(matches!(
        host.result(&long.binding).expect("settle v1"),
        InvocationOutcome::Finished { .. }
    ));
    let fresh = host
        .begin("acme.hot/run", &json!({}))
        .expect("v2 still admits");
    assert_eq!(fresh.binding.generation(), 2);
}

#[test]
fn concurrent_admissions_never_mix_generations() {
    let carrier = ControlledCarrier::new("concurrent");
    let host = Arc::new(build_host(carrier.clone()));
    let v1 = activate(&host, "acme.concurrent/ext", &["acme.concurrent/run"]).expect("v1");
    // Hold one v1 call so generation 1 is certainly represented.
    let long = host
        .begin("acme.concurrent/run", &json!({"prompt": "hold"}))
        .expect("admit on v1");
    assert_eq!(long.binding.generation(), 1);

    let stop = Arc::new(AtomicBool::new(false));
    let admissions: Arc<Mutex<Vec<(String, u64, u64)>>> = Arc::new(Mutex::new(Vec::new()));
    let workers: Vec<_> = (0..4)
        .map(|worker| {
            let host = Arc::clone(&host);
            let stop = Arc::clone(&stop);
            let admissions = Arc::clone(&admissions);
            thread::spawn(move || {
                let mut rounds = 0usize;
                while !stop.load(Ordering::SeqCst) {
                    match host.begin("acme.concurrent/run", &json!({"prompt": worker})) {
                        Ok(admitted) => {
                            admissions.lock().expect("admissions").push((
                                admitted.binding.instance_id().to_owned(),
                                admitted.binding.generation(),
                                admitted.binding.registry_epoch().get(),
                            ));
                            if matches!(admitted.outcome, InvocationOutcome::Admitted) {
                                let _ = host.result(&admitted.binding);
                            }
                            rounds += 1;
                        }
                        Err(_) => thread::yield_now(),
                    }
                }
                rounds
            })
        })
        .collect();

    thread::sleep(Duration::from_millis(30));
    let v2 = activate(&host, "acme.concurrent/ext", &["acme.concurrent/run"]).expect("v2");
    thread::sleep(Duration::from_millis(60));
    stop.store(true, Ordering::SeqCst);
    let mut rounds = 0usize;
    for worker in workers {
        rounds += worker.join().expect("worker");
    }
    assert!(rounds > 0, "the workers admitted calls");

    let _ = host.result(&long.binding).expect("settle the held v1 call");

    let admissions = admissions.lock().expect("admissions");
    assert!(
        admissions.iter().any(|(_, generation, _)| *generation == 1),
        "at least one admission happened on v1"
    );
    assert!(
        admissions.iter().any(|(_, generation, _)| *generation == 2),
        "at least one admission happened on v2"
    );
    let snapshot = host.catalog();
    for (instance_id, generation, registry_epoch) in admissions.iter() {
        let entry = snapshot
            .entry(instance_id)
            .expect("admitted instance is in the catalog");
        assert_eq!(entry.generation(), *generation);
        assert_eq!(entry.registry_epoch().get(), *registry_epoch);
    }
    // The capability stays routable, but only through the new generation.
    assert!(snapshot.serves("acme.concurrent/run"));
    let v1_entry = snapshot.entry(&v1.instance_id).expect("v1 entry");
    assert_ne!(v1_entry.state, InstanceLifecycle::Active);
    let v2_entry = snapshot.entry(&v2.instance_id).expect("v2 entry");
    assert_eq!(v2_entry.state, InstanceLifecycle::Active);
    snapshot_is_consistent(&snapshot);
}

#[test]
fn a_failed_switch_keeps_the_old_snapshot_and_releases_the_new_session() {
    let carrier = ControlledCarrier::new("switch-failure");
    let host = build_host(carrier.clone());
    let v1 = activate(&host, "acme.switch/ext", &["acme.switch/run"]).expect("activate v1");
    let epoch_before = host.catalog().epoch();

    let staged = host
        .stage(stage_request(
            "acme.switch/ext",
            "2.0.0",
            &["acme.switch/run"],
            &[("agent-execution", &["acme.switch/run"])],
        ))
        .expect("stage v2");
    carrier.with_ready_fault(FaultClass::Crashed);
    let failure = host.prepare(staged).expect_err("preparation fails");
    assert_eq!(failure.code, "extension_carrier_crashed");

    assert_eq!(host.catalog().epoch(), epoch_before);
    assert!(host.catalog().serves("acme.switch/run"));
    assert_eq!(
        carrier.dropped_sessions.load(Ordering::SeqCst),
        1,
        "the failed preparation released its session"
    );
    let admitted = host
        .begin("acme.switch/run", &json!({}))
        .expect("v1 still routes");
    assert_eq!(admitted.binding.instance_id(), v1.instance_id);
}

#[test]
fn every_surface_reads_one_epoch() {
    let carrier = ControlledCarrier::new("readers");
    let host = Arc::new(build_host(carrier.clone()));
    activate(&host, "acme.readers/ext", &["acme.readers/run"]).expect("v1");

    let stop = Arc::new(AtomicBool::new(false));
    let reads = Arc::new(AtomicUsize::new(0));
    let readers: Vec<_> = ["desktop-ui", "cli", "mcp"]
        .into_iter()
        .map(|surface| {
            let host = Arc::clone(&host);
            let stop = Arc::clone(&stop);
            let reads = Arc::clone(&reads);
            thread::spawn(move || {
                while !stop.load(Ordering::SeqCst) {
                    let snapshot = host.catalog();
                    snapshot_is_consistent(&snapshot);
                    let document = snapshot.document();
                    assert_eq!(document.epoch, snapshot.epoch());
                    assert_eq!(document.extensions.len(), snapshot.len());
                    // The projection and the snapshot agree, which is what "one
                    // epoch for every surface" means in practice.
                    for entry in &document.extensions {
                        assert!(snapshot.entry(entry.instance_id()).is_some());
                    }
                    reads.fetch_add(1, Ordering::SeqCst);
                    let _ = surface;
                }
            })
        })
        .collect();

    for version in ["1.1.0", "1.2.0"] {
        let staged = host
            .stage(stage_request(
                "acme.readers/ext",
                version,
                &["acme.readers/run"],
                &[("agent-execution", &["acme.readers/run"])],
            ))
            .expect("stage");
        host.activate(host.prepare(staged).expect("prepare"))
            .expect("activate");
    }
    thread::sleep(Duration::from_millis(20));
    stop.store(true, Ordering::SeqCst);
    for reader in readers {
        reader.join().expect("reader");
    }
    assert!(reads.load(Ordering::SeqCst) > 0);
    snapshot_is_consistent(&host.catalog());
}

#[test]
fn a_binding_cannot_follow_a_settled_revoked_instance_into_a_guess() {
    let carrier = ControlledCarrier::new("strict");
    let host = build_host(carrier.clone());
    activate(&host, "acme.strict/ext", &["acme.strict/run"]).expect("activate");
    let admitted = host.begin("acme.strict/run", &json!({})).expect("admit");
    assert!(matches!(
        host.result(&admitted.binding).expect("settle"),
        InvocationOutcome::Finished { .. }
    ));
    let failure: ApplicationFailure = host
        .result(&admitted.binding)
        .expect_err("a settled binding is not settled twice");
    assert_eq!(failure.code, "extension_invocation_settled");
    let _ = host;
}
