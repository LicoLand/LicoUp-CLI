//! The host's composition of the work-context seam.
//!
//! `licoup-agent-drivers` owns the seam itself — the binding contract, the
//! adapter transport, the production host driver and the C01 effect port — and
//! declares the port it reads every Agent and the host's own lane through. This
//! module is the composition above it: it answers that port with the
//! implementations this host has today, and it keeps every former
//! `platform::work_context_ports` path reachable through the re-exports below.
//!
//! What it answers the port with, and where each answer travels next:
//!
//! * the Codex and Pi protocol adapters and their driver executions, in
//!   [`adapter`], which move to `licoup-agent-codex` and `licoup-agent-pi` when
//!   those crates exist;
//! * this host's own conversation lane, reached through
//!   [`crate::agent_port::dispatch`] and the Agent inventory port
//!   [`crate::target_port::agent_target_port`] composes, neither of
//!   which is any Agent's protocol.
//!
//! The hermetic work-context fixtures the integration suites drive are host
//! test material; they live here, and they travel to their Agents' crates with
//! the protocol halves they describe.

use std::sync::Arc;

use licoup_agent_runtime::work_context::{ProtocolFamily, WorkContextConfig, WorkContextRuntime};
use licoup_agent_targets::port::AgentTargetPort;
use licoup_conversation::ConversationStore;
use serde_json::Value;

// Every former path of the seam stays reachable here. The list is explicit
// rather than a glob so the composition's own `bind_persisted_work_context`
// below is the one this path names.
pub use licoup_agent_drivers::{
    AdapterCall, AdapterResponse, AdapterTransport, AgentProfileSource, CancelOutcome,
    ControlDelivery, ControlDisposition, CountingTransport, DeliveryOutcome, EffectBridge,
    EffectCapabilities, EffectCapability, EffectControl, EffectDelivery, EffectDispatch,
    EffectHandle, EffectInvocation, EffectObservation, EffectOperation, EffectRefusal,
    EffectSessionOwner, EffectState, EffectTurn, EffectUnknownReason, HostDriverTransport,
    ReconcileOutcome, RuntimeAgentProfile, Settlement, SteerOutcome, SubmitOutcome,
    UnavailableAdapterTransport, bind_host_work_context, boxed_work_context_port,
    effect_input_text, preregistered_work_context_port, unverified_snapshot,
};

mod adapter;
#[cfg(any(test, feature = "test-support"))]
mod codex;
#[cfg(any(test, feature = "test-support"))]
mod pi;

pub use adapter::{CodexAdapterProtocol, PiAdapterProtocol, pi_session_id_missing};

#[cfg(any(test, feature = "test-support"))]
pub use licoup_agent_runtime::work_context::{CapabilityProfile, ChildBinding, SessionPresence};

#[cfg(any(test, feature = "test-support"))]
pub use codex::{hermetic_codex, hermetic_codex_high, hermetic_codex_lost, hermetic_codex_low};
#[cfg(any(test, feature = "test-support"))]
pub use pi::{hermetic_pi, hermetic_pi_high, hermetic_pi_lost, hermetic_pi_low};

/// The production C01 effect bridge over this host's own lane and runtime Agent
/// profile registry.
///
/// The bridge itself is the seam's, and it names none of its implementations:
/// it takes the session writer claim, the Agent profile source and the effect
/// dispatch as injected surfaces. This composition is where this host answers
/// those three — the same two concrete types, in the same order, that the seam
/// used to compose for itself before it moved, so the production bridge is
/// unchanged and the seam is free of them.
pub fn production_effect_bridge(owner: Arc<dyn EffectSessionOwner>) -> EffectBridge {
    EffectBridge::new(
        owner,
        Arc::new(crate::platform::strategy_runtime::RuntimeRegistryAgentProfiles),
        Arc::new(crate::platform::strategy_runtime::LaneEffectDispatch),
    )
}

/// The Agent protocol registration one protocol family is reached through.
///
/// This is the one place the host names which Agent answers for a family. It
/// used to be the `ProtocolFamily` match inside the seam's own binding
/// function; the seam no longer carries it, and this composition does until
/// each Agent's crate registers itself.
fn registration(family: ProtocolFamily) -> licoup_agent_drivers::AgentProtocolRegistration {
    match family {
        ProtocolFamily::Codex => adapter::CODEX,
        ProtocolFamily::Pi => adapter::PI,
    }
}

/// This host's reach into its own conversation lane.
pub fn host_lane_reach() -> licoup_agent_drivers::LaneReach {
    licoup_agent_drivers::LaneReach {
        target_port: crate::target_port::agent_target_port,
        dispatch: dispatch_lane,
    }
}

/// One lane control operation, dispatched exactly as this host dispatched it
/// before the seam moved.
fn dispatch_lane(port: &AgentTargetPort, operation: &str, params: &Value) -> Result<Value, String> {
    let _ = port;
    crate::agent_port::dispatch(operation, params).map_err(|error| error.to_string())
}

/// The production driver transport for one Agent, over this host's lane.
pub fn host_driver_transport(family: ProtocolFamily) -> HostDriverTransport {
    HostDriverTransport::new(registration(family), host_lane_reach())
}

/// Bind one Agent's work context over the given transport.
pub fn bind_adapter_work_context(
    family: ProtocolFamily,
    config: WorkContextConfig,
    transport: Arc<dyn AdapterTransport>,
    store: Option<ConversationStore>,
) -> WorkContextRuntime {
    licoup_agent_drivers::bind_agent_protocol(registration(family), config, transport, store)
}

/// The production bind for one Agent, over this host's driver transport.
pub fn bind_persisted_work_context(
    store: ConversationStore,
    family: ProtocolFamily,
    _profile: licoup_agent_runtime::work_context::CapabilityProfile,
    config: WorkContextConfig,
) -> WorkContextRuntime {
    bind_adapter_work_context(
        family,
        config,
        Arc::new(host_driver_transport(family)),
        Some(store),
    )
}

#[cfg(any(test, feature = "test-support"))]
pub fn fixture_child_binding() -> ChildBinding {
    ChildBinding {
        child_conversation_id: "conversation:child".into(),
        membership_id: "membership:child-assistant".into(),
        source_task_id: "goal:source-task".into(),
        parent_conversation_id: "conversation:parent".into(),
    }
}

#[cfg(any(test, feature = "test-support"))]
pub fn fixture_config(knowledge_injected: bool) -> WorkContextConfig {
    WorkContextConfig::child(fixture_child_binding()).with_knowledge_injected(knowledge_injected)
}

#[cfg(any(test, feature = "test-support"))]
pub fn lost_session_protocol(
    family_codex: bool,
) -> licoup_agent_runtime::work_context::HermeticProtocol {
    let protocol = if family_codex {
        licoup_agent_runtime::work_context::HermeticProtocol::codex(CapabilityProfile::High)
    } else {
        licoup_agent_runtime::work_context::HermeticProtocol::pi(CapabilityProfile::High)
    };
    protocol.with_presence(SessionPresence::Lost)
}
