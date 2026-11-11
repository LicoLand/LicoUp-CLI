//! The agent-execution port every caller above the platform layer reaches.
//!
//! One port owns execution, cancellation, steering, session resume and history
//! for every Agent, and the work-context binding the continuity host drives.
//! `licoup-agent-drivers` declares that port and owns the machines behind it;
//! this module is the composition's entry to it, so a caller names the port and
//! never the lane, the registry or an Agent's own module.
//!
//! It lives at the crate root for the same reason [`crate::target_port`] does:
//! it joins the layers once, above both, and neither layer has to know the
//! other. What answers the port today is this host's own conversation lane,
//! [`crate::platform::conversation_lane`], which is the host's control plane
//! rather than any Agent's protocol — the same lane the driver port's
//! `LaneReach` answers with, read here through one entry. When a caller needs
//! an Agent's own protocol half, it asks the driver port's per-Agent
//! registration; it never names the Agent's module.
//!
//! Every entry here is fail-closed in the same way the moved crate's ports are:
//! a host that never composed an answer reports a refusal rather than
//! inventing an effect.

use std::sync::Arc;

use licoup_agent_drivers::AdapterTransport;
use licoup_agent_drivers::runtime_adapters::RuntimeAdapterError;
use licoup_agent_runtime::work_context::{
    HermeticProtocol, ProtocolFamily, WorkContextConfig, WorkContextRuntime,
};
use licoup_conversation::ConversationStore;
use serde_json::{Value, json};

/// Install this host's answers for the driver ports, once.
///
/// The production entry points install for themselves; a caller that reaches
/// the port from a binary whose own startup did not is the reason this is
/// public. The first installation wins, so a running turn cannot have the
/// answers under it replaced.
pub fn install() {
    crate::platform::runtime_adapters::install();
}

/// Dispatch one Agent lane operation through the port.
///
/// The operation vocabulary is the port's own — `send`, `stream`, `steer`,
/// `cancel`, `history`, `capabilities`, `open`, `cleanup` — and an operation
/// outside it fails closed. The typed refusal is the port's, so a caller
/// projects it into the client's error chain exactly as it did before the
/// entry moved here.
pub fn dispatch(operation: &str, params: &Value) -> Result<Value, RuntimeAdapterError> {
    install();
    crate::platform::conversation_lane::dispatch_lane_operation(operation, params)
}

/// Send one turn and settle it inside the call.
pub fn send(params: &Value) -> Result<Value, RuntimeAdapterError> {
    dispatch("send", params)
}

/// Steer one in-flight turn through its Agent's native control channel.
pub fn steer(params: &Value) -> Result<Value, RuntimeAdapterError> {
    dispatch("steer", params)
}

/// Cancel one in-flight turn through its Agent's native control channel.
pub fn cancel(params: &Value) -> Result<Value, RuntimeAdapterError> {
    dispatch("cancel", params)
}

/// End one Agent's persisted conversation.
pub fn cleanup(params: &Value) -> Result<Value, RuntimeAdapterError> {
    dispatch("cleanup", params)
}

/// Read one Agent's process-local history page.
#[cfg(test)]
pub fn history(params: &Value) -> Result<Value, RuntimeAdapterError> {
    dispatch("history", params)
}

/// Open or resume one conversation session.
///
/// Session binding is a read whose failure a caller diagnoses rather than a
/// dispatch whose failure it classifies, so the lane's own reason travels
/// rather than being collapsed into a dispatch code.
pub fn open_or_resume(params: &Value) -> anyhow::Result<Value> {
    crate::platform::conversation_lane::open_or_resume(params)
}

/// Read one Agent's capability matrix, inventory plus static family facts.
///
/// It is the admission answer the workflow runtime asks before it binds an
/// actor slot: an Agent whose lane reports no capabilities is not admitted, and
/// a refusal is reported as an admission failure rather than as an empty
/// matrix.
pub fn capabilities(agent_id: &str) -> Result<Value, RuntimeAdapterError> {
    dispatch("capabilities", &json!({ "agent": agent_id }))
}

/// Verified Agent discovery, for a channel's own admission.
///
/// Discovery is not admission: a caller still applies its own readiness and
/// executable gate, exactly as the lane's own scan reports it.
pub fn scan_targets() -> anyhow::Result<Value> {
    crate::platform::conversation_lane::lane_target_scan()
}

/// Bounded conversation listing for one Agent.
pub fn conversation_list(params: &Value) -> anyhow::Result<Value> {
    crate::platform::conversation_lane::lane_conversation_list(params)
}

/// The production driver transport for one Agent protocol family.
///
/// The transport is the seam's, and which Agent answers a family is the
/// composition's answer at the port — this entry names neither.
pub fn work_context_transport(family: ProtocolFamily) -> Arc<dyn AdapterTransport> {
    Arc::new(crate::platform::work_context_ports::host_driver_transport(
        family,
    ))
}

/// Bind one Agent's work context over an injected transport.
pub fn bind_work_context(
    family: ProtocolFamily,
    config: WorkContextConfig,
    transport: Arc<dyn AdapterTransport>,
    store: Option<ConversationStore>,
) -> WorkContextRuntime {
    crate::platform::work_context_ports::bind_adapter_work_context(family, config, transport, store)
}

/// Bind the hermetic work context a continuity suite drives.
///
/// It is the seam's own hermetic protocol rather than any Agent's, and it is
/// reached here so a continuity suite composes through the same entry the
/// production binding does.
pub fn bind_hermetic_work_context(
    protocol: HermeticProtocol,
    config: WorkContextConfig,
) -> WorkContextRuntime {
    crate::platform::work_context_ports::bind_host_work_context(protocol, config)
}
