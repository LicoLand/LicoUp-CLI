#![deny(clippy::wildcard_enum_match_arm)]

//! Configuration-owned transition tables and constant-time local executors.
//!
//! The build compiles `resources/state-machines.json`. Generated types and
//! tables are private build output; handwritten code owns effects and guards,
//! never a second transition relation.
use core::fmt;

include!(concat!(env!("OUT_DIR"), "/state_machines.rs"));

pub use conversation_send::{
    Event as SendEvent, State as SendState, TRANSITIONS as SEND_TRANSITIONS,
    Transition as SendTransition,
};
pub use conversation_turn::{
    Event as TurnEvent, State as TurnState, TRANSITIONS as TURN_TRANSITIONS,
    Transition as TurnTransition,
};

pub const ALL_SEND_EVENTS: &[SendEvent] = &conversation_send::ALL_EVENTS;
pub const ALL_SEND_STATES: &[SendState] = &conversation_send::ALL_STATES;
pub const ALL_TURN_EVENTS: &[TurnEvent] = &conversation_turn::ALL_EVENTS;
pub const ALL_TURN_STATES: &[TurnState] = &conversation_turn::ALL_STATES;

impl TurnState {
    pub const fn transition(self, event: TurnEvent) -> Result<Self, TransitionError> {
        match conversation_turn::transition(self, event) {
            Some(next) => Ok(next),
            None => Err(TransitionError::Turn { state: self, event }),
        }
    }

    pub const fn is_terminal(self) -> bool {
        conversation_turn::terminal(self)
    }
}

impl SendState {
    pub const fn transition(self, event: SendEvent) -> Result<Self, TransitionError> {
        match conversation_send::transition(self, event) {
            Some(next) => Ok(next),
            None => Err(TransitionError::Send { state: self, event }),
        }
    }

    pub const fn is_terminal(self) -> bool {
        conversation_send::terminal(self)
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum TransitionError {
    Turn { state: TurnState, event: TurnEvent },
    Send { state: SendState, event: SendEvent },
}

impl fmt::Display for TransitionError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Turn { state, event } => {
                write!(formatter, "invalid turn transition: {state:?} + {event:?}")
            }
            Self::Send { state, event } => {
                write!(formatter, "invalid send transition: {state:?} + {event:?}")
            }
        }
    }
}

impl std::error::Error for TransitionError {}
