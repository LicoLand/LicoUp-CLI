//! Agent-agnostic Subagent MCP status/plan/install dispatcher.

use crate::domain::integration_state::IntegrationState;
use crate::platform::{
    antigravity_subagent_mcp_manager, claude_code_subagent_mcp_manager, cursor_subagent_mcp_manager,
};
use std::path::Path;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SubagentMcpEnsureState {
    Ready,
    Missing,
    Unavailable,
    Unsupported,
}

impl SubagentMcpEnsureState {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Ready => "ready",
            Self::Missing => "missing",
            Self::Unavailable => "unavailable",
            Self::Unsupported => "unsupported",
        }
    }

    pub const fn ready(self) -> bool {
        matches!(self, Self::Ready)
    }
}

#[derive(Clone, Debug)]
pub struct SubagentMcpEnsurePlan {
    pub agent_id: String,
    pub digest: String,
    pub plugin_version: String,
    pub source: String,
    pub release: String,
    pub requires_confirmation: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SubagentMcpEnsureError {
    Unsupported,
    ApprovalRequired,
    ApprovalMismatch,
    ApprovalConsumed,
    InvalidBinary,
    ConfigAmbiguous,
    ConfigPathUnsupported,
    InstallFailed,
    ProcessUnavailable,
}

impl SubagentMcpEnsureError {
    pub const fn code(self) -> &'static str {
        match self {
            Self::Unsupported => "subagent_mcp_unsupported",
            Self::ApprovalRequired => "subagent_mcp_approval_required",
            Self::ApprovalMismatch => "subagent_mcp_approval_mismatch",
            Self::ApprovalConsumed => "subagent_mcp_approval_consumed",
            Self::InvalidBinary => "subagent_mcp_binary_invalid",
            Self::ConfigAmbiguous => "subagent_mcp_config_ambiguous",
            Self::ConfigPathUnsupported => "subagent_mcp_config_path_unsupported",
            Self::InstallFailed => "subagent_mcp_install_failed",
            Self::ProcessUnavailable => "subagent_mcp_process_unavailable",
        }
    }
}

pub fn status(
    agent_id: &str,
    binary_path: Option<&Path>,
    mcp_binary_path: Option<&Path>,
    config_path: Option<&Path>,
) -> SubagentMcpEnsureState {
    match agent_id {
        // This client installs no caller integration into Codex: the plugin
        // that carries it belongs to Codex's own marketplace, and the adapter
        // package owns the Codex side of that registration. An unsupported
        // answer is the truthful one, not a status this client cannot observe.
        "codex" => SubagentMcpEnsureState::Unsupported,
        "antigravity" => {
            if binary_path.is_none() {
                return SubagentMcpEnsureState::Unavailable;
            }
            let owned = mcp_binary_path
                .map(Path::to_path_buf)
                .or_else(antigravity_subagent_mcp_manager::default_mcp_binary_path);
            let Some(path) = owned else {
                return SubagentMcpEnsureState::Unavailable;
            };
            let state = match config_path {
                Some(explicit) => {
                    antigravity_subagent_mcp_manager::status_with_config_path(&path, explicit)
                }
                None => antigravity_subagent_mcp_manager::status(&path),
            };
            match state {
                IntegrationState::Ready => SubagentMcpEnsureState::Ready,
                IntegrationState::Missing => SubagentMcpEnsureState::Missing,
                IntegrationState::Unavailable => SubagentMcpEnsureState::Unavailable,
            }
        }
        "cursor" | "claude-code" => {
            if binary_path.is_none() {
                return SubagentMcpEnsureState::Unavailable;
            }
            let owned = mcp_binary_path
                .map(Path::to_path_buf)
                .or_else(antigravity_subagent_mcp_manager::default_mcp_binary_path);
            let Some(path) = owned else {
                return SubagentMcpEnsureState::Unavailable;
            };
            let state = match (agent_id, config_path) {
                ("cursor", Some(explicit)) => {
                    cursor_subagent_mcp_manager::status_with_config_path(&path, explicit)
                }
                ("cursor", None) => cursor_subagent_mcp_manager::status(&path),
                (_, Some(explicit)) => {
                    claude_code_subagent_mcp_manager::status_with_config_path(&path, explicit)
                }
                _ => claude_code_subagent_mcp_manager::status(&path),
            };
            match state {
                IntegrationState::Ready => SubagentMcpEnsureState::Ready,
                IntegrationState::Missing => SubagentMcpEnsureState::Missing,
                IntegrationState::Unavailable => SubagentMcpEnsureState::Unavailable,
            }
        }
        _ => SubagentMcpEnsureState::Unsupported,
    }
}

pub fn plan(
    agent_id: &str,
    binary_path: Option<&Path>,
    mcp_binary_path: Option<&Path>,
    config_path: Option<&Path>,
) -> Result<SubagentMcpEnsurePlan, SubagentMcpEnsureError> {
    match agent_id {
        "codex" => Err(SubagentMcpEnsureError::Unsupported),
        "antigravity" => {
            if binary_path.is_none_or(|path| !path.is_file()) {
                return Err(SubagentMcpEnsureError::InvalidBinary);
            }
            let path = mcp_binary_path
                .map(Path::to_path_buf)
                .or_else(antigravity_subagent_mcp_manager::default_mcp_binary_path)
                .ok_or(SubagentMcpEnsureError::InvalidBinary)?;
            let plan = match config_path {
                Some(explicit) => {
                    antigravity_subagent_mcp_manager::AntigravitySubagentMcpPlan::prepare_with_config_path(
                        "antigravity",
                        &path,
                        explicit,
                    )
                }
                None => antigravity_subagent_mcp_manager::AntigravitySubagentMcpPlan::prepare(
                    "antigravity",
                    &path,
                ),
            }
            .map_err(map_antigravity_error)?;
            Ok(SubagentMcpEnsurePlan {
                agent_id: agent_id.to_owned(),
                digest: plan.digest().to_owned(),
                plugin_version:
                    antigravity_subagent_mcp_manager::AntigravitySubagentMcpPlan::version()
                        .to_owned(),
                source: antigravity_subagent_mcp_manager::AntigravitySubagentMcpPlan::source()
                    .to_owned(),
                release: "packaged".to_owned(),
                requires_confirmation: true,
            })
        }
        "cursor" | "claude-code" => {
            if binary_path.is_none_or(|path| !path.is_file()) {
                return Err(SubagentMcpEnsureError::InvalidBinary);
            }
            let path = mcp_binary_path
                .map(Path::to_path_buf)
                .or_else(antigravity_subagent_mcp_manager::default_mcp_binary_path)
                .ok_or(SubagentMcpEnsureError::InvalidBinary)?;
            let plan = match (agent_id, config_path) {
                ("cursor", Some(explicit)) => {
                    cursor_subagent_mcp_manager::plan_with_config_path(&path, explicit)
                }
                ("cursor", None) => cursor_subagent_mcp_manager::plan(&path),
                (_, Some(explicit)) => {
                    claude_code_subagent_mcp_manager::plan_with_config_path(&path, explicit)
                }
                _ => claude_code_subagent_mcp_manager::plan(&path),
            }
            .map_err(map_registration_error)?;
            Ok(SubagentMcpEnsurePlan {
                agent_id: agent_id.to_owned(),
                digest: plan.digest().to_owned(),
                plugin_version: "0.2.0".to_owned(),
                source: "LicoUp managed user MCP registration".to_owned(),
                release: "packaged".to_owned(),
                requires_confirmation: true,
            })
        }
        _ => Err(SubagentMcpEnsureError::Unsupported),
    }
}

pub fn install(
    agent_id: &str,
    binary_path: Option<&Path>,
    mcp_binary_path: Option<&Path>,
    confirmation: &str,
    confirmed: bool,
    config_path: Option<&Path>,
) -> Result<(bool, bool), SubagentMcpEnsureError> {
    match agent_id {
        "codex" => {
            let _ = (binary_path, confirmation, confirmed);
            Err(SubagentMcpEnsureError::Unsupported)
        }
        "antigravity" => {
            if binary_path.is_none_or(|path| !path.is_file()) {
                return Err(SubagentMcpEnsureError::InvalidBinary);
            }
            let path = mcp_binary_path
                .map(Path::to_path_buf)
                .or_else(antigravity_subagent_mcp_manager::default_mcp_binary_path)
                .ok_or(SubagentMcpEnsureError::InvalidBinary)?;
            let plan = match config_path {
                Some(explicit) => {
                    antigravity_subagent_mcp_manager::AntigravitySubagentMcpPlan::prepare_with_config_path(
                        "antigravity",
                        &path,
                        explicit,
                    )
                }
                None => antigravity_subagent_mcp_manager::AntigravitySubagentMcpPlan::prepare(
                    "antigravity",
                    &path,
                ),
            }
            .map_err(map_antigravity_error)?;
            let mut permit = plan
                .approve(confirmed, confirmation)
                .map_err(map_antigravity_error)?;
            let receipt = antigravity_subagent_mcp_manager::install(&plan, &mut permit)
                .map_err(map_antigravity_error)?;
            Ok((
                receipt.installed,
                receipt.plugin_ready_for_new_conversations,
            ))
        }
        "cursor" | "claude-code" => {
            if binary_path.is_none_or(|path| !path.is_file()) {
                return Err(SubagentMcpEnsureError::InvalidBinary);
            }
            let path = mcp_binary_path
                .map(Path::to_path_buf)
                .or_else(antigravity_subagent_mcp_manager::default_mcp_binary_path)
                .ok_or(SubagentMcpEnsureError::InvalidBinary)?;
            let (plan, install_cursor) = match (agent_id, config_path) {
                ("cursor", Some(explicit)) => (
                    cursor_subagent_mcp_manager::plan_with_config_path(&path, explicit)
                        .map_err(map_registration_error)?,
                    true,
                ),
                ("cursor", None) => (
                    cursor_subagent_mcp_manager::plan(&path).map_err(map_registration_error)?,
                    true,
                ),
                (_, Some(explicit)) => (
                    claude_code_subagent_mcp_manager::plan_with_config_path(&path, explicit)
                        .map_err(map_registration_error)?,
                    false,
                ),
                _ => (
                    claude_code_subagent_mcp_manager::plan(&path)
                        .map_err(map_registration_error)?,
                    false,
                ),
            };
            let mut permit = plan
                .approve(confirmed, confirmation)
                .map_err(map_registration_error)?;
            if install_cursor {
                cursor_subagent_mcp_manager::install(&plan, &mut permit)
                    .map_err(map_registration_error)?;
            } else {
                claude_code_subagent_mcp_manager::install(&plan, &mut permit)
                    .map_err(map_registration_error)?;
            }
            Ok((true, true))
        }
        _ => Err(SubagentMcpEnsureError::Unsupported),
    }
}

fn map_antigravity_error(
    error: antigravity_subagent_mcp_manager::AntigravitySubagentMcpError,
) -> SubagentMcpEnsureError {
    use antigravity_subagent_mcp_manager::AntigravitySubagentMcpError::*;
    match error {
        NotAntigravity => SubagentMcpEnsureError::Unsupported,
        InvalidMcpBinary => SubagentMcpEnsureError::InvalidBinary,
        ApprovalRequired => SubagentMcpEnsureError::ApprovalRequired,
        ApprovalMismatch => SubagentMcpEnsureError::ApprovalMismatch,
        ApprovalConsumed => SubagentMcpEnsureError::ApprovalConsumed,
        ConfigAmbiguous => SubagentMcpEnsureError::ConfigAmbiguous,
        ConfigPathUnsupported => SubagentMcpEnsureError::ConfigPathUnsupported,
        ConfigUnavailable | InstallFailed => SubagentMcpEnsureError::InstallFailed,
    }
}

fn map_registration_error(
    error: cursor_subagent_mcp_manager::CursorSubagentMcpError,
) -> SubagentMcpEnsureError {
    use crate::platform::provider_mcp_registration::RegistrationError::*;
    match error {
        InvalidConnector => SubagentMcpEnsureError::InvalidBinary,
        ApprovalRequired => SubagentMcpEnsureError::ApprovalRequired,
        ApprovalMismatch | ConfigChanged => SubagentMcpEnsureError::ApprovalMismatch,
        ApprovalConsumed => SubagentMcpEnsureError::ApprovalConsumed,
        ConfigAmbiguous | DuplicateConnectorEntry => SubagentMcpEnsureError::ConfigAmbiguous,
        ConfigPathUnsupported => SubagentMcpEnsureError::ConfigPathUnsupported,
        ConfigUnavailable | OwnedEntryAmbiguous | WriteFailed => {
            SubagentMcpEnsureError::InstallFailed
        }
    }
}
