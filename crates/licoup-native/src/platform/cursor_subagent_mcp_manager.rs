//! Cursor user MCP registration through the common digest-bound manager.

use super::provider_mcp_registration::{
    ProviderConfigKind, RegistrationError, RegistrationPermit, RegistrationPlan,
};
use crate::domain::integration_state::IntegrationState;
use std::path::Path;

pub type CursorSubagentMcpError = RegistrationError;
pub type CursorSubagentMcpPlan = RegistrationPlan;
pub type CursorSubagentMcpPermit = RegistrationPermit;

pub fn plan(connector: &Path) -> Result<CursorSubagentMcpPlan, CursorSubagentMcpError> {
    RegistrationPlan::prepare(ProviderConfigKind::Cursor, connector)
}

/// Plan against one explicitly discovered Cursor config path. Only the exact
/// reviewed candidate is admitted and it is bound into the approval digest.
pub fn plan_with_config_path(
    connector: &Path,
    config_path: &Path,
) -> Result<CursorSubagentMcpPlan, CursorSubagentMcpError> {
    RegistrationPlan::prepare_with_config_path(ProviderConfigKind::Cursor, connector, config_path)
}

pub fn status(connector: &Path) -> IntegrationState {
    match super::provider_mcp_registration::status(ProviderConfigKind::Cursor, connector) {
        Ok(true) => IntegrationState::Ready,
        Ok(false) => IntegrationState::Missing,
        Err(_) => IntegrationState::Unavailable,
    }
}

/// Read-only readiness probe at one explicitly discovered config path.
pub fn status_with_config_path(connector: &Path, config_path: &Path) -> IntegrationState {
    match super::provider_mcp_registration::status_with_config_path(
        ProviderConfigKind::Cursor,
        connector,
        config_path,
    ) {
        Ok(true) => IntegrationState::Ready,
        Ok(false) => IntegrationState::Missing,
        Err(_) => IntegrationState::Unavailable,
    }
}

pub fn install(
    plan: &CursorSubagentMcpPlan,
    permit: &mut CursorSubagentMcpPermit,
) -> Result<(), CursorSubagentMcpError> {
    super::provider_mcp_registration::install(plan, permit)
}

pub fn remove(
    plan: &CursorSubagentMcpPlan,
    permit: &mut CursorSubagentMcpPermit,
) -> Result<(), CursorSubagentMcpError> {
    super::provider_mcp_registration::remove(plan, permit)
}
