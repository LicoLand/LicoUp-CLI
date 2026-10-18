use super::failure::ProtocolFailure;
use serde_json::Value;
use std::io;
use std::path::Path;
use uuid::Uuid;

#[derive(Clone, Debug)]
pub struct DriverConfig {
    pub prompt: String,
    pub requested_session_id: String,
    pub model: Option<String>,
    pub reasoning_effort: Option<String>,
    /// Vendor permission mode requested by the caller, or None when the turn
    /// leaves it to the launch default. LaunchIdentity resolves the default
    /// (bypassPermissions, the vendor YOLO mode) before argv and effective
    /// settings are projected, so an unspecified turn remains compatible with
    /// any live process while an explicit supported value stays authoritative.
    pub permission_mode: Option<String>,
    pub allowed_tools: Option<String>,
    pub private_instructions: Option<String>,
    pub turn_id: String,
}

impl DriverConfig {
    pub fn from_params(
        params: &Value,
        prompt: &str,
        session_id: &str,
        _cwd: Option<&Path>,
    ) -> Result<Self, ProtocolFailure> {
        if prompt.trim().is_empty() {
            return Err(ProtocolFailure::new(
                "claude_code_empty_prompt",
                "Claude Code requires a non-empty message.",
                "request/validate",
            ));
        }
        let private_instructions =
            text_param(params, &["privateInstructions", "private_instructions"]);
        let reasoning_effort = text_param(params, &["reasoningEffort", "reasoning_effort"]);
        if reasoning_effort.as_deref().is_some_and(|value| {
            !matches!(
                value,
                "low" | "medium" | "high" | "xhigh" | "max" | "ultracode"
            )
        }) {
            return Err(ProtocolFailure::new(
                "claude_code_invalid_effort",
                "Claude Code does not support the requested effort level.",
                "request/validate",
            ));
        }
        let permission_mode = text_param(
            params,
            &[
                "permissionMode",
                "permission_mode",
                "approvalPolicy",
                "approval_policy",
            ],
        )
        .map(|value| match value.as_str() {
            "manual" => "manual".to_string(),
            "on-request" => "default".to_string(),
            "never" => "dontAsk".to_string(),
            _ => value,
        });
        let allowed_tools = params
            .get("allowedTools")
            .or_else(|| params.get("allowed-tools"))
            .and_then(Value::as_array)
            .map(|tools| {
                tools
                    .iter()
                    .filter_map(Value::as_str)
                    .map(str::trim)
                    .filter(|value| !value.is_empty())
                    .collect::<Vec<_>>()
            })
            .filter(|values| !values.is_empty())
            .map(|values| {
                // A bound the native contract keeps must reject explicitly;
                // a silent prefix would execute a different allowlist than the
                // one the caller authorized.
                if values.len() > 64 {
                    return Err(ProtocolFailure::new(
                        "claude_code_allowed_tools_unsupported",
                        "Claude Code accepts at most 64 allowed tools in one launch; the requested allowlist cannot be executed and is not reported as applied.",
                        "request/validate",
                    ));
                }
                Ok(values.join(","))
            })
            .transpose()?;
        if permission_mode.as_deref().is_some_and(|value| {
            !matches!(
                value,
                "default"
                    | "manual"
                    | "acceptEdits"
                    | "plan"
                    | "auto"
                    | "dontAsk"
                    | "bypassPermissions"
            )
        }) {
            return Err(ProtocolFailure::new(
                "claude_code_invalid_permission_mode",
                "Claude Code does not support the requested permission mode.",
                "request/validate",
            ));
        }
        // An omitted permission mode is not an explicit selection: LaunchIdentity
        // resolves the launch default (bypassPermissions, the vendor YOLO mode)
        // before argv and effective settings are projected, keeping process
        // continuation compatible while fresh and resumed launches still start
        // in YOLO. Unsupported values fail validation here, before launch.
        Ok(Self {
            prompt: prompt.to_string(),
            requested_session_id: session_id.trim().to_string(),
            model: text_param(params, &["model", "modelId"]),
            reasoning_effort,
            permission_mode,
            allowed_tools,
            private_instructions,
            turn_id: Uuid::new_v4().to_string(),
        })
    }

    pub fn stdin_message(&self) -> io::Result<Value> {
        // The prompt stays off argv entirely; a fresh-process resume passes
        // only the native session identifier via --resume (LaunchIdentity).
        Ok(super::parser::user_message(&self.prompt))
    }
}

fn text_param(params: &Value, keys: &[&str]) -> Option<String> {
    keys.iter()
        .find_map(|key| params.get(*key).and_then(Value::as_str))
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
}
