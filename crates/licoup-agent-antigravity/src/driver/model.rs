use super::errors::ProtocolFailure;
use licoup_agent_adapter_sdk::Transition;
use serde_json::Value;
use std::time::Duration;

/// Temporary official Antigravity CLI lane.
///
/// Prompt and native conversation identity travel in launch arguments
/// (`--print=<prompt>`, `--conversation=<id>`), matching Cursor's argv privacy
/// exception. Session identity is recovered from the official Agent Hooks
/// contract (`conversationId` on stdin / `ANTIGRAVITY_CONVERSATION_ID`).
pub const RUNTIME_PROTOCOL: &str = "antigravity-cli-argv-hook-v1";
pub const DRIVER_ID: &str = "antigravity-cli";
pub(super) const HOOK_NAMESPACE: &str = "lico-up-antigravity-session";
/// The receipt path variable the launching driver exports for one turn.
///
/// The name belongs to the module that writes the receipt, so the launcher reads
/// it from there rather than retyping it: the writer and the launcher cannot
/// disagree about which variable names the file.
pub(super) use crate::hook::RECEIPT_ENV;
pub(super) const PROCESS_POLL_INTERVAL: Duration = Duration::from_millis(50);

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
    pub(super) fn failed(
        failure: ProtocolFailure,
        started_at: String,
        stdout_truncated: bool,
        stderr_truncated: bool,
    ) -> Self {
        let session_id = failure.session_id.clone().unwrap_or_default();
        let transitions =
            crate::parser::failure_transitions(failure.code, failure.stage, failure.message);
        Self {
            ok: false,
            output: String::new(),
            transitions,
            thread_id: failure.thread_id.clone().unwrap_or_default(),
            session_id,
            turn_id: failure.turn_id.clone().unwrap_or_default(),
            turn_status: failure.turn_status.clone().unwrap_or_default(),
            effective: EffectiveSettings::default(),
            error: Some(failure),
            status_code: None,
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
    pub stdin_prompt: bool,
    pub structured_stream: bool,
    pub new_session: bool,
    pub resume_session: bool,
    pub model: bool,
    pub reasoning_effort: bool,
    pub permission_mode: bool,
    pub interactive_approval_events: bool,
    pub error_code: Option<&'static str>,
}

impl CapabilityProbe {
    pub(super) fn unavailable() -> Self {
        Self {
            available: false,
            supported: false,
            error_code: Some("antigravity_executable_unavailable"),
            ..Self::default()
        }
    }

    pub(super) fn official(
        version_command_ok: bool,
        help_command_ok: bool,
        help_text: &str,
    ) -> Self {
        let print = help_text.contains("--print");
        let conversation = help_text.contains("--conversation");
        let model = help_text.contains("--model");
        let effort = help_text.contains("--effort");
        let supported = print && conversation;
        Self {
            available: true,
            supported,
            version_command_ok,
            help_command_ok,
            stdin_prompt: false,
            structured_stream: false,
            new_session: supported,
            resume_session: supported,
            model,
            reasoning_effort: effort,
            permission_mode: help_text.contains("--dangerously-skip-permissions"),
            interactive_approval_events: false,
            error_code: if supported {
                None
            } else {
                Some("antigravity_cli_argv_hook_surface_unavailable")
            },
        }
    }
}
