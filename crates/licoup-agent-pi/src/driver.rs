//! The Pi driver: this Agent's process half and the vocabulary it speaks.
//!
//! A Pi RPC turn is described by this Agent's own facts: the failure shape its
//! protocol reports, the effective settings a session negotiated, the launch
//! configuration one request becomes, and the session records a native resume
//! resolves against. All four live beside the parser that reads Pi's frames
//! rather than in the client that starts the process.
//!
//! What runs the turn is here too, and it is the whole of what the host
//! composed before: launching `pi --mode rpc --offline`, reading its JSONL
//! lines, supervising the turn, answering `steer` control requests, probing the
//! executable for the capabilities one session needs, and reporting the
//! outcome. The host names this module for the runtime protocol identity, the
//! probe, the execution and the active-turn steering; it holds no Pi launch, no
//! Pi frame rule and no Pi turn phase of its own.
//!
//! The one thing this half does not own is *where* the events of a turn go: that
//! is the host's, because the host owns the consumer, and it arrives through
//! [`crate::port::turn_event`].

mod active_control;
mod execution;
mod io;
mod probe;
mod supervision;

pub mod errors;
pub mod model;
pub mod params;
pub mod sessions;

pub use active_control::{ControlDisposition, steer};
pub use execution::execute;
pub use model::{CapabilityProbe, EffectiveSettings, RUNTIME_PROTOCOL, RunResult};
pub use probe::probe;

#[cfg(test)]
mod tests;
