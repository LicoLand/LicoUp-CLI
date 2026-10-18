//! The OpenClaw process half: the whole of what one turn runs.
//!
//! The protocol vocabulary this driver speaks — the Gateway ACP state machine,
//! the byte-line codec that classifies one frame exactly once, the allowlisted
//! update projection, the session continuity binding, the run and failure
//! vocabulary, and the request validation — already lives in this package
//! ([`crate::parser`], [`crate::gateway_acp`]) and is read here at its own path
//! rather than through a second name.
//!
//! What this module adds is the *process* half, which the client composed until
//! VENDOR-CODE-REMOVAL retired it: probing the selected executable's bounded
//! capability, resolving the Gateway attach endpoint, spawning the ACP bridge
//! (`supervision`), reading its framed lines under a byte bound (`io`),
//! supervising the turn to its protocol finish (`execution`) and cancelling an
//! active turn through the shared control plane.
//!
//! Two facts here are not the package's and do not become its own: the client's
//! Gateway lifecycle, which arrives through [`crate::port::gateway`], and *which*
//! MCP servers a turn registers, which the caller supplies to
//! [`execute_with_connection`] exactly as it supplied it to the former kernel
//! driver. A package whose host installed no port still runs a turn against an
//! explicit `gatewayWsUrl`, registering what its caller supplied, which is what
//! this package's own suite drives.

mod execution;
mod io;
mod probe;
mod supervision;

#[cfg(test)]
mod tests;

pub use crate::gateway_acp::errors::ProtocolFailure;
pub use crate::gateway_acp::model::{
    CapabilityProbe, EffectiveSettings, RUNTIME_PROTOCOL, RunResult,
};
pub use execution::execute_with_connection;
pub use probe::probe;

use licoup_agent_drivers::acp_driver_runtime::ControlDisposition;

/// Cancel the active OpenClaw turn on one session.
///
/// The active-turn registry belongs to the shared control plane; this package
/// supplies only the identity its own sessions are pooled by, which is the
/// driver identity its registration declares.
pub fn cancel(session_id: &str) -> ControlDisposition {
    licoup_agent_drivers::acp_driver_runtime::cancel_active_turn("openclaw-acp", session_id)
}
