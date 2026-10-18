use super::errors::ProtocolFailure;
use serde_json::Value;
use std::time::Duration;

/// OpenClaw's native ACP bridge over JSON-RPC lines. Private prompt and
/// conversation values stay on stdin; launch arguments contain only attach data.
pub const RUNTIME_PROTOCOL: &str = "openclaw-acp-stdio-jsonrpc";
pub const PROCESS_POLL_INTERVAL: Duration = Duration::from_millis(50);

#[derive(Clone, Debug, Default)]
pub struct EffectiveSettings {
    pub cwd: Option<String>,
    pub model: Option<String>,
    pub reasoning_effort: Option<String>,
    pub sandbox: Option<Value>,
    pub approval_policy: Option<Value>,
}

#[derive(Debug)]
pub struct RunResult {
    pub ok: bool,
    pub output: String,
    pub transitions: Vec<licoup_agent_adapter_sdk::Transition>,
    pub error: Option<ProtocolFailure>,
    pub session_id: String,
    pub thread_id: String,
    pub turn_id: String,
    pub turn_status: String,
    pub effective: EffectiveSettings,
    pub status_code: Option<i32>,
    pub stdout_truncated: bool,
    pub stderr_truncated: bool,
    pub started_at: String,
}

impl RunResult {
    pub fn failed(
        failure: ProtocolFailure,
        started_at: String,
        status_code: Option<i32>,
        stdout_truncated: bool,
        stderr_truncated: bool,
    ) -> Self {
        let session_id = failure.session_id.clone().unwrap_or_default();
        let transitions =
            crate::parser::failed_transitions(&failure.code, &failure.stage, &failure.message);
        Self {
            ok: false,
            output: String::new(),
            transitions,
            error: Some(failure.clone()),
            thread_id: session_id.clone(),
            session_id,
            turn_id: failure.turn_id.clone().unwrap_or_default(),
            turn_status: failure.turn_status.clone().unwrap_or_default(),
            effective: EffectiveSettings::default(),
            status_code,
            stdout_truncated,
            stderr_truncated,
            started_at,
        }
    }
}

#[derive(Clone, Debug, Default)]
pub struct CapabilityProbe {
    pub available: bool,
    pub supported: bool,
    pub version: Option<String>,
    pub error_code: Option<&'static str>,
    pub supports_streaming: bool,
    pub supports_tools: bool,
    pub supports_approvals: bool,
    pub supports_reasoning: bool,
    pub supports_model_override: bool,
}
