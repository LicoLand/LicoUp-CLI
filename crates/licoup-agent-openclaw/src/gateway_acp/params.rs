use super::errors::ProtocolFailure;
use licoup_agent_targets::platform::virtual_machine::is_absolute_acp_working_directory;
use serde_json::{Map, Value};
use std::path::Path;
use uuid::Uuid;

#[derive(Clone, Debug)]
pub struct ProtocolConfig {
    pub prompt: String,
    pub requested_session_id: String,
    pub native_session_key: Option<String>,
    pub cwd: String,
    pub reasoning_effort: Option<String>,
    pub turn_id: String,
    pub mcp_servers: Vec<Value>,
}

impl ProtocolConfig {
    /// Validate one OpenClaw request, asking the host for its MCP registration.
    ///
    /// *Which* servers the client registers is the client's fact: it reads the
    /// user's collaboration-plugin configuration and the installed package
    /// store, neither of which this package may reach. *When* the list is needed
    /// is this package's fact, because it is a field of the session request the
    /// package builds. The supplier is a lazy argument rather than a port so
    /// both halves stay visible at the one call site that needs them, and it
    /// runs only after every request field has been accepted: a malformed
    /// request still reports its own typed failure rather than a registration
    /// one, exactly as this driver has always ordered those checks.
    pub fn from_params(
        params: &Value,
        prompt: &str,
        session_id: &str,
        cwd: Option<&Path>,
        local_mcp: impl FnOnce() -> Result<Vec<Value>, ProtocolFailure>,
    ) -> Result<Self, ProtocolFailure> {
        let mut config = Self::validated(params, prompt, session_id, cwd)?;
        config.mcp_servers = local_mcp()?;
        Ok(config)
    }

    /// Validate one OpenClaw request that registers no local MCP server.
    ///
    /// Used where the transport already carries its own registration — the
    /// virtual-machine branch — and by this package's protocol fixtures, which
    /// exercise framing rather than installation-backed registration.
    pub fn from_params_without_local_mcp(
        params: &Value,
        prompt: &str,
        session_id: &str,
        cwd: Option<&Path>,
    ) -> Result<Self, ProtocolFailure> {
        Self::validated(params, prompt, session_id, cwd)
    }

    /// Every request field this Agent accepts, normalized or refused.
    ///
    /// The registration is not a request field, so it is not read here: this
    /// function is the whole of the request validation.
    fn validated(
        params: &Value,
        prompt: &str,
        session_id: &str,
        cwd: Option<&Path>,
    ) -> Result<Self, ProtocolFailure> {
        if prompt.trim().is_empty() {
            return Err(ProtocolFailure::new(
                "openclaw_empty_prompt",
                "OpenClaw requires a non-empty message.",
                "request/validate",
            ));
        }
        if params
            .get("privateInstructions")
            .and_then(Value::as_str)
            .is_some_and(|value| !value.is_empty())
        {
            return Err(ProtocolFailure::new(
                "openclaw_acp_private_instructions_unsupported",
                "OpenClaw ACP does not expose a private instruction channel.",
                "capability/private-instructions",
            ));
        }
        if text_param(params, &["model", "modelId"]).is_some() {
            return Err(ProtocolFailure::new(
                "openclaw_acp_model_override_unsupported",
                "OpenClaw ACP does not expose native model selection.",
                "capability/model",
            ));
        }
        if explicit_value(params, &["sandbox", "sandboxMode"]).is_some() {
            return Err(ProtocolFailure::new(
                "openclaw_acp_sandbox_override_unsupported",
                "OpenClaw ACP does not expose a per-turn sandbox override.",
                "capability/sandbox",
            ));
        }
        if explicit_value(params, &["approvalPolicy", "approval_policy"]).is_some() {
            return Err(ProtocolFailure::new(
                "openclaw_acp_approval_override_unsupported",
                "OpenClaw ACP approvals require an explicit client approval response.",
                "capability/approval",
            ));
        }
        let reasoning_effort = text_param(params, &["reasoningEffort", "reasoning_effort"]);
        if reasoning_effort.as_deref().is_some_and(|value| {
            !matches!(
                value,
                "off" | "minimal" | "low" | "medium" | "high" | "xhigh" | "adaptive" | "max"
            )
        }) {
            return Err(ProtocolFailure::new(
                "openclaw_acp_invalid_thought_level",
                "The requested OpenClaw thought level is not supported.",
                "request/validate",
            ));
        }
        let cwd = cwd
            .filter(|path| is_absolute_acp_working_directory(path))
            .map(|path| path.to_string_lossy().to_string())
            .ok_or_else(|| {
                ProtocolFailure::new(
                    "openclaw_acp_absolute_cwd_required",
                    "OpenClaw ACP requires an absolute working directory.",
                    "request/validate",
                )
            })?;
        let requested_session_id = session_id.trim().to_string();
        let runtime_agent_id = text_param(
            params,
            &["openclawAgentId", "runtimeAgentId", "targetAgentId"],
        );
        let normalized_runtime_agent_id = runtime_agent_id.as_deref().map(normalize_agent_id);
        if runtime_agent_id.is_some()
            && normalized_runtime_agent_id
                .as_deref()
                .is_none_or(str::is_empty)
        {
            return Err(ProtocolFailure::new(
                "openclaw_acp_invalid_agent_id",
                "The requested OpenClaw agent identifier is invalid.",
                "request/validate",
            ));
        }
        let explicit_native_session_key = text_param(
            params,
            &["sessionKey", "nativeSessionKey", "openclawSessionKey"],
        );
        if !requested_session_id.is_empty()
            && explicit_native_session_key
                .as_deref()
                .is_some_and(|key| key != requested_session_id)
        {
            return Err(ProtocolFailure::new(
                "openclaw_acp_conflicting_session_id",
                "The requested OpenClaw conversation identifiers do not match.",
                "request/validate",
            ));
        }
        let native_session_key = explicit_native_session_key
            .or_else(|| (!requested_session_id.is_empty()).then(|| requested_session_id.clone()))
            .or_else(|| {
                normalized_runtime_agent_id
                    .map(|agent_id| format!("agent:{agent_id}:acp:{}", Uuid::new_v4()))
            });
        Ok(Self {
            prompt: prompt.to_string(),
            requested_session_id,
            native_session_key,
            cwd,
            reasoning_effort,
            turn_id: Uuid::new_v4().to_string(),
            mcp_servers: Vec::new(),
        })
    }

    pub fn is_resume(&self) -> bool {
        !self.requested_session_id.is_empty()
    }

    pub fn session_meta(&self) -> Option<Map<String, Value>> {
        self.native_session_key.as_ref().map(|key| {
            let mut meta = Map::new();
            meta.insert("sessionKey".into(), Value::String(key.clone()));
            meta.insert("requireExisting".into(), Value::Bool(self.is_resume()));
            meta
        })
    }
}

pub fn explicit_value<'a>(params: &'a Value, keys: &[&str]) -> Option<&'a Value> {
    keys.iter()
        .find_map(|key| params.get(*key))
        .filter(|value| !value.is_null())
}

pub fn text_param(params: &Value, keys: &[&str]) -> Option<String> {
    keys.iter()
        .find_map(|key| params.get(*key).and_then(Value::as_str))
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
}

pub fn normalize_agent_id(value: &str) -> String {
    let normalized = value
        .trim()
        .to_ascii_lowercase()
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() || character == '-' || character == '_' {
                character
            } else {
                '-'
            }
        })
        .collect::<String>();
    normalized.trim_matches('-').to_string()
}
