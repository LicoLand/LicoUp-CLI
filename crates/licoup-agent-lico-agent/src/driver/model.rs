use super::errors::ProtocolFailure;
use licoup_agent_adapter_sdk::Transition;
use serde_json::Value;

// The runtime protocol id this Agent's turns run under is the adapter package's
// fact, named once there and read here, so the id the execution surface reports
// and the protocol that package parses cannot describe two different wires.
pub use crate::parser::RUNTIME_PROTOCOL;

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
    pub transitions: Vec<Transition>,
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
    pub(crate) fn failed(failure: ProtocolFailure, started_at: String) -> Self {
        let transitions =
            crate::parser::failure_transitions(failure.code, failure.stage, failure.message);
        Self {
            ok: false,
            output: String::new(),
            transitions,
            error: Some(failure),
            session_id: String::new(),
            thread_id: String::new(),
            turn_id: String::new(),
            turn_status: String::new(),
            effective: EffectiveSettings::default(),
            status_code: None,
            stdout_truncated: false,
            stderr_truncated: false,
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
