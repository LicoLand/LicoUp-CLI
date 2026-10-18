//! The settings one Claude Code launch reports, and the capability facts a
//! probe reads from the installed CLI.
//!
//! These are the fields the CLI's own `system`/`init` frame and `--help` output
//! carry, in the shape the client projects them. They are this Agent's
//! vocabulary, so they live with the protocol rather than with the process that
//! happens to read them.

use serde_json::Value;

/// The settings a Claude Code turn actually runs with.
#[derive(Clone, Debug, Default)]
pub struct EffectiveSettings {
    /// The working directory the CLI reported.
    pub cwd: Option<String>,
    /// The model the CLI reported.
    pub model: Option<String>,
    /// The reasoning effort the launch requested.
    pub reasoning_effort: Option<String>,
    /// The vendor permission mode in force.
    pub permission_mode: Option<String>,
    /// The vendor sandbox policy, when the CLI reports one.
    pub sandbox: Option<Value>,
    /// The vendor approval policy, as the client projects it.
    pub approval_policy: Option<Value>,
}

/// What the installed CLI's own command line says it can do.
///
/// Continuation is available only while the exact supervised streaming-input
/// process remains live. Persisted CLI resume is deliberately reported apart
/// from it, because the vendor contract puts the native session identifier on
/// argv rather than in a durable record this client owns.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct CapabilityProbe {
    /// Whether the CLI answered at all.
    pub available: bool,
    /// Whether `<cli> --version` succeeded.
    pub version_command_ok: bool,
    /// Whether `<cli> --help` succeeded.
    pub help_command_ok: bool,
    /// Whether the CLI reads its prompt on standard input.
    pub stdin_prompt: bool,
    /// Whether the CLI writes a structured stream.
    pub structured_stream: bool,
    /// Whether the CLI can open a new conversation.
    pub new_session: bool,
    /// Whether the CLI can resume a persisted conversation.
    pub resume_session: bool,
    /// Whether the CLI accepts a model selection.
    pub model: bool,
    /// Whether the CLI accepts a reasoning-effort selection.
    pub reasoning_effort: bool,
    /// Whether the CLI accepts a permission mode.
    pub permission_mode: bool,
    /// Whether the CLI emits interactive approval events.
    pub interactive_approval_events: bool,
}

impl CapabilityProbe {
    /// What this Agent's documented streaming-input lane provides once the CLI
    /// answers its own version or help command.
    pub fn official(version_command_ok: bool, help_command_ok: bool) -> Self {
        Self {
            available: version_command_ok || help_command_ok,
            version_command_ok,
            help_command_ok,
            stdin_prompt: true,
            structured_stream: true,
            new_session: true,
            resume_session: true,
            model: true,
            reasoning_effort: true,
            permission_mode: true,
            interactive_approval_events: false,
        }
    }
}
