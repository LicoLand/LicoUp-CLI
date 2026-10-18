//! A38 at `component-integration`: fault injection around stage, prepare,
//! activation, drain and restart, and the compatible state that survives.
//!
//! The install transaction itself (bytes, journals, GC) belongs to the package
//! store and its own acceptance; what is injected here is the runtime half: a
//! carrier that fails to start, handshake or become ready, a commit that loses
//! a race, a restart that finds an instance missing. In every case the host
//! activates only a complete instance, keeps the previous catalog when the new
//! one fails, records unsettled work as `unknown` instead of replaying it, and
//! leaves the stopped generation visible with its identity facts.

use std::fs;
use std::path::Path;
use std::sync::atomic::Ordering;

use licoup_extension_contracts::deployment::InstanceLifecycle;
use licoup_native::platform::extension_host::FaultClass;
use serde_json::json;

use crate::support::{ControlledCarrier, activate, host as build_host, stage_request};

fn request(
    package: &str,
    version: &str,
    capability: &str,
) -> licoup_native::platform::extension_host::StageRequest {
    stage_request(
        package,
        version,
        &[capability],
        &[("agent-execution", &[capability])],
    )
}

#[test]
fn a_failed_preparation_releases_its_session_and_leaves_the_catalog_untouched() {
    for (label, script) in [
        ("start", FaultClass::Crashed),
        ("initialize", FaultClass::Unresponsive),
        ("ready", FaultClass::Protocol),
    ] {
        let carrier = ControlledCarrier::new("prepare-failure");
        let host = build_host(carrier.clone());
        activate(&host, "acme.stable/ext", &["acme.stable/run"]).expect("v1");
        let epoch_before = host.catalog().epoch();
        let entries_before = host.catalog().len();

        match label {
            "start" => carrier.with_start_fault(script),
            "initialize" => carrier.with_initialize_fault(script),
            _ => carrier.with_ready_fault(script),
        }
        let staged = host
            .stage(request("acme.stable/ext", "2.0.0", "acme.stable/run"))
            .expect("stage v2");
        let failure = host.prepare(staged).expect_err("preparation fails");
        assert_eq!(failure.component.as_ref(), "extension_host", "{label}");

        // The failed preparation changed nothing that readers can see.
        assert_eq!(host.catalog().epoch(), epoch_before, "{label}");
        assert_eq!(host.catalog().len(), entries_before, "{label}");
        assert!(host.catalog().serves("acme.stable/run"), "{label}");
        // A session that was started before the failure was released.
        if label != "start" {
            assert_eq!(
                carrier.dropped_sessions.load(Ordering::SeqCst),
                1,
                "{label}"
            );
        }
        // And the old generation still admits.
        let admitted = host
            .begin("acme.stable/run", &json!({}))
            .expect("v1 still routes");
        assert_eq!(admitted.binding.generation(), 1);
    }
}

#[test]
fn a_commit_that_loses_the_race_is_refused_and_its_session_released() {
    let carrier = ControlledCarrier::new("race");
    let host = build_host(carrier.clone());
    let prepared_v1 = host
        .prepare(
            host.stage(request("acme.race/ext", "1.0.0", "acme.race/run"))
                .expect("stage v1"),
        )
        .expect("prepare v1");
    let prepared_v2 = host
        .prepare(
            host.stage(request("acme.race/ext", "2.0.0", "acme.race/run"))
                .expect("stage v2"),
        )
        .expect("prepare v2");
    let v1_instance = prepared_v1.instance_id().to_owned();
    let receipt = host
        .activate(prepared_v2)
        .expect("the newer generation wins");
    let failure = host
        .activate(prepared_v1)
        .expect_err("the older preparation loses the race");
    assert_eq!(failure.code, "extension_activation_superseded");
    assert_eq!(failure.field.as_deref(), Some("generation"));

    assert_eq!(carrier.dropped_sessions.load(Ordering::SeqCst), 1);
    assert!(host.instance_report(&v1_instance).is_none());
    let snapshot = host.catalog();
    assert!(snapshot.entry(&receipt.instance_id).is_some());
    assert_eq!(snapshot.len(), 1);
    assert_eq!(
        snapshot
            .route("acme.race/run")
            .map(|entry| entry.generation()),
        Some(2)
    );
}

#[test]
fn an_unrelated_commit_is_rebased_not_refused() {
    let carrier = ControlledCarrier::new("rebase");
    let host = build_host(carrier.clone());
    let one = host
        .prepare(
            host.stage(request("acme.one/ext", "1.0.0", "acme.one/run"))
                .expect("stage one"),
        )
        .expect("prepare one");
    let two = host
        .prepare(
            host.stage(request("acme.two/ext", "1.0.0", "acme.two/run"))
                .expect("stage two"),
        )
        .expect("prepare two");
    // Both preparations read epoch 0; the first commit moves the catalog.
    assert_eq!(one.expected_epoch().get(), 0);
    assert_eq!(two.expected_epoch().get(), 0);
    let first = host.activate(one).expect("first commit");
    let second = host
        .activate(two)
        .expect("an unrelated commit is re-based onto the new epoch");
    assert_eq!(first.registry_epoch.get(), 1);
    assert_eq!(second.registry_epoch.get(), 2);
    let snapshot = host.catalog();
    assert!(snapshot.serves("acme.one/run"));
    assert!(snapshot.serves("acme.two/run"));
}

#[test]
fn restart_reconciliation_stops_missing_instances_and_records_unknown() {
    let carrier = ControlledCarrier::new("restart");
    let host = build_host(carrier.clone());
    let receipt = activate(&host, "acme.restart/ext", &["acme.restart/run"]).expect("activate");
    let admitted = host
        .begin("acme.restart/run", &json!({"prompt": "in flight"}))
        .expect("admit");

    let missing = host.reconcile_after_restart(&[]);
    assert_eq!(missing, vec![receipt.instance_id.clone()]);
    let report = host.instance_report(&receipt.instance_id).expect("report");
    assert_eq!(report.state, InstanceLifecycle::Stopped);
    assert_eq!(report.unknown, 1);
    assert_eq!(report.in_flight, 0);
    assert!(!host.catalog().serves("acme.restart/run"));
    // The missing instance was never re-dispatched, and its late result does not
    // resurrect the unknown effect.
    assert_eq!(carrier.dispatches(), 1);
    let failure = host
        .result(&admitted.binding)
        .expect_err("a settled-unknown invocation is not answered again");
    assert_eq!(failure.code, "extension_invocation_settled");

    // An instance that is observed after the restart reconciles to nothing.
    let other = build_host(ControlledCarrier::new("restart-ok"));
    let observed = activate(&other, "acme.restart/ext", &["acme.restart/run"]).expect("activate");
    assert!(
        other
            .reconcile_after_restart(std::slice::from_ref(&observed.instance_id))
            .is_empty()
    );
    assert!(other.catalog().serves("acme.restart/run"));
}

#[test]
fn a_stopped_generation_keeps_its_record_and_a_new_one_can_take_over() {
    let carrier = ControlledCarrier::new("retained");
    let host = build_host(carrier.clone());
    let v1 = activate(&host, "acme.retained/ext", &["acme.retained/run"]).expect("v1");
    assert_eq!(host.revoke("acme.retained/ext"), 1);
    let entry = host
        .catalog()
        .entry(&v1.instance_id)
        .cloned()
        .expect("a stopped instance stays visible");
    assert_eq!(entry.state, InstanceLifecycle::Stopped);
    assert_eq!(entry.generation(), 1);
    assert_eq!(entry.registry_epoch(), v1.registry_epoch);

    // A later activation is a new generation of the same package, not a
    // resurrection of the old one.
    let v2 = activate(&host, "acme.retained/ext", &["acme.retained/run"]).expect("v2");
    assert_eq!(v2.generation, 2);
    assert!(host.catalog().serves("acme.retained/run"));
    let snapshot = host.catalog();
    assert_eq!(snapshot.len(), 2);
    assert_eq!(
        snapshot.entry(&v1.instance_id).expect("v1").state,
        InstanceLifecycle::Stopped
    );
    assert_eq!(
        snapshot.entry(&v2.instance_id).expect("v2").state,
        InstanceLifecycle::Active
    );
}

#[test]
fn finish_drain_refuses_while_work_is_unsettled_and_succeeds_after() {
    let carrier = ControlledCarrier::new("drain");
    let host = build_host(carrier.clone());
    let receipt = activate(&host, "acme.drain/ext", &["acme.drain/run"]).expect("activate");
    let admitted = host
        .begin("acme.drain/run", &json!({"prompt": "work"}))
        .expect("admit");
    assert_eq!(host.revoke("acme.drain/ext"), 1);

    // The instance is draining with an outstanding obligation: stopping now
    // would lose it.
    let failure = host
        .finish_drain(&receipt.instance_id)
        .expect_err("unsettled work blocks the stop");
    assert_eq!(failure.code, "extension_instance_work_unsettled");
    assert_eq!(
        host.instance_report(&receipt.instance_id)
            .expect("report")
            .state,
        InstanceLifecycle::Draining
    );

    let _ = host.result(&admitted.binding).expect("settle");
    host.finish_drain(&receipt.instance_id)
        .expect("nothing outstanding, the stop proceeds");
    assert_eq!(
        host.instance_report(&receipt.instance_id)
            .expect("report")
            .state,
        InstanceLifecycle::Stopped
    );
}

#[test]
fn a_session_that_cannot_be_shut_down_is_failed_not_reported_as_a_clean_stop() {
    let carrier = ControlledCarrier::new("stubborn");
    let host = build_host(carrier.clone());
    let receipt = activate(&host, "acme.stubborn/ext", &["acme.stubborn/run"]).expect("activate");
    // Nothing is outstanding, so the drain reaches its release immediately;
    // make the release fail and check the truth the host publishes.
    carrier.with_shutdown_fault(FaultClass::Crashed);
    let failure = host
        .finish_drain(&receipt.instance_id)
        .expect_err("a release that fails is visible");
    assert_eq!(failure.code, "extension_instance_shutdown_failed");
    assert_eq!(
        host.instance_report(&receipt.instance_id)
            .expect("report")
            .state,
        InstanceLifecycle::Failed
    );
    assert!(!host.catalog().serves("acme.stubborn/run"));
    let entry = host
        .catalog()
        .entry(&receipt.instance_id)
        .cloned()
        .expect("the failed instance stays visible");
    assert_eq!(entry.state, InstanceLifecycle::Failed);
    assert_eq!(entry.generation(), 1);
}

#[test]
fn the_host_sources_do_not_reach_workflow_or_gateway() {
    // A local negative check on the slice's own sources. It proves the module
    // does not name the workflow runtime or the gateway; production linkage and
    // import-graph conformance remain the development graph tooling's evidence.
    let directory = Path::new(env!("CARGO_MANIFEST_DIR")).join("src/platform/extension_host");
    let mut files = 0usize;
    for entry in fs::read_dir(&directory).expect("extension_host sources") {
        let path = entry.expect("entry").path();
        if path.extension().and_then(|extension| extension.to_str()) != Some("rs") {
            continue;
        }
        let text = fs::read_to_string(&path).expect("source");
        files += 1;
        for forbidden in [
            "licoup_workflow",
            "workflow_runtime::",
            "workflow_store::",
            "gateway_runtime::",
            "llm_gateway",
        ] {
            assert!(
                !text.contains(forbidden),
                "{} must not reach {forbidden}",
                path.display()
            );
        }
    }
    assert!(files >= 5, "the slice's sources were actually read");
}
