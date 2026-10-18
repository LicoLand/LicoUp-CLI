pub mod adapter;
pub mod artifact;
pub mod dispatch;
pub mod error;
pub mod live_status;
pub mod model;
pub mod normalization;
pub mod params;
pub mod port;
pub mod probe;
// The protocol-selector vocabulary is a re-export of `licoup-agent-runtime`,
// not a behaviour of this crate, and the host's own lane reads it in its test
// build. It is therefore gated on a FEATURE rather than on `cfg(test)`: a
// `cfg(test)` seam is compiled out of every crate that is a dependency, and
// `licoup-native` is exactly such a consumer.
#[cfg(any(test, feature = "test-support"))]
pub mod protocol_selector {
    pub use licoup_agent_runtime::protocol_selector::*;
}
pub mod registry;
pub mod root_cause;
pub mod subagent_mesh;

// Public host-neutral L4/L5 contracts. Concrete drivers remain composed in
// this native host until their individually owned modules can move without
// crossing concurrent ownership boundaries.
pub use licoup_agent_runtime::{PersistentTurnRuntime, RuntimeDriver, RuntimeDriverRegistry};

pub const RUNTIME_SCHEMA_VERSION: u32 = 3;
const DEFAULT_MAX_STDERR_BYTES: usize = 512 * 1024;
// Keep the native dispatch clamp identical to the public subagent MCP bound.
// A lower hidden clamp turns an accepted budget into a misleading early
// output-limit failure and prevents exact native-session continuation.
const MAX_OUTPUT_BYTES: usize = 64 * 1024 * 1024;

/// Dispatch implementations must stay in one-to-one correspondence with the
/// canonical target-adapters packaging registry. This is implementation
/// dispatch, not a readiness claim; release readiness is reduced separately.
pub const PACKAGED_RUNTIME_ADAPTER_IDS: &[&str] = &[
    "openclaw",
    "claude-code",
    "codex",
    "antigravity",
    "opencode",
    "copilot",
    "kilo-code",
    "cursor",
    "hermes",
    "kimi-code",
    "pi",
    "lico-agent",
    "deepseek-harness",
];

// Every name the former `platform::runtime_adapters` path exposed to the host
// is exposed here at the widest visibility. The host's own callers are in
// another crate now, and a `pub(crate)` item that was reachable inside one
// crate is not reachable from the crate above it.
pub use adapter::{RuntimeAdapter, adapter_for_agent_public, text_param_public};
pub use dispatch::send_message;

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum RuntimeLane {
    Dedicated(RuntimeAdapter),
    GenericCli(licoup_agent_targets::domain::cli_registration::CliRegistration),
}

pub fn runtime_lane_for_agent(agent_id: &str) -> Option<RuntimeLane> {
    if let Some(adapter) = adapter::adapter_for_agent(agent_id) {
        return Some(RuntimeLane::Dedicated(adapter));
    }
    licoup_agent_targets::domain::cli_registration::registration_for(agent_id)
        .map(RuntimeLane::GenericCli)
}

pub fn has_runtime_lane(agent_id: &str) -> bool {
    runtime_lane_for_agent(agent_id).is_some()
}
pub use error::RuntimeAdapterError;
pub use params::{
    MAX_IMAGE_ATTACHMENT_BYTES_PER_FILE, MAX_IMAGE_ATTACHMENT_BYTES_TOTAL, MAX_IMAGE_ATTACHMENTS,
    attachment_media_type_supported,
};
pub use probe::probe_runtime_driver;
pub use registry::{
    adapter_management_catalog, inventory_capability_matrix, native_capabilities_for_agent,
    runtime_driver_profile,
};
pub use registry::{
    reload_conversation_readiness_document, reload_conversation_readiness_from_path,
};
pub use subagent_mesh::{
    apply_mcp_runtime_root, apply_subagent_caller_context, production_subagent_registry,
};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct GeneratedInstructionDelivery {
    pub text: String,
    pub field: Option<&'static str>,
    pub guidance: Option<String>,
}

/// Compose generated guidance for one explicitly declared adapter policy.
/// The canonical Event text is supplied separately and remains unchanged.
pub fn compose_generated_instruction_delivery(
    agent_id: &str,
    user_text: &str,
    guidance: Option<&str>,
) -> Result<GeneratedInstructionDelivery, &'static str> {
    let Some(guidance) = guidance else {
        return Ok(GeneratedInstructionDelivery {
            text: user_text.to_owned(),
            field: None,
            guidance: None,
        });
    };
    let Some(adapter) = adapter_for_agent_public(agent_id) else {
        if licoup_agent_targets::domain::cli_registration::registration_for(agent_id).is_some() {
            return Ok(GeneratedInstructionDelivery {
                text: format!("{guidance}\n\n{user_text}"),
                field: None,
                guidance: None,
            });
        }
        // An Agent with no declared instruction policy is delivered the
        // composed text unchanged. This is a behaviour seam, so it is gated on
        // the feature rather than on `cfg(test)`: the host's own conversation
        // suites drive it, and the host links this crate as a dependency, so a
        // `cfg(test)` arm is compiled out of exactly the build that reads it —
        // which is how this arm once turned two host suite failures into
        // `runtime_instruction_policy_undeclared`.
        #[cfg(any(test, feature = "test-support"))]
        {
            return Ok(GeneratedInstructionDelivery {
                text: format!("{guidance}\n\n{user_text}"),
                field: None,
                guidance: None,
            });
        }
        #[cfg(not(any(test, feature = "test-support")))]
        return Err("runtime_instruction_policy_undeclared");
    };
    let policy = match adapter {
        RuntimeAdapter::Codex => {
            licoup_agent_runtime::InstructionPolicy::NativeDeveloperInstructions
        }
        RuntimeAdapter::Cursor | RuntimeAdapter::Antigravity | RuntimeAdapter::DeepSeekHarness => {
            licoup_agent_runtime::InstructionPolicy::OrdinaryWirePrefix
        }
        RuntimeAdapter::ClaudeCode
        | RuntimeAdapter::Copilot
        | RuntimeAdapter::Hermes
        | RuntimeAdapter::KiloCode
        | RuntimeAdapter::KimiCode
        | RuntimeAdapter::OpenClaw
        | RuntimeAdapter::OpenCode => {
            licoup_agent_runtime::InstructionPolicy::NativePrivateInstructions
        }
        RuntimeAdapter::Pi | RuntimeAdapter::LicoAgent => {
            return Err("runtime_instruction_policy_unavailable");
        }
    };
    Ok(match policy {
        licoup_agent_runtime::InstructionPolicy::NativeDeveloperInstructions => {
            GeneratedInstructionDelivery {
                text: user_text.to_owned(),
                field: Some("developerInstructions"),
                guidance: Some(guidance.to_owned()),
            }
        }
        licoup_agent_runtime::InstructionPolicy::NativePrivateInstructions => {
            GeneratedInstructionDelivery {
                text: user_text.to_owned(),
                field: Some("privateInstructions"),
                guidance: Some(guidance.to_owned()),
            }
        }
        licoup_agent_runtime::InstructionPolicy::OrdinaryWirePrefix => {
            GeneratedInstructionDelivery {
                text: format!("{guidance}\n\n{user_text}"),
                field: None,
                guidance: None,
            }
        }
    })
}
