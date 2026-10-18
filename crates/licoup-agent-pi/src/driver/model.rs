use super::errors::ProtocolFailure;
use serde_json::Value;
use std::time::Duration;

/// Official Pi Coding Agent lane: `pi --mode rpc` JSONL over stdin/stdout.
/// Prompts and session identity stay on the stdio channel; launch argv is fixed.
pub const RUNTIME_PROTOCOL: &str = "pi-rpc-stdio-jsonl";
pub const PROCESS_POLL_INTERVAL: Duration = Duration::from_millis(50);

#[derive(Clone, Debug, Default)]
pub struct EffectiveSettings {
    pub cwd: Option<String>,
    pub model: Option<String>,
    pub reasoning_effort: Option<String>,
    pub permission_mode: Option<String>,
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
        let transitions = crate::parser::failed_transitions(&failure);
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
    pub version_command_ok: bool,
    pub help_command_ok: bool,
    pub error_code: Option<&'static str>,
}

impl CapabilityProbe {
    pub fn unavailable() -> Self {
        Self {
            available: false,
            supported: false,
            version_command_ok: false,
            help_command_ok: false,
            error_code: Some("pi_executable_unavailable"),
        }
    }

    pub fn installed(version_command_ok: bool, help_command_ok: bool) -> Self {
        Self {
            available: true,
            supported: true,
            version_command_ok,
            help_command_ok,
            error_code: None,
        }
    }
}
