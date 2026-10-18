//! The port this seam declares for the Agents it reaches.
//!
//! This crate owns the host-side work-context seam: the binding contract, the
//! transport, the production host driver and the C01 effect port. It owns no
//! Agent's protocol. Everything it needs *from* one Agent arrives through
//! [`AgentProtocolRegistration`] — how that Agent's protocol adapter is built,
//! how its driver is executed and which inventory id lane control addresses it
//! by — and the composition above this crate supplies those implementations:
//! `licoup-native` today, `licoup-agent-codex` and `licoup-agent-pi` once
//! those crates exist.
//!
//! The seam also reaches the host's own conversation lane on the steer and
//! cancel paths. That reach is not any Agent's protocol either, so it arrives
//! the same way, as [`LaneReach`], and this crate names no lane module.
//!
//! Every member is a `fn` pointer rather than a trait, so the seam keeps no
//! state a caller did not hand it, and no vendor name is compiled into it.

use std::path::Path;
use std::sync::Arc;

use licoup_agent_runtime::work_context::{
    NativeCapabilitySnapshot, NativeCapabilitySupport, WorkContextConfig, WorkContextRuntime,
};
use licoup_agent_targets::port::AgentTargetPort;
use licoup_conversation::ConversationStore;
use serde_json::Value;

use crate::transport::AdapterTransport;

/// The capability snapshot before a bound adapter has answered anything.
///
/// An answer this seam has not received is `Unverified`, never a silent
/// `Supported` and never a refusal; `fork` is the one dimension whose default
/// is `Unsupported`, because no adapter here has ever reported it. A resolved
/// adapter overrides this with its own negotiated snapshot.
pub fn unverified_snapshot() -> NativeCapabilitySnapshot {
    NativeCapabilitySnapshot {
        exact_resume: NativeCapabilitySupport::Unverified,
        fork: NativeCapabilitySupport::Unsupported,
        compact: NativeCapabilitySupport::Unverified,
        steer: NativeCapabilitySupport::Unverified,
        cancel: NativeCapabilitySupport::Unverified,
        tools: NativeCapabilitySupport::Unverified,
        isolated_context: NativeCapabilitySupport::Unverified,
        parallel_contexts: NativeCapabilitySupport::Unverified,
    }
}

/// One driver execution, in the shape the seam's transport asks for it.
///
/// The fields are the arguments the host driver already resolved — the
/// executable, the caller's params, the prompt, the bound native session, the
/// working directory, the resolved dispatch timeout and the two output bounds
/// — so an implementing Agent projects them onto its own execution entry point
/// without a second derivation of any of them.
pub struct DriverInvocation<'a> {
    /// The executable the driver process is launched from.
    pub executable: &'a str,
    /// The caller's adapter params.
    pub params: &'a Value,
    /// The prompt text, already selected from `text` or `message`.
    pub prompt: &'a str,
    /// The bound native session id, empty when the binding carries none.
    pub session_id: &'a str,
    /// The working directory: the call's own, else the configured one.
    pub cwd: Option<&'a Path>,
    /// The resolved dispatch timeout. Zero means unbounded.
    pub timeout_ms: u64,
    /// The stdout bound, when the transport sets one.
    pub max_stdout: Option<usize>,
    /// The stderr bound.
    pub max_stderr: usize,
}

/// One Agent protocol's reported failure, reduced to the facts the seam's
/// response mapping reads.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DriverFailure {
    /// The failure's stable code.
    pub code: &'static str,
    /// The lifecycle stage the failure was observed at.
    pub stage: &'static str,
    /// The redacted message.
    pub message: &'static str,
    /// The native identity the failure carries, when it carries one. The
    /// Agent's own projection chooses it — the thread id for one protocol, the
    /// session id for another — because which identity a failure names is a
    /// fact about that Agent's protocol.
    pub identity: Option<String>,
}

/// One driver execution's outcome.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DriverExecution {
    /// Whether the execution succeeded.
    pub ok: bool,
    /// The reported thread id.
    pub thread_id: String,
    /// The reported session id.
    pub session_id: String,
    /// The execution's output.
    pub output: String,
    /// The protocol failure, when the execution failed.
    pub failure: Option<DriverFailure>,
}

/// Execute one Agent's driver, exactly as that Agent's production path does.
pub type DriverExecute = fn(&DriverInvocation<'_>) -> DriverExecution;

/// Bind one Agent's protocol adapter over the host's transport.
///
/// The adapter is built here and wrapped into the work-context runtime in one
/// step, so no adapter type crosses this port: what an Agent's protocol IS
/// stays on the Agent's side of it.
pub type BindProtocol = fn(
    WorkContextConfig,
    Option<ConversationStore>,
    Arc<dyn AdapterTransport>,
) -> WorkContextRuntime;

/// One Agent, as the host-side seam reaches it.
///
/// The three facts are the whole of what the seam needs and the whole of what
/// it may not know by name: the inventory id lane control addresses, the
/// driver execution, and the builder for that Agent's protocol adapter.
#[derive(Clone, Copy)]
pub struct AgentProtocolRegistration {
    /// The inventory id lane control and the diagnostics address this Agent by.
    pub agent_id: &'static str,
    /// Build this Agent's protocol adapter over the host's transport.
    pub bind: BindProtocol,
    /// Execute this Agent's driver.
    pub execute: DriverExecute,
}

/// Dispatch one lane control operation against the host's own lane.
///
/// The port this reads is the composed Agent inventory port, and the payload of
/// `Err` is the lane's own error, which the seam does not surface: it reports
/// its own stable code for a failed dispatch.
pub type LaneDispatch = fn(&AgentTargetPort, &str, &Value) -> Result<Value, String>;

/// How the host reaches its own conversation lane.
///
/// The transport drives steer and cancel through the lane rather than through
/// an Agent's driver, so the lane is part of how the host reaches an Agent —
/// and it is the host's own machinery, not any Agent's protocol. The
/// composition supplies both members; this crate names no lane module and
/// builds no port value of its own.
#[derive(Clone, Copy)]
pub struct LaneReach {
    /// The composed Agent inventory port the lane reads.
    pub target_port: fn() -> AgentTargetPort,
    /// Dispatch one lane control operation.
    pub dispatch: LaneDispatch,
}
