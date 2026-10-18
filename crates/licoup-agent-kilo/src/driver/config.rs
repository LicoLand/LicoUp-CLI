//! One Kilo turn's request, read out of the host's parameters.
//!
//! The host passes generic conversation parameters; which of them this Agent
//! accepts, and under which spellings, is this Agent's own contract. A value the
//! contract does not accept is refused here rather than forwarded, and the one
//! refusal that matters is the working directory: this protocol resolves a
//! session against an absolute directory, so a relative one is rejected before
//! any endpoint is touched.

use serde_json::Value;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

/// One serve turn's request.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ServeTurnConfig {
    pub prompt: String,
    pub private_instructions: Option<String>,
    pub requested_session_id: String,
    pub cwd: String,
    pub model: Option<String>,
    pub runtime_agent: Option<String>,
    pub reasoning_effort: Option<String>,
    pub mode: Option<String>,
    pub allow_all: Option<bool>,
}

impl ServeTurnConfig {
    /// Read one turn's request, or refuse it.
    pub fn from_params(
        params: &Value,
        prompt: &str,
        session_id: &str,
        cwd: Option<&Path>,
    ) -> Result<Self, super::ProtocolFailure> {
        let cwd = cwd
            .map(Path::to_path_buf)
            .or_else(|| {
                params
                    .get("cwd")
                    .or_else(|| params.get("workingDirectory"))
                    .and_then(Value::as_str)
                    .map(PathBuf::from)
            })
            .unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| PathBuf::from("/")));
        if !cwd.is_absolute() {
            return Err(super::ProtocolFailure::new(
                "acp_working_directory_invalid",
                "ACP conversation sessions require an absolute working directory.",
                "initialize",
            ));
        }
        Ok(Self {
            prompt: prompt.to_string(),
            private_instructions: text_setting(params, &["privateInstructions"]),
            requested_session_id: session_id.trim().to_string(),
            cwd: cwd.to_string_lossy().into_owned(),
            model: text_setting(params, &["model"]),
            runtime_agent: text_setting(params, &["agent", "runtimeAgent"]),
            reasoning_effort: text_setting(params, &["reasoningEffort", "reasoning"]),
            mode: text_setting(params, &["mode"]),
            allow_all: params.get("allowAll").and_then(Value::as_bool),
        })
    }

    /// Whether this turn continues a native session rather than starting one.
    pub fn is_resume(&self) -> bool {
        !self.requested_session_id.is_empty()
    }
}

/// The first of several accepted spellings that carries a non-empty value.
///
/// An empty string is not a setting: it is reported as absent so a turn never
/// asks the endpoint for a model named `""`.
fn text_setting(params: &Value, keys: &[&str]) -> Option<String> {
    keys.iter().find_map(|key| {
        params
            .get(*key)
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_string)
    })
}

/// The wall-clock instant one turn started, in milliseconds since the epoch.
pub fn timestamp() -> String {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn a_relative_working_directory_is_refused_before_any_endpoint_is_touched() {
        let failure =
            ServeTurnConfig::from_params(&json!({}), "hello", "", Some(Path::new("relative/dir")))
                .unwrap_err();
        assert_eq!(failure.code, "acp_working_directory_invalid");
        assert_eq!(failure.stage, "initialize");
    }

    #[test]
    fn accepted_spellings_fall_back_in_order_and_empty_values_are_absent() {
        let config = ServeTurnConfig::from_params(
            &json!({
                "cwd": "/workspace/project",
                "agent": "  ",
                "runtimeAgent": "build",
                "reasoning": "low",
                "reasoningEffort": "high",
                "privateInstructions": "be brief",
                "allowAll": true,
            }),
            "hello",
            "  kilo-1  ",
            None,
        )
        .unwrap();
        assert_eq!(config.cwd, "/workspace/project");
        assert_eq!(config.runtime_agent.as_deref(), Some("build"));
        assert_eq!(config.reasoning_effort.as_deref(), Some("high"));
        assert_eq!(config.private_instructions.as_deref(), Some("be brief"));
        assert_eq!(config.requested_session_id, "kilo-1");
        assert_eq!(config.allow_all, Some(true));
        assert!(config.is_resume());
    }
}
