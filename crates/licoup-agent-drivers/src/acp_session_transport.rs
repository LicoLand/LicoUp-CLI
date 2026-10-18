//! Persistent ACP session transport shared by vendor adapters.
//!
//! The pool is keyed by driver identity, executable, and workspace. Session
//! control is likewise driver-scoped so native identifiers from different ACP
//! implementations can never alias each other in shared process state.

pub mod approval_port;
mod approval_store;
mod approval_wait;
mod capabilities;
mod command;
mod continuity;
pub mod errors;
mod events;
mod execution;
mod io;

mod protocol;
mod supervision;

pub use approval_store::register_park_and_inbox;
pub use approval_store::resolve_interaction_approval;
pub use capabilities::{AcpSessionDriverSpec, CapabilityProbe, EffectiveSettings, RunResult};
pub use continuity::{ControlDisposition, cancel, cleanup_session};
pub use errors::ProtocolFailure;
pub use events::{TransportEvent, read_protocol_messages, request_id_matches};
pub use execution::execute;
pub use io::{drain_bounded, drain_stderr, read_bounded, write_message};

// The bounds the host's own Hermes suites assert against, and the protocol
// vocabulary those suites drive the reducer with. They are `test-support`
// rather than `cfg(test)` for the reason stated in `acp_driver_runtime::replay`:
// the host's test build links this crate, so a `cfg(test)` re-export is
// compiled out of the build that reads it.
#[cfg(any(test, feature = "test-support"))]
pub use capabilities::{
    APPROVAL_POLL_INTERVAL, CONTROL_QUEUE_CAPACITY, MAX_POOLED_TRANSPORTS, MAX_TRACKED_SESSIONS,
    PROCESS_POLL_INTERVAL,
};
#[cfg(any(test, feature = "test-support"))]
pub use command::{LaunchSpec, ProtocolConfig};
#[cfg(any(test, feature = "test-support"))]
pub use protocol::{
    INITIALIZE_REQUEST_ID, MODEL_REQUEST_ID, PROMPT_REQUEST_ID, ProtocolEffect, ProtocolPhase,
    SESSION_REQUEST_ID, SessionProtocol,
};

#[cfg(test)]
mod tests;
