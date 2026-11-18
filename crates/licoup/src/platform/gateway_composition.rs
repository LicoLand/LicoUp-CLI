//! Gateway sidecar composition: this host's answers for the runtime ports.
//!
//! The Private Gateway process (`licoup gateway run`) never links conversation or
//! custody internals; it consumes the credential handoff lease and the
//! readiness callback. The Telegram channel process (`lico-telegram-channel`)
//! consumes the agent-execution port's lane answers. This composition supplies
//! each port once per process, so the channel reaches every Agent through the
//! same port the host's own callers use.

use anyhow::Result;
use credential_lease::{CustodyPort, port};
use licoup_private_gateway::credential::{
    GatewayCredentialHandoff, GatewayCredentialLease, LlmApiKeyProvider,
};
use licoup_private_gateway::ports::readiness;
use licoup_public_gateway::lane::{self, ChannelError, LanePort};
use licoup_public_gateway::telegram;
use serde_json::Value;
use std::sync::Arc;

use super::llm_api_key_vault::PlatformLlmApiKeyVault;

struct ProductionGatewayVault {
    vault: PlatformLlmApiKeyVault,
}

impl CustodyPort<LlmApiKeyProvider> for ProductionGatewayVault {
    fn lease_from_handoff(
        &self,
        handoff: GatewayCredentialHandoff,
    ) -> Result<GatewayCredentialLease> {
        self.vault.gateway_lease_from_handoff(handoff)
    }
}

/// Bound a lane answer's failure for the contract crossing: lane errors are
/// code-shaped text; the channel only ever surfaces the bounded code.
fn lane_error(error: anyhow::Error) -> ChannelError {
    const MAX_MESSAGE_CHARS: usize = 240;
    let text = error.to_string();
    ChannelError::new(
        "gateway_lane_failed",
        text.chars().take(MAX_MESSAGE_CHARS).collect::<String>(),
    )
}

fn lane_scan_targets() -> Result<Value, ChannelError> {
    crate::agent_port::scan_targets().map_err(lane_error)
}

fn lane_conversation_list(params: &Value) -> Result<Value, ChannelError> {
    crate::agent_port::conversation_list(params).map_err(lane_error)
}

fn lane_open_or_resume(params: &Value) -> Result<Value, ChannelError> {
    crate::agent_port::open_or_resume(params).map_err(lane_error)
}

/// The channel's lane dispatch, answered by the agent-execution port.
///
/// The channel names no lane module: every operation it asks for — send,
/// steer, cancel, cleanup — arrives here and travels the same port the rest of
/// the host's Agent callers use.
fn lane_dispatch(operation: &str, params: &Value) -> Result<Value, ChannelError> {
    crate::agent_port::dispatch(operation, params)
        .map_err(|error| ChannelError::new("gateway_lane_dispatch_failed", error.to_string()))
}

/// Install every port the Private Gateway process consumes.
pub fn install() -> Result<(), &'static str> {
    readiness::install(super::runtime_adapters::reload_conversation_readiness_document)?;
    let vault = PlatformLlmApiKeyVault::production().map_err(|_| "gateway_vault_unavailable")?;
    port::install::<LlmApiKeyProvider, _>(Arc::new(ProductionGatewayVault { vault }))?;
    Ok(())
}

/// Install the ports the Telegram channel process consumes: bot-token
/// custody from the product store and the agent-execution lane answers.
pub fn install_channel() -> Result<(), &'static str> {
    telegram::install_custody()?;
    lane::install(LanePort {
        scan_targets: lane_scan_targets,
        conversation_list: lane_conversation_list,
        open_or_resume: lane_open_or_resume,
        dispatch: lane_dispatch,
    })
}

/// Install the readiness callback alone; the client that manages a gateway
/// still applies and validates pushed readiness without dispatching lanes.
pub fn install_readiness() -> Result<(), &'static str> {
    readiness::install(super::runtime_adapters::reload_conversation_readiness_document)
}
