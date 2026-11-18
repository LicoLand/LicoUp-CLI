//! Claude Code user MCP registration through the common digest-bound manager.

use super::provider_mcp_registration::{
    ProviderConfigKind, RegistrationError, RegistrationPermit, RegistrationPlan,
};
use crate::domain::integration_state::IntegrationState;
use std::path::Path;

pub type ClaudeCodeSubagentMcpError = RegistrationError;
pub type ClaudeCodeSubagentMcpPlan = RegistrationPlan;
pub type ClaudeCodeSubagentMcpPermit = RegistrationPermit;

pub fn plan(connector: &Path) -> Result<ClaudeCodeSubagentMcpPlan, ClaudeCodeSubagentMcpError> {
    RegistrationPlan::prepare(ProviderConfigKind::ClaudeCode, connector)
}

/// Plan against one explicitly discovered Claude Code config path. Only the
/// exact reviewed candidate is admitted and it is bound into the approval
/// digest.
pub fn plan_with_config_path(
    connector: &Path,
    config_path: &Path,
) -> Result<ClaudeCodeSubagentMcpPlan, ClaudeCodeSubagentMcpError> {
    RegistrationPlan::prepare_with_config_path(
        ProviderConfigKind::ClaudeCode,
        connector,
        config_path,
    )
}

pub fn status(connector: &Path) -> IntegrationState {
    match super::provider_mcp_registration::status(ProviderConfigKind::ClaudeCode, connector) {
        Ok(true) => IntegrationState::Ready,
        Ok(false) => IntegrationState::Missing,
        Err(_) => IntegrationState::Unavailable,
    }
}

/// Read-only readiness probe at one explicitly discovered config path.
pub fn status_with_config_path(connector: &Path, config_path: &Path) -> IntegrationState {
    match super::provider_mcp_registration::status_with_config_path(
        ProviderConfigKind::ClaudeCode,
        connector,
        config_path,
    ) {
        Ok(true) => IntegrationState::Ready,
        Ok(false) => IntegrationState::Missing,
        Err(_) => IntegrationState::Unavailable,
    }
}

pub fn install(
    plan: &ClaudeCodeSubagentMcpPlan,
    permit: &mut ClaudeCodeSubagentMcpPermit,
) -> Result<(), ClaudeCodeSubagentMcpError> {
    super::provider_mcp_registration::install(plan, permit)
}

pub fn remove(
    plan: &ClaudeCodeSubagentMcpPlan,
    permit: &mut ClaudeCodeSubagentMcpPermit,
) -> Result<(), ClaudeCodeSubagentMcpError> {
    super::provider_mcp_registration::remove(plan, permit)
}
