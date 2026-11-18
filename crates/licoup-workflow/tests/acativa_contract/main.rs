//! The engine contract this crate publishes.
//!
//! `licoup-workflow` is allowed to own LicoUp vocabulary only: the schema
//! identity, the persisted snapshot sideband and the Assistant event
//! spellings. It is not allowed to grow a second graph machine. These checks
//! pin that boundary at the type level, so a future local copy of a fact the
//! engine already owns fails to compile here instead of drifting silently.

use std::collections::BTreeMap;

use licoup_workflow::machine;

/// Unify two spellings of one type. Rust has no structural typing, so the
/// call compiles only while both paths name the very same item.
fn one_type<T>(value: T) -> T {
    value
}

#[test]
fn shared_machine_facts_are_the_engine_own_types() {
    // Commands, statuses and reducer output move between the durable store
    // and the engine without translation; a local copy would need one.
    let _: Option<acativa::workflow::machine::RunCommand> = one_type(None::<machine::RunCommand>);
    let _: Option<acativa::workflow::machine::CommandKind> = one_type(None::<machine::CommandKind>);
    let _: Option<acativa::workflow::machine::CommandStatus> =
        one_type(None::<machine::CommandStatus>);
    let _: Option<acativa::workflow::machine::ReducerDelta> =
        one_type(None::<machine::ReducerDelta>);
    let _: Option<acativa::workflow::machine::SharedResourceState> =
        one_type(None::<machine::SharedResourceState>);
    let _: Option<acativa::workflow::machine::JoinLedger> = one_type(None::<machine::JoinLedger>);
    let _: Option<acativa::workflow::machine::VisitFact> = one_type(None::<machine::VisitFact>);
}

#[test]
fn the_persisted_snapshot_keeps_engine_fields_and_a_named_l_sideband() {
    // Run status is the engine's; only the callback-decision spelling is
    // translated at the LicoUp boundary.
    let _: Option<acativa::workflow::ir::StrategyRunStatus> =
        one_type(None::<licoup_workflow::StrategyRunStatus>);

    let mut snapshot = machine::RunSnapshot::empty("run-contract", "definition", "semantics");
    snapshot.conversation_id = Some("conversation-1".to_owned());
    snapshot.assistant_membership_id = Some("membership-1".to_owned());
    snapshot.route_receipt = Some(serde_json::json!({"route": "direct"}));

    // The command ledger is keyed and valued by engine types, not by a
    // LicoUp mirror. This compiles only while both are the same items.
    fn is_engine_command_ledger(_: &BTreeMap<String, acativa::workflow::machine::RunCommand>) {}
    is_engine_command_ledger(&snapshot.commands);

    // The three LicoUp-only identities are the documented sideband over the
    // engine's opaque `context_id` / `run_binding` pair.
    assert_eq!(snapshot.conversation_id.as_deref(), Some("conversation-1"));
    assert_eq!(
        snapshot.assistant_membership_id.as_deref(),
        Some("membership-1")
    );
    assert_eq!(
        snapshot.route_receipt,
        Some(serde_json::json!({"route": "direct"}))
    );
}

#[test]
fn the_engine_limits_are_the_published_limits() {
    assert_eq!(
        licoup_workflow::MAX_ACTIVE_EFFECTS,
        acativa::MAX_ACTIVE_EFFECTS
    );
    assert_eq!(licoup_workflow::MAX_GRAPH_STATES, acativa::MAX_GRAPH_STATES);
    assert_eq!(
        licoup_workflow::MAX_GRAPH_TRANSITIONS,
        acativa::MAX_GRAPH_TRANSITIONS
    );
    assert_eq!(
        licoup_workflow::MAX_BINDING_SLOTS,
        acativa::MAX_BINDING_SLOTS
    );
    assert_eq!(
        licoup_workflow::MAX_RUNTIME_REQUIREMENTS,
        acativa::MAX_RUNTIME_REQUIREMENTS
    );
    assert_eq!(
        licoup_workflow::MAX_WORKSET_ITEMS,
        acativa::MAX_WORKSET_ITEMS
    );
    assert_eq!(
        licoup_workflow::MAX_RETRY_ATTEMPTS,
        acativa::MAX_RETRY_ATTEMPTS
    );
}

#[test]
fn the_dispatch_decisions_are_the_engine_own_types() {
    let _: Option<acativa::workflow::dispatch::ClaimVerdict> =
        one_type(None::<licoup_workflow::ClaimVerdict>);
    let _: Option<acativa::workflow::dispatch::ClaimLease> =
        one_type(None::<licoup_workflow::ClaimLease>);
    let _: Option<acativa::workflow::dispatch::EffectCompletion> =
        one_type(None::<licoup_workflow::EffectCompletion>);
    let _: Option<acativa::workflow::dispatch::RecoveryVerdict> =
        one_type(None::<licoup_workflow::RecoveryVerdict>);
    let _: Option<acativa::workflow::dispatch::SuccessorAdmission> =
        one_type(None::<licoup_workflow::SuccessorAdmission>);
}
