//! Cross-host identity: a handle one host issued is never another host's.
//!
//! Two hosts in one process produce the *same visible facts* for their first
//! instance and first invocation: `instance-1`/`invocation-1`, generation 1,
//! epoch 1, the same package and capability. A caller can therefore hold host
//! A's binding and pass it to host B, and every field a comparison could look
//! at is equal. The requirement is that host B refuses it by identity — before
//! any invocation id is looked up, and without calling its carrier — and that
//! the same holds for hook tickets and prepared extensions.
//!
//! The facts the carriers count here are the proof: a refusal that still
//! reached the carrier would be observable as a receive call.

use licoup_native::platform::extension_host::Observation;
use serde_json::json;

use crate::support::{Call, ControlledCarrier, activate, host as build_host, stage_request};

fn receipts_and_bindings(
    label: &'static str,
) -> (
    std::sync::Arc<ControlledCarrier>,
    licoup_native::platform::extension_host::ExtensionHost,
    licoup_native::platform::extension_host::AdmittedInvocation,
) {
    let carrier = ControlledCarrier::new(label);
    let host = build_host(carrier.clone());
    activate(&host, "acme.cross/ext", &["acme.cross/run"]).expect("activate");
    let admitted = host
        .begin("acme.cross/run", &json!({"prompt": label}))
        .expect("admit");
    (carrier, host, admitted)
}

#[test]
fn a_binding_from_another_host_is_never_accepted() {
    let (_carrier_a, host_a, admitted_a) = receipts_and_bindings("host-a");
    let (carrier_b, host_b, admitted_b) = receipts_and_bindings("host-b");

    // The two hosts hand out indistinguishable visible facts.
    assert_eq!(
        admitted_a.binding.invocation_id(),
        admitted_b.binding.invocation_id()
    );
    assert_eq!(
        admitted_a.binding.instance_id(),
        admitted_b.binding.instance_id()
    );
    assert_eq!(
        admitted_a.binding.generation(),
        admitted_b.binding.generation()
    );
    assert_eq!(
        admitted_a.binding.registry_epoch(),
        admitted_b.binding.registry_epoch()
    );
    assert_eq!(
        admitted_a.binding.package_id(),
        admitted_b.binding.package_id()
    );
    assert_eq!(
        admitted_a.binding.capability(),
        admitted_b.binding.capability()
    );

    // Host B must refuse host A's handles before any lookup.
    for failure in [
        host_b
            .observe(&admitted_a.binding)
            .expect_err("foreign observe"),
        host_b
            .cancel(&admitted_a.binding)
            .expect_err("foreign cancel"),
        host_b
            .result(&admitted_a.binding)
            .expect_err("foreign result"),
    ] {
        assert_eq!(failure.code, "extension_invocation_foreign_host");
        assert_eq!(failure.stage, "extension/invocation");
    }
    // Host B's carrier saw exactly its own dispatch and nothing else: the
    // refusal did not reach it, and it did not answer with host B's session.
    assert_eq!(carrier_b.dispatches(), 1);
    assert_eq!(
        carrier_b
            .calls()
            .iter()
            .filter(|call| matches!(
                call,
                Call::Observe { .. } | Call::Cancel { .. } | Call::Result { .. }
            ))
            .count(),
        0
    );
    // Host A's own binding is still host A's.
    assert!(host_a.observe(&admitted_a.binding).is_ok());
    // Host B's own binding still answers normally.
    assert_eq!(
        host_b.observe(&admitted_b.binding).expect("own observe"),
        Observation::Running
    );
}

#[test]
fn a_hook_ticket_from_another_host_is_never_accepted() {
    let (_carrier_a, host_a, _) = receipts_and_bindings("hook-a");
    let (carrier_b, host_b, _) = receipts_and_bindings("hook-b");
    let instance_a = host_a
        .catalog()
        .entries()
        .next()
        .expect("a instance")
        .instance_id()
        .to_owned();
    let instance_b = host_b
        .catalog()
        .entries()
        .next()
        .expect("b instance")
        .instance_id()
        .to_owned();
    let ticket_a = host_a
        .register_hook(&instance_a, "acme.cross/run")
        .expect("a hook");
    let ticket_b = host_b
        .register_hook(&instance_b, "acme.cross/run")
        .expect("b hook");
    assert_eq!(ticket_a.ticket_id(), ticket_b.ticket_id());

    let before = carrier_b.dispatches();
    let failure = host_b
        .hook_request_effect(&ticket_a, "acme.cross/run", &json!({"prompt": "stolen"}))
        .expect_err("a foreign hook is refused");
    assert_eq!(failure.code, "extension_hook_foreign_host");
    // The refusal added no dispatch: the count is still only host B's own call.
    assert_eq!(carrier_b.dispatches(), before);
    assert_eq!(before, 1);
}

#[test]
fn a_prepared_extension_from_another_host_is_never_activated() {
    let carrier_a = ControlledCarrier::new("prepared-a");
    let host_a = build_host(carrier_a.clone());
    let carrier_b = ControlledCarrier::new("prepared-b");
    let host_b = build_host(carrier_b.clone());

    let prepared = host_a
        .prepare(
            host_a
                .stage(stage_request(
                    "acme.cross/ext",
                    "1.0.0",
                    &["acme.cross/run"],
                    &[("agent-execution", &["acme.cross/run"])],
                ))
                .expect("stage"),
        )
        .expect("prepare");
    let failure = host_b
        .activate(prepared)
        .expect_err("a prepared value belongs to the host that prepared it");
    assert_eq!(failure.code, "extension_prepared_foreign_host");
    assert_eq!(host_a.host_contract_range(), host_b.host_contract_range());
    assert!(host_b.catalog().is_empty());
    // The foreign preparation released its session instead of committing it.
    assert_eq!(
        carrier_a
            .dropped_sessions
            .load(std::sync::atomic::Ordering::SeqCst),
        1
    );
}
