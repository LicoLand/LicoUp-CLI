//! Protocol-signal-only settlement for one Membership-scoped turn.
//!
//! Adapters report what happened at their protocol boundary. This arbiter is
//! the only component in the native host that turns those reports into a
//! canonical terminal state. It delegates every state change to the canonical
//! `licoup-conversation` FSM and records one projection delta for every
//! accepted state change.

use licoup_conversation::{SendEvent, SendState, TransitionError, TurnEvent, TurnState};
use serde_json::{Value, json};

/// Closed L4 signal vocabulary accepted by L5 settlement.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum SettlementSignal {
    /// The native protocol reported its own successful terminal message.
    ProtocolFinish,
    /// The carrier ended without a protocol terminal. Content already
    /// received makes this a complete response; an empty EOF is transport
    /// loss, never successful empty output.
    Eof { has_content: bool },
    /// The protocol or carrier reported a concrete error.
    Error,
    /// The supervised native cancel operation was acknowledged.
    CancelConfirmed,
    /// A caller-explicit, non-zero turn deadline expired.
    ExplicitDeadline,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum SettlementFailureReason {
    TransportLost,
    ProtocolError,
    DeadlineExceeded,
}

impl SettlementFailureReason {
    pub(crate) const fn wire_name(self) -> &'static str {
        match self {
            Self::TransportLost => "transportLost",
            Self::ProtocolError => "protocolError",
            Self::DeadlineExceeded => "deadlineExceeded",
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum SettlementOutcome {
    Completed,
    Failed(SettlementFailureReason),
    Cancelled,
}

impl SettlementOutcome {
    pub(crate) const fn wire_name(self) -> &'static str {
        match self {
            Self::Completed => "completed",
            Self::Failed(_) => "failed",
            Self::Cancelled => "cancelled",
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum DeltaCause {
    DispatchClaimed,
    DispatchStarted,
    ProtocolFinish,
    EofWithContent,
    TransportLost,
    ProtocolError,
    CancelConfirmed,
    DeadlineExceeded,
}

impl DeltaCause {
    const fn wire_name(self) -> &'static str {
        match self {
            Self::DispatchClaimed => "dispatchClaimed",
            Self::DispatchStarted => "dispatchStarted",
            Self::ProtocolFinish => "protocolFinish",
            Self::EofWithContent => "eofWithContent",
            Self::TransportLost => "transportLost",
            Self::ProtocolError => "protocolError",
            Self::CancelConfirmed => "cancelConfirmed",
            Self::DeadlineExceeded => "deadlineExceeded",
        }
    }
}

/// Typed delta projected for each canonical FSM state change.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum SettlementDelta {
    TurnStateChanged {
        from: TurnState,
        to: TurnState,
        cause: DeltaCause,
    },
    SendStateChanged {
        from: SendState,
        to: SendState,
        cause: DeltaCause,
    },
}

impl SettlementDelta {
    pub(crate) fn to_json(self) -> Value {
        match self {
            Self::TurnStateChanged { from, to, cause } => json!({
                "kind": "turnStateChanged",
                "from": turn_state_wire(from),
                "to": turn_state_wire(to),
                "cause": cause.wire_name(),
            }),
            Self::SendStateChanged { from, to, cause } => json!({
                "kind": "sendStateChanged",
                "from": send_state_wire(from),
                "to": send_state_wire(to),
                "cause": cause.wire_name(),
            }),
        }
    }
}

pub(crate) type SettlementError = TransitionError;

/// L5 stateful arbiter. Terminal states remain write-once because the
/// canonical FSM absorbs all later signals; no adapter can relabel an outcome.
#[derive(Debug)]
pub(crate) struct TurnSettlementArbiter {
    turn_state: TurnState,
    send_state: SendState,
    outcome: Option<SettlementOutcome>,
    deltas: Vec<SettlementDelta>,
}

impl Default for TurnSettlementArbiter {
    fn default() -> Self {
        Self {
            turn_state: TurnState::Pending,
            send_state: SendState::Sending,
            outcome: None,
            deltas: Vec::new(),
        }
    }
}

impl TurnSettlementArbiter {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    /// Enter the executable turn state through the canonical claim/start
    /// relation. Both changes are independently observable.
    pub(crate) fn begin_dispatch(&mut self) -> Result<(), SettlementError> {
        match self.turn_state {
            TurnState::Pending => {
                self.transition_turn(TurnEvent::Claim, DeltaCause::DispatchClaimed)?;
                self.transition_turn(TurnEvent::Start, DeltaCause::DispatchStarted)
            }
            TurnState::Claimed => {
                self.transition_turn(TurnEvent::Start, DeltaCause::DispatchStarted)
            }
            TurnState::WaitingForHuman => {
                self.transition_turn(TurnEvent::Resume, DeltaCause::DispatchStarted)
            }
            TurnState::Running
            | TurnState::Succeeded
            | TurnState::Failed
            | TurnState::Interrupted
            | TurnState::Cancelled => Ok(()),
        }
    }

    /// Decide the exact terminal outcome from one protocol-level signal.
    pub(crate) fn settle(
        &mut self,
        signal: SettlementSignal,
    ) -> Result<SettlementOutcome, SettlementError> {
        if let Some(outcome) = self.outcome {
            return Ok(outcome);
        }
        self.begin_dispatch()?;
        let (outcome, send_event, turn_event, cause) = match signal {
            SettlementSignal::ProtocolFinish => (
                SettlementOutcome::Completed,
                SendEvent::Deliver,
                TurnEvent::Succeed,
                DeltaCause::ProtocolFinish,
            ),
            SettlementSignal::Eof { has_content: true } => (
                SettlementOutcome::Completed,
                SendEvent::Deliver,
                TurnEvent::Succeed,
                DeltaCause::EofWithContent,
            ),
            SettlementSignal::Eof { has_content: false } => (
                SettlementOutcome::Failed(SettlementFailureReason::TransportLost),
                SendEvent::Fail,
                TurnEvent::Fail,
                DeltaCause::TransportLost,
            ),
            SettlementSignal::Error => (
                SettlementOutcome::Failed(SettlementFailureReason::ProtocolError),
                SendEvent::Fail,
                TurnEvent::Fail,
                DeltaCause::ProtocolError,
            ),
            SettlementSignal::CancelConfirmed => (
                SettlementOutcome::Cancelled,
                SendEvent::Deliver,
                TurnEvent::Cancel,
                DeltaCause::CancelConfirmed,
            ),
            SettlementSignal::ExplicitDeadline => (
                SettlementOutcome::Failed(SettlementFailureReason::DeadlineExceeded),
                SendEvent::Fail,
                TurnEvent::Fail,
                DeltaCause::DeadlineExceeded,
            ),
        };
        self.transition_send(send_event, cause)?;
        self.transition_turn(turn_event, cause)?;
        self.outcome = Some(outcome);
        Ok(outcome)
    }

    pub(crate) const fn turn_state(&self) -> TurnState {
        self.turn_state
    }

    pub(crate) const fn send_state(&self) -> SendState {
        self.send_state
    }

    #[cfg(test)]
    pub(crate) fn deltas(&self) -> &[SettlementDelta] {
        &self.deltas
    }

    pub(crate) fn drain_deltas(&mut self) -> Vec<SettlementDelta> {
        core::mem::take(&mut self.deltas)
    }

    fn transition_turn(
        &mut self,
        event: TurnEvent,
        cause: DeltaCause,
    ) -> Result<(), SettlementError> {
        let from = self.turn_state;
        let to = from.transition(event)?;
        if to != from {
            self.turn_state = to;
            self.deltas
                .push(SettlementDelta::TurnStateChanged { from, to, cause });
        }
        Ok(())
    }

    fn transition_send(
        &mut self,
        event: SendEvent,
        cause: DeltaCause,
    ) -> Result<(), SettlementError> {
        let from = self.send_state;
        let to = from.transition(event)?;
        if to != from {
            self.send_state = to;
            self.deltas
                .push(SettlementDelta::SendStateChanged { from, to, cause });
        }
        Ok(())
    }
}

pub(crate) const fn turn_state_wire(state: TurnState) -> &'static str {
    match state {
        TurnState::Pending => "pending",
        TurnState::Claimed => "claimed",
        TurnState::Running => "running",
        TurnState::WaitingForHuman => "waiting-for-human",
        TurnState::Succeeded => "succeeded",
        TurnState::Failed => "failed",
        TurnState::Interrupted => "interrupted",
        TurnState::Cancelled => "cancelled",
    }
}

pub(crate) const fn send_state_wire(state: SendState) -> &'static str {
    match state {
        SendState::Sending => "sending",
        SendState::Delivered => "delivered",
        SendState::Failed => "failed",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn settle(signal: SettlementSignal) -> TurnSettlementArbiter {
        let mut arbiter = TurnSettlementArbiter::new();
        arbiter.settle(signal).unwrap();
        arbiter
    }

    #[test]
    fn test_settlement_protocol_finish_completes_even_without_content() {
        let arbiter = settle(SettlementSignal::ProtocolFinish);
        assert_eq!(arbiter.outcome, Some(SettlementOutcome::Completed));
        assert_eq!(arbiter.turn_state(), TurnState::Succeeded);
        assert_eq!(arbiter.send_state(), SendState::Delivered);
        assert_eq!(arbiter.deltas().len(), 4);
    }

    #[test]
    fn test_settlement_eof_requires_content_and_deadline_is_explicit_failure() {
        let with_content = settle(SettlementSignal::Eof { has_content: true });
        assert_eq!(with_content.outcome, Some(SettlementOutcome::Completed));

        let empty = settle(SettlementSignal::Eof { has_content: false });
        assert_eq!(
            empty.outcome,
            Some(SettlementOutcome::Failed(
                SettlementFailureReason::TransportLost
            ))
        );

        let deadline = settle(SettlementSignal::ExplicitDeadline);
        assert_eq!(
            deadline.outcome,
            Some(SettlementOutcome::Failed(
                SettlementFailureReason::DeadlineExceeded
            ))
        );
    }

    #[test]
    fn test_settlement_every_state_change_has_exactly_one_delta() {
        let arbiter = settle(SettlementSignal::Error);
        assert_eq!(
            arbiter.deltas(),
            &[
                SettlementDelta::TurnStateChanged {
                    from: TurnState::Pending,
                    to: TurnState::Claimed,
                    cause: DeltaCause::DispatchClaimed,
                },
                SettlementDelta::TurnStateChanged {
                    from: TurnState::Claimed,
                    to: TurnState::Running,
                    cause: DeltaCause::DispatchStarted,
                },
                SettlementDelta::SendStateChanged {
                    from: SendState::Sending,
                    to: SendState::Failed,
                    cause: DeltaCause::ProtocolError,
                },
                SettlementDelta::TurnStateChanged {
                    from: TurnState::Running,
                    to: TurnState::Failed,
                    cause: DeltaCause::ProtocolError,
                },
            ]
        );
    }

    #[test]
    fn test_cancel_confirmed_is_cancelled_not_failed_and_is_write_once() {
        let mut arbiter = settle(SettlementSignal::CancelConfirmed);
        assert_eq!(arbiter.outcome, Some(SettlementOutcome::Cancelled));
        assert_eq!(arbiter.turn_state(), TurnState::Cancelled);
        assert_eq!(arbiter.send_state(), SendState::Delivered);
        let delta_count = arbiter.deltas().len();

        assert_eq!(
            arbiter.settle(SettlementSignal::Error).unwrap(),
            SettlementOutcome::Cancelled
        );
        assert_eq!(arbiter.turn_state(), TurnState::Cancelled);
        assert_eq!(arbiter.deltas().len(), delta_count);
    }
}
