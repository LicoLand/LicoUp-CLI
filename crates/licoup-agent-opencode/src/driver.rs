//! This Agent's own half of one turn.
//!
//! The client's serve engine owns the sockets, the process and the active-turn
//! registry. What the engine cannot know is what *OpenCode* needs: the request
//! shape it accepts, the identity document it answers with, how a completed
//! message becomes this Agent's transitions, and what its endpoint must report
//! before a turn may start. That is what this module tree owns.
//!
//! - [`control`] reaches the shared active-turn registry force stop reads, and
//!   states this Agent's capability answer.
//! - [`continuity`] binds the native conversation a turn runs against: an exact
//!   load for a resume, a fresh session otherwise.
//! - [`serve_transport`] performs the turn against the engine's port: post the
//!   message, project the stream's chunks and the terminal document.
//! - [`probe`] is the capability probe the host runs before offering the Agent.
//!
//! Nothing here opens a socket or starts a process: every engine operation
//! arrives through [`crate::port::serve`], so a package running without its host
//! reports that it could not ask rather than performing the effect itself.

mod continuity;
mod control;
mod probe;
mod serve_transport;

use licoup_agent_drivers::acp_driver_runtime::AcpDriverSpec;

pub use control::{cancel, serve_capabilities};
pub use probe::capability_probe;
pub use serve_transport::{ServeStreamFailure, execute, watch_session_events};

/// This Agent's failure type: a closed code, a fixed message and the stage it
/// was observed at. The shared ACP driver vocabulary, named here so this
/// module's signatures read as this Agent's.
pub use licoup_agent_drivers::acp_driver_runtime::ProtocolFailure;
/// The shared ACP driver vocabulary this Agent's declaration is stated in.
pub use licoup_agent_drivers::acp_driver_runtime::{
    CapabilityProbe, EffectiveSettings, ProtocolConfig, RunResult,
};

/// The runtime protocol identity OpenCode's frames are recorded and reported
/// under.
pub const RUNTIME_PROTOCOL: &str = "opencode-serve-http-v1";

/// OpenCode's immutable serve launch declaration, as the shared engine reads it.
pub const OPENCODE_DRIVER: AcpDriverSpec = AcpDriverSpec::new(RUNTIME_PROTOCOL, &["serve"])
    .with_identity("opencode-serve", "opencode_serve");

#[cfg(test)]
mod tests;
