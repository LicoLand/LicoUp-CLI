//! Gateway sidecar composition: this host's answers for the runtime ports.
//!
//! The Private Gateway process (`lico-gateway`) never links conversation or
//! custody internals; it consumes the credential handoff lease and the
//! readiness callback. The Telegram channel process (`lico-telegram-channel`)
//! consumes the agent-execution port's lane answers. This composition supplies
//! each port once per process, so the channel reaches every Agent through the
//! same port the host's own callers use.

use anyhow::{Result, anyhow};
use licoup_private_gateway::ports::readiness;
use licoup_public_gateway::lane::{self, LanePort};
use licoup_vault::api_key::{GatewayCredentialHandoff, GatewayCredentialLease};
use licoup_vault::port::{self, GatewayVaultPort};
use serde_json::Value;
use std::sync::Arc;

use super::llm_api_key_vault::PlatformLlmApiKeyVault;

struct ProductionGatewayVault {
    vault: PlatformLlmApiKeyVault,
}

impl GatewayVaultPort for ProductionGatewayVault {
    fn lease_from_handoff(
        &self,
        handoff: GatewayCredentialHandoff,
    ) -> Result<GatewayCredentialLease> {
        self.vault.gateway_lease_from_handoff(handoff)
    }
}

/// The channel's lane dispatch, answered by the agent-execution port.
///
/// The channel names no lane module: every operation it asks for — send,
/// steer, cancel, cleanup — arrives here and travels the same port the rest of
/// the host's Agent callers use.
fn dispatch_lane(operation: &str, params: &Value) -> Result<Value> {
    crate::agent_port::dispatch(operation, params)
        .map_err(|error| anyhow!("gateway_lane_dispatch_failed:{error}"))
}

/// Install every port the Private Gateway process consumes.
pub fn install() -> Result<(), &'static str> {
    readiness::install(super::runtime_adapters::reload_conversation_readiness_document)?;
    let vault = PlatformLlmApiKeyVault::production().map_err(|_| "gateway_vault_unavailable")?;
    port::install(Arc::new(ProductionGatewayVault { vault }))?;
    Ok(())
}

/// Install the lane port the Telegram channel process consumes.
pub fn install_channel() -> Result<(), &'static str> {
    lane::install(LanePort {
        scan_targets: crate::agent_port::scan_targets,
        conversation_list: crate::agent_port::conversation_list,
        open_or_resume: crate::agent_port::open_or_resume,
        dispatch: dispatch_lane,
    })
}

/// Install the readiness callback alone; the client that manages a gateway
/// still applies and validates pushed readiness without dispatching lanes.
pub fn install_readiness() -> Result<(), &'static str> {
    readiness::install(super::runtime_adapters::reload_conversation_readiness_document)
}
