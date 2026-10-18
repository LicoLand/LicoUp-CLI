//! The Kimi Code half of one ACP execution.
//!
//! Kimi Code officially exposes ACP v1 over stdio through `kimi acp`. Keeping
//! every operation on that one transport prevents a session created by one
//! protocol from being resumed through a different protocol with a misleading
//! identity, so this module declares exactly one launch and one session
//! identity and never opens a second route to the Agent.
//!
//! What is Kimi's, and therefore here: the runtime protocol identity, the
//! driver identity the transport pools and cancels by, the launch arguments,
//! the model and reasoning settings Kimi reads, and the autonomous-mode flag.
//! What is not: the ACP engine that runs them. The shared engine in
//! `licoup-agent-drivers` owns framing, capability negotiation, the session
//! lifecycle, bounded process I/O and failure projection, and it resolves this
//! Agent's frame dialect from the transport port through
//! [`crate::dialect::DRIVER_ID`] — so the engine runs a Kimi session without
//! naming Kimi, and this package describes one without reimplementing the
//! engine.
//!
//! Both entry points are fail-closed through [`crate::port::execution`]: a host
//! that installed no execution port admits nothing, so a package running outside
//! the client reports the refusal rather than starting a process.

use std::path::Path;

use licoup_agent_drivers::acp_driver_runtime::{
    AcpDriverSpec, CapabilityProbe, ControlDisposition, ProtocolFailure, RunResult,
    cancel_active_turn, execute_acp, probe_acp,
};
use serde_json::Value;

/// The runtime protocol Kimi Code is reached through, as the packaged driver
/// declaration reports it.
pub const RUNTIME_PROTOCOL: &str = "kimi-code-acp-v1-stdio-ndjson";

/// The wire format this package's entry speaks, as its release declaration
/// names it. It is the ACP v1 profile Kimi Code publishes over stdio, not a
/// Kimi-private envelope.
pub const PROTOCOL_FORMAT: &str = "kimi-code.acp.v1";

/// The one Kimi Code driver this package declares.
///
/// The frame dialect this driver reads is not a field: the transport resolves it
/// from [`crate::dialect::DRIVER_ID`], exactly as it resolves every other
/// Agent's, so this spec names no parser policy and a change to Kimi's dialect
/// is not a change to the engine.
pub const DRIVER: AcpDriverSpec = AcpDriverSpec::new(RUNTIME_PROTOCOL, &["acp"])
    .with_identity(crate::dialect::DRIVER_ID, "kimi_code_acp")
    .with_launch_settings(
        "--model",
        "KIMI_MODEL_THINKING_EFFORT",
        &["low", "high", "max"],
    )
    // ACP subagents have no interactive user attached. Kimi's `--yolo`
    // auto-approves regular tools but may still open permission questions;
    // `--auto` is the documented fully autonomous mode and is therefore the
    // only launch flag that preserves an explicit `allowAll: true` request.
    .with_allow_all_argument("--auto");

/// Probe Kimi Code's executable for the ACP capabilities one session needs.
pub fn capability_probe(
    executable: &str,
    cwd: &Path,
    timeout_ms: u64,
    max_stdout: Option<usize>,
    max_stderr: usize,
) -> Result<CapabilityProbe, ProtocolFailure> {
    if !crate::port::execution::admits_execution() {
        return Err(refused_admission());
    }
    probe_acp(DRIVER, executable, cwd, timeout_ms, max_stdout, max_stderr)
}

/// Run one Kimi Code turn through the shared ACP engine.
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
    if !crate::port::execution::admits_execution() {
        return RunResult::failed(
            DRIVER,
            refused_admission(),
            licoup_agent_drivers::acp_driver_runtime::timestamp(),
            None,
            false,
            false,
            CapabilityProbe::default(),
            Vec::new(),
        );
    }
    execute_acp(
        DRIVER, executable, params, prompt, session_id, cwd, timeout_ms, max_stdout, max_stderr,
    )
}

/// Cancel the active Kimi turn on one session.
///
/// The active-turn registry belongs to the shared control plane; this package
/// supplies only the identity its own sessions are pooled by.
pub fn cancel(session_id: &str) -> ControlDisposition {
    cancel_active_turn(DRIVER.agent_id, session_id)
}

/// The one refusal a package whose host installed no execution port reports.
///
/// It is a protocol failure like any other, so a caller reads it through the
/// same shape as a refusal the Agent itself reported, and the code states which
/// side refused.
fn refused_admission() -> ProtocolFailure {
    ProtocolFailure::new(
        "kimi_code_acp_execution_not_admitted",
        "The host admits no new Agent execution on this process.",
        "dispatch",
    )
}
