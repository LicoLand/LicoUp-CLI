use super::errors::ProtocolFailure;
use serde_json::Value;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

pub const PROCESS_POLL_INTERVAL: Duration = Duration::from_millis(50);
pub const CONTROL_ACK_TIMEOUT: Duration = Duration::from_secs(1);
pub const CONTROL_QUEUE_CAPACITY: usize = 4;
pub const MAX_POOLED_TRANSPORTS: usize = 8;
pub const MAX_TRACKED_SESSIONS: usize = 1024;
pub const APPROVAL_POLL_INTERVAL: Duration = Duration::from_millis(50);

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct AcpSessionDriverSpec {
    pub driver_id: &'static str,
    pub runtime_id: &'static str,
    pub launch_args: &'static [&'static str],
}

impl AcpSessionDriverSpec {
    pub const fn new(driver_id: &'static str, launch_args: &'static [&'static str]) -> Self {
        Self {
            driver_id,
            runtime_id: driver_id,
            launch_args,
        }
    }

    pub const fn with_runtime_id(mut self, runtime_id: &'static str) -> Self {
        self.runtime_id = runtime_id;
        self
    }
}

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
    // Non-conversation TUI/history callers still construct this shared result.
    // The live Hermes conversation lane uses parser-produced transitions and
    // never forwards this projection through runtime normalization.
    #[allow(dead_code)]
    pub events: Vec<Value>,
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
        Self {
            ok: false,
            output: String::new(),
            events: Vec::new(),
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
    pub supports_model_override: bool,
    pub supports_reasoning_override: bool,
}

pub fn timestamp() -> String {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .to_string()
}
