//! The host's own lanes, in the shape the moved crate reads them through its
//! port.
//!
//! None of these is any Agent's protocol: the conversation lane is this host's
//! control plane, the generic CLI lane is its fallback for catalog Agents that
//! ship no dedicated driver, the caller manager installs a plugin this host
//! ships, and the installation-state projection reduces the manager's answer to
//! one word. They arrive through the port for the same reason the per-Agent
//! halves do — the moved crate names no module above it.
//!
//! It is the crate root's composition rather than a `platform` module, because
//! answering two of these lanes reads domain facts — the integration state the
//! Codex plugin manager reports and the collaboration plugin's ACP server
//! registration. The platform layer may not reach the domain layer, so the
//! composition that joins them lives above both, exactly as [`crate::target_port`]
//! does for the Agent inventory.

use crate::domain::integration_state::IntegrationState;
use licoup_agent_drivers::runtime_adapters::port::{
    CallerConfigPort, CallerManagerPort, CollaborationMcpPort, ConversationHostPort, GenericCliPort,
};
use licoup_agent_drivers::runtime_adapters::subagent_mesh::CallerManagerKind;
use serde_json::Value;

/// The conversation lane, as the Subagent mesh's two paths reach it.
pub(super) fn conversation_host() -> ConversationHostPort {
    ConversationHostPort {
        execute: |method, params| {
            crate::platform::conversation_host_client::execute(method, params)
                .map_err(|error| error.to_string())
        },
        execute_read_only: |method, params| {
            crate::platform::conversation_host_client::execute_read_only(method, params)
                .map_err(|error| error.to_string())
        },
    }
}

/// The generic CLI/PTY fallback lane.
pub(super) fn generic_cli() -> GenericCliPort {
    GenericCliPort {
        driver_id: crate::platform::generic_cli_driver::DRIVER_ID,
        resolve_executable: resolve_generic_cli_executable,
        execute: |registration, executable, params, text, cwd, timeout_ms, max_stdout| {
            let result = crate::platform::generic_cli_driver::execute(
                registration,
                executable,
                params,
                text,
                cwd,
                timeout_ms,
                max_stdout,
            )?;
            Ok(
                licoup_agent_drivers::runtime_adapters::port::GenericCliRun {
                    ok: result.ok,
                    timed_out: result.timed_out,
                    output: result.output,
                    status_code: result.status_code,
                    stdout_truncated: result.stdout_truncated,
                    started_at: result.started_at,
                    runtime_protocol: result.runtime_protocol,
                },
            )
        },
    }
}

/// The generic CLI/PTY fallback lane's executable resolution.
///
/// The lane resolves through this host's own target port, which the moved
/// crate hands back on every call rather than re-deriving it.
fn resolve_generic_cli_executable(
    port: &licoup_agent_targets::port::AgentTargetPort,
    registration: &licoup_agent_targets::domain::cli_registration::CliRegistration,
    params: &Value,
) -> Result<String, licoup_agent_drivers::runtime_adapters::RuntimeAdapterError> {
    let _ = port;
    crate::platform::generic_cli_driver::resolve_executable(registration, params)
}

/// The user-configuration caller registrations this host manages, in the shape
/// the Subagent mesh drives them through the port.
pub(super) fn caller_config() -> CallerConfigPort {
    CallerConfigPort {
        default_connector:
            crate::platform::antigravity_subagent_mcp_manager::default_mcp_binary_path,
        ready: |kind, connector| match kind {
            CallerManagerKind::CursorUserConfig => {
                crate::platform::cursor_subagent_mcp_manager::status(connector)
                    == IntegrationState::Ready
            }
            CallerManagerKind::AntigravityUserConfig => {
                crate::platform::antigravity_subagent_mcp_manager::status(connector)
                    == IntegrationState::Ready
            }
            CallerManagerKind::ClaudeCodeUserConfig => {
                crate::platform::claude_code_subagent_mcp_manager::status(connector)
                    == IntegrationState::Ready
            }
            // The plugin-shipping Agent is answered by `caller_manager` above.
            CallerManagerKind::CodexPlugin => false,
        },
        plan_digest: |kind, connector| {
            match kind {
            CallerManagerKind::CursorUserConfig => {
                crate::platform::cursor_subagent_mcp_manager::plan(connector)
                    .map(|plan| plan.digest().to_owned())
                    .map_err(|_| ())
            }
            CallerManagerKind::AntigravityUserConfig => {
                crate::platform::antigravity_subagent_mcp_manager::AntigravitySubagentMcpPlan::prepare(
                    "antigravity",
                    connector,
                )
                .map(|plan| plan.digest().to_owned())
                .map_err(|_| ())
            }
            CallerManagerKind::ClaudeCodeUserConfig => {
                crate::platform::claude_code_subagent_mcp_manager::plan(connector)
                    .map(|plan| plan.digest().to_owned())
                    .map_err(|_| ())
            }
            CallerManagerKind::CodexPlugin => Err(()),
        }
        },
        apply: |kind, connector, digest, remove| match kind {
            CallerManagerKind::CursorUserConfig => {
                let plan = crate::platform::cursor_subagent_mcp_manager::plan(connector)
                    .map_err(|_| ())?;
                let mut permit = plan.approve(true, digest).map_err(|_| ())?;
                if remove {
                    crate::platform::cursor_subagent_mcp_manager::remove(&plan, &mut permit)
                } else {
                    crate::platform::cursor_subagent_mcp_manager::install(&plan, &mut permit)
                }
                .map(|_| ())
                .map_err(|_| ())
            }
            CallerManagerKind::AntigravityUserConfig => {
                let plan =
                    crate::platform::antigravity_subagent_mcp_manager::AntigravitySubagentMcpPlan::prepare(
                        "antigravity",
                        connector,
                    )
                    .map_err(|_| ())?;
                let mut permit = plan.approve(true, digest).map_err(|_| ())?;
                if remove {
                    crate::platform::antigravity_subagent_mcp_manager::remove(&plan, &mut permit)
                        .map_err(|_| ())
                } else {
                    crate::platform::antigravity_subagent_mcp_manager::install(&plan, &mut permit)
                        .map(|_| ())
                        .map_err(|_| ())
                }
            }
            CallerManagerKind::ClaudeCodeUserConfig => {
                let plan = crate::platform::claude_code_subagent_mcp_manager::plan(connector)
                    .map_err(|_| ())?;
                let mut permit = plan.approve(true, digest).map_err(|_| ())?;
                if remove {
                    crate::platform::claude_code_subagent_mcp_manager::remove(&plan, &mut permit)
                } else {
                    crate::platform::claude_code_subagent_mcp_manager::install(&plan, &mut permit)
                }
                .map(|_| ())
                .map_err(|_| ())
            }
            CallerManagerKind::CodexPlugin => Err(()),
        },
    }
}

/// The caller-integration manager for a plugin this client installs.
///
/// It installs none. The caller integration a Codex client needs is carried by
/// a plugin that belongs to Codex's own marketplace, and the adapter package
/// owns that side; this host refuses rather than cloning a plugin repository on
/// the user's behalf. Every member fails closed, so a caller registration is
/// reported as unapplied instead of being invented.
pub(super) fn caller_manager() -> CallerManagerPort {
    CallerManagerPort {
        plan_digest: |_| Err(()),
        ready: |_| false,
        apply: |_, _, _| Err(()),
    }
}

/// End one Agent's persisted conversation, as the Subagent mesh reaches it.
///
/// The answer is the port's own cleanup entry rather than the lane's, so the
/// mesh and this host's other callers end a conversation through one path.
pub(super) fn cleanup_conversation(params: &Value) -> Result<Value, String> {
    crate::agent_port::cleanup(params).map_err(|error| error.to_string())
}

/// The MCP servers one ACP runtime reaches this host's collaboration surface
/// through, as the ACP session transport reads them.
///
/// The registration is read out of this host's own client state, which is why
/// it is here rather than in the moved crate: the answer belongs to the host's
/// collaboration surface, not to any Agent's protocol. Measured, the function
/// it answers with validates the runtime identity, opens the client-state
/// store, and then reports no server — `acp_servers_in` has its whole body
/// disabled with "a local installation never activates outbound access", so the
/// transport's `mcpServers` is empty for every ACP runtime today. The port is
/// therefore behaviour-preserving and, for now, permanently empty; whether the
/// registration is restored or removed is a decision above this extraction.
pub(super) fn collaboration_mcp() -> CollaborationMcpPort {
    CollaborationMcpPort {
        acp_servers_for_runtime: |runtime_id| {
            crate::domain::collaboration_plugin::acp_servers_for_runtime(runtime_id)
                .map_err(|error| error.to_string())
        },
    }
}
