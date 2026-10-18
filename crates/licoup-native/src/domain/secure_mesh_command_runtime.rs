//! Application composition for executing already-authorized Secure Mesh commands.
//!
//! The protocol core owns parsing, policy, replay protection, and result shaping. This
//! adapter alone is allowed to resolve local agents and invoke platform runtime lanes.

use anyhow::{Result, anyhow};
use serde_json::{Value, json};
use std::fs;
use std::path::PathBuf;
#[cfg(test)]
use std::{cell::RefCell, path::Path};

use crate::core::secure_mesh_command::{
    SecureAgentDispatchFailure, SecureCommandLocalExecutor, SecureCommandPayload,
    agent_message_send_params, agent_sessions_describe_params, agent_sessions_list_params,
    dispatch_ready_agent_message,
};

const SECURE_MESH_COMMAND_LEDGER_PATH: &str = "licoup/secure-mesh/command-replay.sqlite";

#[cfg(test)]
thread_local! {
    static SECURE_COMMAND_HISTORY_HOME_OVERRIDE: RefCell<Option<PathBuf>> =
        const { RefCell::new(None) };
}

#[derive(Default)]
pub(crate) struct SecureCommandRuntimeExecutor;

impl SecureCommandLocalExecutor for SecureCommandRuntimeExecutor {
    fn execute_secure_command(&mut self, payload: &SecureCommandPayload) -> Result<Value> {
        match payload.command_kind.as_str() {
            "agent.sessions.list" => {
                let mut params = agent_sessions_list_params(payload)?;
                apply_test_history_home(&mut params);
                super::conversations::conversation_list(&params)
            }
            "agent.sessions.describe" => {
                let mut params = agent_sessions_describe_params(payload)?;
                apply_test_history_home(&mut params);
                super::conversations::conversation_list(&params)
            }
            "agent.message.send" => {
                let mut params = agent_message_send_params(payload)?;
                let agent = text_from_any(&params, &["agent", "agentId", "target"])
                    .ok_or_else(|| anyhow!("agent message target is unavailable"))?;
                let executable = super::targets::available_runtime_executable(
                    &crate::target_port::agent_target_port(),
                    &agent,
                )
                .ok_or_else(|| {
                    anyhow::Error::new(SecureAgentDispatchFailure::new(
                        "native_agent_runtime_binding_unavailable",
                        false,
                    ))
                })?;
                params["binaryPath"] = json!(executable.to_string_lossy());
                dispatch_ready_agent_message(&params, |operation, params| {
                    crate::agent_port::dispatch(operation, params).map_err(anyhow::Error::new)
                })
            }
            "secure_mesh.device.verify" => Err(anyhow!(
                "secure mesh command runtime binding requires an interactive endpoint UI for {}",
                payload.command_kind
            )),
            _ => Err(anyhow!(
                "secure mesh command runtime binding does not implement {}",
                payload.command_kind
            )),
        }
    }
}

#[cfg(test)]
pub(crate) fn with_secure_command_test_history_home<T>(
    home: &Path,
    operation: impl FnOnce() -> T,
) -> T {
    struct Reset(Option<PathBuf>);
    impl Drop for Reset {
        fn drop(&mut self) {
            SECURE_COMMAND_HISTORY_HOME_OVERRIDE.with(|slot| {
                slot.replace(self.0.take());
            });
        }
    }

    let previous =
        SECURE_COMMAND_HISTORY_HOME_OVERRIDE.with(|slot| slot.replace(Some(home.to_path_buf())));
    let _reset = Reset(previous);
    operation()
}

#[cfg(test)]
fn apply_test_history_home(params: &mut Value) {
    SECURE_COMMAND_HISTORY_HOME_OVERRIDE.with(|slot| {
        if let Some(home) = slot.borrow().as_ref() {
            params["homeDir"] = json!(home.to_string_lossy());
        }
    });
}

#[cfg(not(test))]
fn apply_test_history_home(_params: &mut Value) {}

pub(crate) fn default_secure_command_ledger_path() -> Result<PathBuf> {
    let path = licoup_foundation::platform::paths::portable_data_dir()?
        .join(SECURE_MESH_COMMAND_LEDGER_PATH);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    Ok(path)
}

fn text_from_any(value: &Value, keys: &[&str]) -> Option<String> {
    keys.iter().find_map(|key| {
        value
            .get(*key)
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|text| !text.is_empty())
            .map(str::to_owned)
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::secure_mesh::SECURE_MESH_COMMAND_PROTOCOL_VERSION;

    #[test]
    fn runtime_adapter_reports_missing_executable_binding() {
        let payload = SecureCommandPayload::from_value(&json!({
            "schema": SECURE_MESH_COMMAND_PROTOCOL_VERSION,
            "commandId": "cmd-unready-agent",
            "commandKind": "agent.message.send",
            "senderIdentity": {
                "endpointId": "pc-a",
                "identityFingerprint": "fingerprint-a",
                "trustState": "verified",
                "endpointKind": "desktop_sidecar"
            },
            "targetBinding": {
                "targetEndpointId": "pc-b",
                "targetAgentId": "unsupported-fixture-agent",
                "workspaceId": "workspace-a"
            },
            "riskClass": "safe_write",
            "requiresUserConfirmation": false,
            "idempotencyKey": "idem-unready-agent",
            "createdAt": "2026-01-01T00:00:00Z",
            "expiresAt": "2026-01-01T00:10:00Z",
            "body": {
                "text": "fixture message"
            }
        }))
        .unwrap();
        let error = SecureCommandRuntimeExecutor
            .execute_secure_command(&payload)
            .unwrap_err();
        assert_eq!(
            error.to_string(),
            "native_agent_runtime_binding_unavailable"
        );
    }
}
