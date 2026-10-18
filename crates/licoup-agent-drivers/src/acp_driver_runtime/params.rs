use super::errors::ProtocolFailure;
use serde_json::Value;
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Clone, Debug)]
pub struct RequestedSettings {
    pub model: Option<String>,
    pub reasoning_effort: Option<String>,
    pub mode: Option<String>,
    pub runtime_agent: Option<String>,
    pub allow_all: Option<bool>,
}

#[derive(Clone, Debug)]
pub struct ProtocolConfig {
    pub prompt: String,
    pub requested_session_id: String,
    pub cwd: String,
    pub settings: RequestedSettings,
    pub allow_all_authorized: bool,
    pub mcp_servers: Vec<Value>,
}

impl ProtocolConfig {
    pub fn from_params(
        params: &Value,
        prompt: &str,
        session_id: &str,
        cwd: Option<&Path>,
    ) -> Result<Self, ProtocolFailure> {
        let Some(cwd) = cwd else {
            return Err(ProtocolFailure::new(
                "acp_working_directory_required",
                "ACP conversation sessions require a working directory.",
                "session/configure",
            ));
        };
        if !cwd.is_absolute() {
            return Err(ProtocolFailure::new(
                "acp_working_directory_invalid",
                "ACP conversation sessions require an absolute working directory.",
                "session/configure",
            ));
        }
        if prompt.is_empty() {
            return Err(ProtocolFailure::new(
                "acp_prompt_required",
                "ACP conversation sessions require a non-empty prompt.",
                "session/configure",
            ));
        }
        if params
            .get("privateInstructions")
            .and_then(Value::as_str)
            .is_some_and(|value| !value.is_empty())
        {
            return Err(ProtocolFailure::new(
                "acp_private_instructions_unsupported",
                "This ACP agent does not expose a private instruction channel.",
                "capability/private-instructions",
            ));
        }
        let allow_all = params.get("allowAll").and_then(Value::as_bool);
        Ok(Self {
            prompt: prompt.to_string(),
            requested_session_id: session_id.trim().to_string(),
            cwd: cwd.to_string_lossy().to_string(),
            settings: RequestedSettings {
                model: text_param(params, &["model", "modelId"]),
                reasoning_effort: text_param(
                    params,
                    &["reasoningEffort", "reasoning_effort", "variant"],
                ),
                mode: text_param(params, &["runtimeMode", "agentMode", "conversationMode"]),
                runtime_agent: text_param(params, &["runtimeAgent", "customAgent"]),
                allow_all,
            },
            allow_all_authorized: allow_all == Some(true),
            mcp_servers: Vec::new(),
        })
    }

    pub fn is_resume(&self) -> bool {
        !self.requested_session_id.is_empty()
    }

    pub fn load_collaboration_mcp(&mut self, runtime_id: &str) -> Result<(), ProtocolFailure> {
        self.mcp_servers = crate::runtime_adapters::port::collaboration_acp_servers(runtime_id)
            .map_err(|_| {
                ProtocolFailure::new(
                    "acp_mcp_registration_invalid",
                    "The optional MCP registration could not be validated safely.",
                    "session/mcp",
                )
            })?;
        Ok(())
    }
}

fn text_param(params: &Value, keys: &[&str]) -> Option<String> {
    keys.iter().find_map(|key| {
        params
            .get(*key)
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_string)
    })
}

pub fn timestamp() -> String {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .to_string()
}
