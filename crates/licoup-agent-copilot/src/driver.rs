//! Copilot's immutable ACP launch declaration and its two bounded entry points.
//!
//! The shared ACP engine owns how a driver is started, bounded, supervised and
//! reconciled; what Copilot *is* to that engine — its runtime protocol
//! identity, the driver identity the transport pools and cancels sessions by,
//! the arguments its ACP entry point takes, and the error prefix its failures
//! are worded with — is Copilot's own fact and lives here.
//!
//! The declaration is metadata only. The frame dialect the transport reads is
//! resolved from [`crate::dialect`] by [`DRIVER_ID`], so this module cannot
//! name a frame rule and a dialect change is never a change to the spec.
//!
//! What runs the declaration is here too, and it is the whole of what the host
//! composed before: probing the canonical entry point and running one turn, both
//! through the shared engine. Copilot asks its host for nothing beyond admission
//! to run at all, because the engine, the frames and the outcome are already the
//! engine's and this package's.

use std::path::Path;

use licoup_agent_drivers::acp_driver_runtime::{
    AcpDriverSpec, CapabilityProbe, ControlDisposition, ProtocolFailure, RunResult,
    cancel_active_turn, execute_acp, probe_acp,
};
use serde_json::Value;

#[cfg(test)]
mod tests;

/// The runtime protocol identity Copilot's frames are recorded and reported
/// under.
pub const RUNTIME_PROTOCOL: &str = "copilot-acp-v1-stdio-ndjson";

/// The inbound format this package's declared native converter reads, named by
/// the crate rather than retyped in the release document.
pub const PROTOCOL_FORMAT: &str = "copilot.acp.v1";

/// The driver identity the shared ACP transport pools and cancels Copilot
/// sessions by. It is also the identity [`crate::dialect`] registers under.
pub const DRIVER_ID: &str = "copilot-acp";

/// The error prefix Copilot's protocol failures are worded with.
pub const ERROR_PREFIX: &str = "copilot_acp";

/// Copilot's ACP entry point: the one canonical invocation this Agent is
/// reached through.
pub const LAUNCH_ARGS: &[&str] = &["--acp", "--stdio", "--no-auto-update"];

/// Copilot's immutable launch declaration, as the shared engine reads it.
pub const DRIVER: AcpDriverSpec =
    AcpDriverSpec::new(RUNTIME_PROTOCOL, LAUNCH_ARGS).with_identity(DRIVER_ID, ERROR_PREFIX);

/// Probe Copilot's executable for the ACP capabilities one session needs,
/// through the same canonical entry point a turn uses.
pub fn capability_probe(
    executable: &str,
    cwd: &Path,
    timeout_ms: u64,
    max_stdout: Option<usize>,
    max_stderr: usize,
) -> Result<CapabilityProbe, ProtocolFailure> {
    probe_acp(DRIVER, executable, cwd, timeout_ms, max_stdout, max_stderr)
}

/// Run one Copilot turn through the shared ACP engine.
///
/// The result carries the raw protocol failure the engine reported; projecting
/// it onto the host's normalized execution vocabulary stays with the
/// composition, because that vocabulary is the host's and not this Agent's.
pub fn execute(
    executable: &str,
    params: &Value,
    prompt: &str,
    session_id: &str,
    cwd: Option<&Path>,
    timeout_ms: u64,
    max_stdout: Option<usize>,
    max_stderr: usize,
) -> RunResult {
    execute_acp(
        DRIVER, executable, params, prompt, session_id, cwd, timeout_ms, max_stdout, max_stderr,
    )
}

/// Cancel the active Copilot turn on one session.
///
/// The active-turn registry belongs to the shared control plane; this package
/// supplies only the identity its own sessions are pooled by.
pub fn cancel(session_id: &str) -> ControlDisposition {
    cancel_active_turn(DRIVER.agent_id, session_id)
}
