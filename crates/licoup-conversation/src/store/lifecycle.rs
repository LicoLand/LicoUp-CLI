//! Pure lifecycle selection for durable Conversation store effects.
//!
//! SQL transactions remain in their owning store modules. This module is the
//! single adapter from store operations to the generated transition tables.

use crate::{DispatchDeliveryState, DispatchState, MembershipStatus, SubagentDispatchClaimState};

pub(crate) fn initial_dispatch() -> DispatchState {
    crate::state_machine::conversation_dispatch::INITIAL
}

pub(crate) fn start_dispatch() -> DispatchState {
    use crate::state_machine::conversation_dispatch as machine;
    machine::transition(machine::State::Accepted, machine::Event::Start)
        .expect("configured accepted dispatch starts running")
}

pub(crate) fn dispatch_permits(current: DispatchState, next: DispatchState) -> bool {
    current == next || crate::state_machine::conversation_dispatch::permits(current, next)
}

pub(crate) fn fail_dispatch(current: DispatchState) -> Option<DispatchState> {
    crate::state_machine::conversation_dispatch::transition(
        current,
        crate::state_machine::conversation_dispatch::Event::Fail,
    )
}

pub(crate) fn initial_membership() -> MembershipStatus {
    crate::state_machine::conversation_membership::INITIAL
}

pub(crate) fn leave_membership() -> MembershipStatus {
    use crate::state_machine::conversation_membership as machine;
    machine::transition(machine::State::Active, machine::Event::Leave)
        .expect("configured active membership can leave")
}

pub(crate) fn reactivate_membership() -> MembershipStatus {
    use crate::state_machine::conversation_membership as machine;
    machine::transition(machine::State::Left, machine::Event::Reactivate)
        .expect("configured membership reactivation returns active")
}

pub(crate) fn initial_claim() -> SubagentDispatchClaimState {
    crate::state_machine::conversation_subagent_claim::INITIAL
}

pub(crate) fn valid_claim_transition(current: &str, next: SubagentDispatchClaimState) -> bool {
    use crate::state_machine::conversation_subagent_claim as machine;
    let Some(current) = machine::State::from_name(current) else {
        return false;
    };
    let event = match next {
        machine::State::Running if current == machine::State::Claimed => machine::Event::Start,
        machine::State::Running => machine::Event::Resume,
        machine::State::CancelRequested => machine::Event::RequestCancel,
        machine::State::ReconciliationRequired => machine::Event::RequireReconciliation,
        machine::State::Completed => machine::Event::Complete,
        machine::State::Failed => machine::Event::Fail,
        machine::State::Cancelled => machine::Event::Cancel,
        machine::State::Claimed => return false,
    };
    machine::transition(current, event) == Some(next)
}

pub(crate) fn reconcile_claim(current: &str, dispatch: &str) -> Option<SubagentDispatchClaimState> {
    use crate::state_machine::conversation_subagent_claim as machine;
    let current = machine::State::from_name(current)?;
    let event = match DispatchState::from_name(dispatch)? {
        DispatchState::Accepted | DispatchState::Running => machine::Event::ReconcileRunning,
        DispatchState::CancelRequested => machine::Event::ReconcileCancelRequested,
        DispatchState::Completed => machine::Event::ReconcileCompleted,
        DispatchState::Failed => machine::Event::ReconcileFailed,
        DispatchState::Cancelled => machine::Event::ReconcileCancelled,
    };
    machine::transition(current, event)
}

/// Bounded SQL relation derived from the generated claim and dispatch
/// machines. Reconciliation remains set-based regardless of stored claim
/// count, while the JSON machines remain the sole transition authority.
pub(crate) fn claim_reconciliation_transitions(
    terminal_dispatches_only: bool,
) -> Vec<(&'static str, &'static str, &'static str)> {
    use crate::state_machine::{conversation_dispatch, conversation_subagent_claim};

    let mut transitions = Vec::new();
    for claim in conversation_subagent_claim::ALL_STATES {
        if conversation_subagent_claim::terminal(claim) {
            continue;
        }
        for dispatch in conversation_dispatch::ALL_STATES {
            if terminal_dispatches_only && !conversation_dispatch::terminal(dispatch) {
                continue;
            }
            if let Some(next) = reconcile_claim(claim.as_str(), dispatch.as_str()) {
                transitions.push((claim.as_str(), dispatch.as_str(), next.as_str()));
            }
        }
    }
    transitions
}

pub(crate) fn settle_claim(
    current: &str,
    dispatch: DispatchState,
) -> Option<SubagentDispatchClaimState> {
    use crate::state_machine::conversation_subagent_claim as machine;
    let current = machine::State::from_name(current)?;
    let event = match dispatch {
        DispatchState::Completed => machine::Event::Complete,
        DispatchState::Failed => machine::Event::Fail,
        DispatchState::Cancelled => machine::Event::Cancel,
        _ => return None,
    };
    machine::transition(current, event)
}

pub(crate) fn initial_delivery() -> DispatchDeliveryState {
    crate::state_machine::conversation_dispatch_delivery::INITIAL
}

pub(crate) fn begin_delivery() -> DispatchDeliveryState {
    use crate::state_machine::conversation_dispatch_delivery as machine;
    machine::transition(machine::State::Pending, machine::Event::Begin)
        .expect("configured pending delivery begins delivering")
}

pub(crate) fn retry_delivery() -> DispatchDeliveryState {
    use crate::state_machine::conversation_dispatch_delivery as machine;
    machine::transition(machine::State::Delivering, machine::Event::Retry)
        .expect("configured interrupted delivery returns to pending")
}

pub(crate) fn admit_delivery(current: DispatchDeliveryState) -> Option<DispatchDeliveryState> {
    crate::state_machine::conversation_dispatch_delivery::transition(
        current,
        crate::state_machine::conversation_dispatch_delivery::Event::Deliver,
    )
}
