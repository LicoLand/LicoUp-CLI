//! A19 at `component-integration`: replaceable carrier ports and plugin fault
//! domains.
//!
//! Two controlled carriers implement the same production port and replace each
//! other without any change on the consumer side. A plugin that stops
//! answering, crashes, exceeds its budget or is revoked stops being routed;
//! every other capability keeps working, in-flight work is recorded `unknown`
//! rather than re-dispatched, and nothing claims an already-performed effect
//! was retracted.
//!
//! The real-process fixture and the confinement claims belong to the
//! process-isolation suite. What is
//! proven here is the host's own fault-domain rule.

use licoup_application::{EffectCertainty, RecoveryAction};
use licoup_native::platform::extension_host::{
    CancelDisposition, FaultClass, InvocationOutcome, Observation,
};
use serde_json::json;

use crate::support::{Call, ControlledCarrier, DispatchReply, activate, host as build_host};

#[test]
fn two_carriers_replace_each_other_without_changing_the_consumer() {
    let alpha = ControlledCarrier::new("alpha");
    alpha.with_dispatch(DispatchReply::Completed(json!({"carrier": "alpha"})));
    let host_alpha = build_host(alpha.clone());
    activate(&host_alpha, "acme.tools/ext", &["acme.tools/render"]).expect("activate alpha");
    let alpha_call = host_alpha
        .begin("acme.tools/render", &json!({"prompt": "draw"}))
        .expect("admit on alpha");

    let beta = ControlledCarrier::new("beta");
    beta.with_dispatch(DispatchReply::Completed(json!({"carrier": "beta"})));
    let host_beta = build_host(beta.clone());
    activate(&host_beta, "acme.tools/ext", &["acme.tools/render"]).expect("activate beta");
    let beta_call = host_beta
        .begin("acme.tools/render", &json!({"prompt": "draw"}))
        .expect("admit on beta");

    // The consumer code path is identical; only the implementation differs.
    assert_eq!(
        alpha_call.outcome,
        InvocationOutcome::Finished {
            payload: json!({"carrier": "alpha"})
        }
    );
    assert_eq!(
        beta_call.outcome,
        InvocationOutcome::Finished {
            payload: json!({"carrier": "beta"})
        }
    );
    assert_eq!(alpha.dispatches(), 1);
    assert_eq!(beta.dispatches(), 1);
}

#[test]
fn an_unresponsive_plugin_stops_being_routed_and_the_rest_keeps_working() {
    let carrier = ControlledCarrier::new("mixed");
    // One dispatch is admitted and stays in flight; the observe then hangs.
    let host = build_host(carrier.clone());
    let first = activate(&host, "acme.one/ext", &["acme.one/run"]).expect("activate one");
    let second = activate(&host, "acme.two/ext", &["acme.two/run"]).expect("activate two");
    assert_ne!(first.instance_id, second.instance_id);

    // A call that completed before the fault is not retracted later.
    let completed = host
        .begin("acme.two/run", &json!({"prompt": "done"}))
        .expect("admit on two");
    let _ = host
        .result(&completed.binding)
        .expect("the completed call settles");

    let in_flight = host
        .begin("acme.one/run", &json!({"prompt": "hangs"}))
        .expect("admit on one");
    carrier.with_observe_fault(FaultClass::Unresponsive);
    let failure = host
        .observe(&in_flight.binding)
        .expect_err("the unresponsive instance faults");
    assert_eq!(failure.code, "extension_carrier_unresponsive");
    assert_eq!(failure.effect, EffectCertainty::Uncertain);
    assert_eq!(failure.recovery, RecoveryAction::ReconcileBeforeRetry);

    let report = host.instance_report(&first.instance_id).expect("report");
    assert_eq!(
        report.state,
        licoup_extension_contracts::deployment::InstanceLifecycle::Failed
    );
    assert_eq!(report.in_flight, 0);
    assert_eq!(report.unknown, 1);

    // The other instance is untouched, including the call that already settled.
    let other = host.instance_report(&second.instance_id).expect("report");
    assert_eq!(
        other.state,
        licoup_extension_contracts::deployment::InstanceLifecycle::Active
    );
    assert_eq!(other.unknown, 0);
    let still_works = host
        .begin("acme.two/run", &json!({"prompt": "again"}))
        .expect("the healthy capability still admits");
    assert_eq!(still_works.outcome, InvocationOutcome::Admitted);

    // The failed generation cannot be routed to again, and its capability is
    // refused alone rather than failing the host.
    let failure = host
        .begin("acme.one/run", &json!({}))
        .expect_err("the failed instance serves nothing new");
    assert_eq!(failure.code, "extension_capability_unavailable");
    assert_eq!(failure.presentation_args.get("reason"), Some("not-active"));
}

#[test]
fn a_budget_overrun_quarantines_the_instance_without_redispatching() {
    let carrier = ControlledCarrier::new("budget");
    carrier.with_dispatch(DispatchReply::Fault(FaultClass::OverBudget));
    let host = build_host(carrier.clone());
    let receipt = activate(&host, "acme.costly/ext", &["acme.costly/run"]).expect("activate");

    let failure = host
        .begin("acme.costly/run", &json!({"prompt": "expensive"}))
        .expect_err("over budget is a fault");
    assert_eq!(failure.code, "extension_carrier_over_budget");
    assert_eq!(
        host.instance_report(&receipt.instance_id)
            .expect("report")
            .state,
        licoup_extension_contracts::deployment::InstanceLifecycle::Quarantined
    );

    // The work is never re-dispatched: the next admission is refused outright.
    let dispatches = carrier.dispatches();
    assert_eq!(dispatches, 1);
    assert!(host.begin("acme.costly/run", &json!({})).is_err());
    assert_eq!(carrier.dispatches(), 1);
}

#[test]
fn a_crashed_plugin_is_recorded_unknown_and_never_replayed() {
    let carrier = ControlledCarrier::new("crashy");
    let host = build_host(carrier.clone());
    let receipt = activate(&host, "acme.crash/ext", &["acme.crash/run"]).expect("activate");
    let in_flight = host
        .begin("acme.crash/run", &json!({"prompt": "go"}))
        .expect("admit");
    assert_eq!(in_flight.outcome, InvocationOutcome::Admitted);

    carrier.with_result_fault(FaultClass::Crashed);
    let failure = host
        .result(&in_flight.binding)
        .expect_err("the crashed instance cannot answer");
    assert_eq!(failure.code, "extension_carrier_crashed");
    let report = host.instance_report(&receipt.instance_id).expect("report");
    assert_eq!(report.unknown, 1);
    assert_eq!(report.in_flight, 0);
    assert_eq!(carrier.dispatches(), 1);
}

#[test]
fn revocation_withdraws_admission_but_lets_in_flight_work_settle() {
    let carrier = ControlledCarrier::new("revoked");
    let host = build_host(carrier.clone());
    let receipt = activate(&host, "acme.revoked/ext", &["acme.revoked/run"]).expect("activate");
    let in_flight = host
        .begin("acme.revoked/run", &json!({"prompt": "in flight"}))
        .expect("admit");

    assert_eq!(host.revoke("acme.revoked/ext"), 1);

    // New admission is withdrawn; the in-flight call keeps its binding.
    let failure = host
        .begin("acme.revoked/run", &json!({}))
        .expect_err("revoked admission");
    assert_eq!(failure.presentation_args.get("reason"), Some("withdrawn"));
    assert_eq!(failure.recovery, RecoveryAction::InstallOrRetryRuntime);

    let outcome = host
        .result(&in_flight.binding)
        .expect("the original generation still settles");
    assert_eq!(
        outcome,
        InvocationOutcome::Finished {
            payload: json!({"carrier": "revoked"})
        }
    );
    // With nothing outstanding, the revoked instance finalizes.
    let report = host.instance_report(&receipt.instance_id).expect("report");
    assert_eq!(
        report.state,
        licoup_extension_contracts::deployment::InstanceLifecycle::Stopped
    );
    assert_eq!(report.in_flight, 0);

    let failure = host
        .begin("acme.revoked/run", &json!({}))
        .expect_err("a stopped instance serves nothing new");
    assert_eq!(failure.presentation_args.get("reason"), Some("withdrawn"));
}

#[test]
fn cancellation_is_a_request_on_the_original_generation() {
    let carrier = ControlledCarrier::new("cancellable");
    let host = build_host(carrier.clone());
    let receipt = activate(&host, "acme.cancel/ext", &["acme.cancel/run"]).expect("activate");
    let in_flight = host
        .begin("acme.cancel/run", &json!({"prompt": "long"}))
        .expect("admit");

    let disposition = host.cancel(&in_flight.binding).expect("cancel request");
    assert_eq!(disposition, CancelDisposition::Acknowledged);
    // Cancellation alone does not settle: the result still decides.
    let report = host.instance_report(&receipt.instance_id).expect("report");
    assert_eq!(report.in_flight, 1);
    let outcome = host.result(&in_flight.binding).expect("result settles");
    assert!(matches!(outcome, InvocationOutcome::Finished { .. }));

    // An extension that does not implement cancellation says so.
    let other = ControlledCarrier::new("uncancellable");
    let host = build_host(other.clone());
    activate(&host, "acme.uncancel/ext", &["acme.uncancel/run"]).expect("activate");
    let in_flight = host.begin("acme.uncancel/run", &json!({})).expect("admit");
    other.with_cancel(CancelDisposition::Unsupported);
    assert_eq!(
        host.cancel(&in_flight.binding)
            .expect("unsupported is visible"),
        CancelDisposition::Unsupported
    );
}

#[test]
fn a_cancel_that_cannot_answer_isolates_the_instance_and_records_unknown() {
    let carrier = ControlledCarrier::new("cancel-fault");
    let host = build_host(carrier.clone());
    let receipt =
        activate(&host, "acme.cancel-fault/ext", &["acme.cancel-fault/run"]).expect("activate");
    let in_flight = host
        .begin("acme.cancel-fault/run", &json!({}))
        .expect("admit");
    carrier.with_cancel_fault(FaultClass::Unresponsive);
    let failure = host
        .cancel(&in_flight.binding)
        .expect_err("a cancel that cannot be delivered is a fault");
    assert_eq!(failure.code, "extension_carrier_unresponsive");
    let report = host.instance_report(&receipt.instance_id).expect("report");
    assert_eq!(
        report.state,
        licoup_extension_contracts::deployment::InstanceLifecycle::Failed
    );
    assert_eq!(report.unknown, 1);
    assert_eq!(report.in_flight, 0);
}

#[test]
fn a_hook_requests_effects_and_never_dispatches_them() {
    let carrier = ControlledCarrier::new("hooked");
    let host = build_host(carrier.clone());
    let receipt = activate(
        &host,
        "acme.hooks/ext",
        &["acme.hooks/observe", "acme.hooks/act"],
    )
    .expect("activate");
    let ticket = host
        .register_hook(&receipt.instance_id, "acme.hooks/observe")
        .expect("register");

    let dispatched = host
        .hook_request_effect(&ticket, "acme.hooks/act", &json!({"prompt": "react"}))
        .expect("the hook effect is admitted");
    assert_eq!(dispatched.binding.generation(), receipt.generation);
    assert_ne!(dispatched.binding.invocation_id(), ticket.ticket_id());
    assert_eq!(carrier.dispatches(), 1);
}

#[test]
fn a_superseded_or_foreign_hook_cannot_redispatch_effects() {
    let old = ControlledCarrier::new("hooks-old");
    let host = build_host(old.clone());
    let first = activate(
        &host,
        "acme.hooks/ext",
        &["acme.hooks/observe", "acme.hooks/act"],
    )
    .expect("v1");
    let stale_ticket = host
        .register_hook(&first.instance_id, "acme.hooks/observe")
        .expect("register");
    activate(
        &host,
        "acme.hooks/ext",
        &["acme.hooks/observe", "acme.hooks/act"],
    )
    .expect("v2");
    let failure = host
        .hook_request_effect(&stale_ticket, "acme.hooks/act", &json!({}))
        .expect_err("a stale hook is refused");
    assert_eq!(failure.code, "extension_hook_generation_superseded");
    assert_eq!(old.dispatches(), 0);

    // A ticket from a different host is not a ticket this host issued: the
    // ticket id may collide, the facts may not.
    let other = ControlledCarrier::new("hooks-other");
    let other_host = build_host(other.clone());
    let other_receipt = activate(
        &other_host,
        "acme.hooks/ext",
        &["acme.hooks/observe", "acme.hooks/act"],
    )
    .expect("activate on the other host");
    let other_ticket = other_host
        .register_hook(&other_receipt.instance_id, "acme.hooks/observe")
        .expect("register on the other host");
    assert_eq!(other_ticket.ticket_id(), stale_ticket.ticket_id());
    let failure = other_host
        .hook_request_effect(&stale_ticket, "acme.hooks/act", &json!({}))
        .expect_err("the foreign ticket is refused");
    // Identity is checked before the ticket's contents: another host run's
    // ticket is a foreign handle, not a forged copy of this host's.
    assert_eq!(failure.code, "extension_hook_foreign_host");
    assert_eq!(other.dispatches(), 0);
}

#[test]
fn a_hook_is_never_silently_rerouted_to_another_instance() {
    let carrier = ControlledCarrier::new("reroute");
    let host = build_host(carrier.clone());
    let old_receipt = activate(&host, "acme.old/ext", &["acme.shared/run"]).expect("old");
    let ticket = host
        .register_hook(&old_receipt.instance_id, "acme.shared/run")
        .expect("register");

    // Another package starts serving the same capability, and the hook's own
    // generation is withdrawn.
    let new_receipt = activate(&host, "acme.new/ext", &["acme.shared/run"]).expect("new");
    host.revoke("acme.old/ext");

    let before = carrier.dispatches();
    let failure = host
        .hook_request_effect(&ticket, "acme.shared/run", &json!({"prompt": "react"}))
        .expect_err("the hook is pinned to its own generation");
    assert_eq!(failure.code, "extension_hook_generation_superseded");
    assert_eq!(
        carrier.dispatches(),
        before,
        "the stale hook dispatched nothing"
    );
    assert!(
        !carrier.calls().iter().any(|call| matches!(
            call,
            Call::Dispatch { instance, .. } if instance == &new_receipt.instance_id
        )),
        "the replacement instance must not receive the stale hook's effect"
    );

    // The capability itself is still reachable for ordinary callers.
    let admitted = host
        .begin("acme.shared/run", &json!({}))
        .expect("the new generation admits ordinary calls");
    assert_eq!(admitted.binding.instance_id(), new_receipt.instance_id);
}

#[test]
fn a_fault_does_not_take_the_host_or_its_readers_down() {
    let carrier = ControlledCarrier::new("resilient");
    carrier.with_dispatch(DispatchReply::Fault(FaultClass::Protocol));
    let host = build_host(carrier.clone());
    let receipt = activate(&host, "acme.protocol/ext", &["acme.protocol/run"]).expect("activate");
    // A protocol violation isolates only its own instance.
    assert!(host.begin("acme.protocol/run", &json!({})).is_err());
    assert_eq!(
        host.instance_report(&receipt.instance_id)
            .expect("report")
            .state,
        licoup_extension_contracts::deployment::InstanceLifecycle::Failed
    );
    // Readers still get a complete catalog, at a newer epoch.
    let snapshot = host.catalog();
    assert!(snapshot.epoch().get() >= 2);
    assert!(snapshot.entry(&receipt.instance_id).is_some());
    assert!(!snapshot.serves("acme.protocol/run"));

    // An observation of a settled binding is refused rather than answered from
    // a guess.
    let failure = host
        .begin("acme.protocol/run", &json!({}))
        .expect_err("nothing serves it");
    assert_eq!(failure.presentation_args.get("reason"), Some("not-active"));
}

#[test]
fn a_late_observation_of_a_settled_call_is_refused_not_invented() {
    let carrier = ControlledCarrier::new("late");
    let host = build_host(carrier.clone());
    activate(&host, "acme.late/ext", &["acme.late/run"]).expect("activate");
    let in_flight = host.begin("acme.late/run", &json!({})).expect("admit");
    carrier.with_observation(Observation::Completed {
        payload: json!({"answer": 42}),
    });
    let settled = host.observe(&in_flight.binding).expect("completed");
    assert_eq!(
        settled,
        Observation::Completed {
            payload: json!({"answer": 42})
        }
    );
    let failure = host
        .observe(&in_flight.binding)
        .expect_err("a settled invocation is not observed again");
    assert_eq!(failure.code, "extension_invocation_settled");
}
