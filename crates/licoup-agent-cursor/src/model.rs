//! Cursor's launch and wire vocabulary, as the package's own facts.
//!
//! The client composes the process half of a Cursor turn; *what* that process
//! is — the fixed launch arguments, the capability surface the executable must
//! prove, the effective settings one turn reports, and the result shape the
//! client's normalization reads — belongs to Cursor and lives here.
//!
//! Nothing here starts a process. [`CapabilityProbe`] is the classification of
//! the `--version`/`--help` output the host reads; the host owns the bounded
//! spawn, exactly as it owns the turn's process.

use crate::errors::ProtocolFailure;
use serde_json::Value;
use std::time::Duration;

/// Official Cursor Agent CLI lane. Prompt and native session identity travel in
/// fixed launch arguments; continuity is backed by local Cursor chat storage.
pub const RUNTIME_PROTOCOL: &str = "cursor-agent-cli-v1";
pub const DRIVER_ID: &str = "cursor-cli";
pub const CREATE_CHAT_ARGS: &[&str] = &["create-chat"];
pub const TURN_ARGS: &[&str] = &[
    "--print",
    "--output-format",
    "stream-json",
    "--trust",
    "--force",
    "--approve-mcps",
    "--stream-partial-output",
];
/// The wire format this package's entry speaks, as the release declaration
/// names it. It is the format identity, not the channel label: the channel the
/// parser and its fixtures record is `strict-lf-ndjson`.
pub const PROTOCOL_FORMAT: &str = "cursor.agent-cli.stream-json.v1";
pub const PROCESS_POLL_INTERVAL: Duration = Duration::from_millis(50);
pub const MAX_SESSION_ID_LEN: usize = 128;
pub const MIN_SESSION_ID_LEN: usize = 8;

/// The settings one Cursor turn was actually run with.
///
/// A field is `None` until the turn's own evidence reports it: an init frame
/// names the model and permission mode the CLI resolved, and the launch context
/// names the workspace. Nothing here is inferred from a neighbouring turn.
#[derive(Clone, Debug, Default)]
pub struct EffectiveSettings {
    pub cwd: Option<String>,
    pub model: Option<String>,
    pub reasoning_effort: Option<String>,
    pub permission_mode: Option<String>,
    pub sandbox: Option<Value>,
    pub approval_policy: Option<Value>,
}

/// One Cursor execution outcome, in the shape the client normalizes.
///
/// It is the driver's result rather than a wire frame: the fields are the
/// parser's own reports plus the process facts the host measured. The transition
/// list is the parser's reducer output, so the failure path here is the same
/// vocabulary the package publishes through its registration.
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
        stdout_truncated: bool,
        stderr_truncated: bool,
    ) -> Self {
        Self::failed_with_status(
            failure,
            started_at,
            stdout_truncated,
            stderr_truncated,
            None,
        )
    }

    pub fn failed_with_status(
        failure: ProtocolFailure,
        started_at: String,
        stdout_truncated: bool,
        stderr_truncated: bool,
        status_code: Option<i32>,
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
            status_code,
            stdout_truncated,
            stderr_truncated,
            started_at,
        }
    }
}

/// The capability surface the installed Cursor Agent CLI proved.
///
/// Every field is read from the executable's own `--version`/`--help` output by
/// the host that ran it. `supported` is the conjunction the lane requires, and
/// the error code names the one reason it was not reached.
#[derive(Clone, Debug, Default)]
pub struct CapabilityProbe {
    pub available: bool,
    pub supported: bool,
    pub version_command_ok: bool,
    pub help_command_ok: bool,
    pub create_chat: bool,
    pub print_turn: bool,
    pub resume_session: bool,
    pub structured_stream: bool,
    pub error_code: Option<&'static str>,
}

impl CapabilityProbe {
    pub fn official(version_ok: bool, help_ok: bool, help_text: &str) -> Self {
        let help_lower = help_text.to_ascii_lowercase();
        let create_chat = help_lower.contains("create-chat");
        let print_turn = help_lower.contains("--print");
        let resume_session = help_lower.contains("--resume");
        let structured_stream = help_lower.contains("stream-json");
        let approve_mcps = help_lower.contains("--approve-mcps");
        let supported = version_ok
            && help_ok
            && create_chat
            && print_turn
            && resume_session
            && structured_stream
            && approve_mcps;
        Self {
            available: version_ok || help_ok,
            supported,
            version_command_ok: version_ok,
            help_command_ok: help_ok,
            create_chat,
            print_turn,
            resume_session,
            structured_stream,
            error_code: if supported {
                None
            } else {
                Some("cursor_cli_capability_incomplete")
            },
        }
    }
}
