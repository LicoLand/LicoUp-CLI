use super::params::DriverConfig;
use super::settings::EffectiveSettings;
use serde_json::Value;
use std::path::{Path, PathBuf};

pub const FIXED_STREAM_ARGS: &[&str] = &[
    "--print",
    "--input-format",
    "stream-json",
    "--output-format",
    "stream-json",
    "--verbose",
    // Token-level streaming: the CLI emits content_block_delta events so the
    // client renders replies progressively instead of whole messages.
    "--include-partial-messages",
];

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LaunchIdentity {
    pub executable: String,
    pub cwd: Option<PathBuf>,
    pub model: Option<String>,
    pub reasoning_effort: Option<String>,
    /// Vendor permission mode mapped to `--permission-mode`. The launch
    /// default (bypassPermissions, the vendor YOLO mode) is resolved here,
    /// before argv and effective settings are projected, so compatibility,
    /// effective settings, fresh sessions, and resumed sessions all observe
    /// one value; an explicit supported selection is retained unchanged.
    pub permission_mode: Option<String>,
    /// Comma-joined tool allowlist passed via `--allowedTools` so an approved
    /// retry does not re-trigger a permission denial.
    pub allowed_tools: Option<String>,
    /// Product guidance carried through Claude Code's private system-prompt
    /// channel. It is never inserted into the user message or transcript.
    pub private_instructions: Option<String>,
    /// Native conversation to resume in a freshly launched process via
    /// `--resume`. Only set when no process-local live transport owns the
    /// session; the CLI loads the persisted transcript itself.
    pub resume_session_id: Option<String>,
}

impl LaunchIdentity {
    pub fn new(executable: &str, config: &DriverConfig, cwd: Option<&Path>) -> Self {
        // Explicit selections stay authoritative; an omitted mode resolves to
        // the vendor YOLO default before the launch mapping, so the identity
        // is pinned to one value for compatibility and effective settings.
        let permission_mode = Some(
            config
                .permission_mode
                .clone()
                .unwrap_or_else(|| "bypassPermissions".to_string()),
        );
        Self {
            executable: executable.to_string(),
            cwd: cwd.map(Path::to_path_buf),
            model: config.model.clone(),
            reasoning_effort: config.reasoning_effort.clone(),
            permission_mode,
            allowed_tools: config.allowed_tools.clone(),
            private_instructions: config.private_instructions.clone(),
            resume_session_id: (!config.requested_session_id.is_empty())
                .then(|| config.requested_session_id.clone()),
        }
    }

    pub fn compatible_with(
        &self,
        executable: &str,
        config: &DriverConfig,
        cwd: Option<&Path>,
    ) -> bool {
        self.executable == executable
            && self.cwd.as_deref() == cwd
            && config
                .model
                .as_ref()
                .is_none_or(|value| self.model.as_ref() == Some(value))
            && config
                .reasoning_effort
                .as_ref()
                .is_none_or(|value| self.reasoning_effort.as_ref() == Some(value))
            // An omitted permission mode leaves the launch default to the
            // identity, so it never contradicts the pinned launch mode (the
            // vendor YOLO default or an explicit selection); only an explicit
            // switch triggers a fresh launcher.
            && config
                .permission_mode
                .as_ref()
                .is_none_or(|value| self.permission_mode.as_ref() == Some(value))
            && config
                .allowed_tools
                .as_ref()
                .is_none_or(|value| self.allowed_tools.as_ref() == Some(value))
            && self.private_instructions == config.private_instructions
    }

    pub fn args(&self) -> Vec<String> {
        let mut args = FIXED_STREAM_ARGS
            .iter()
            .map(|value| (*value).to_string())
            .collect::<Vec<_>>();
        if let Some(session_id) = self.resume_session_id.as_ref() {
            args.extend(["--resume".to_string(), session_id.clone()]);
        }
        if let Some(model) = self.model.as_ref() {
            args.extend(["--model".to_string(), model.clone()]);
        }
        if let Some(effort) = self.reasoning_effort.as_ref() {
            args.extend(["--effort".to_string(), effort.clone()]);
        }
        if let Some(permission_mode) = self.permission_mode.as_ref() {
            args.extend(["--permission-mode".to_string(), permission_mode.clone()]);
        }
        if let Some(allowed_tools) = self.allowed_tools.as_ref() {
            args.extend(["--allowedTools".to_string(), allowed_tools.clone()]);
        }
        if let Some(instructions) = self.private_instructions.as_ref() {
            args.extend(["--append-system-prompt".to_string(), instructions.clone()]);
        }
        args
    }

    pub fn effective(&self) -> EffectiveSettings {
        EffectiveSettings {
            cwd: self
                .cwd
                .as_ref()
                .map(|path| path.to_string_lossy().to_string()),
            model: self.model.clone(),
            reasoning_effort: self.reasoning_effort.clone(),
            permission_mode: self.permission_mode.clone(),
            sandbox: None,
            approval_policy: self.permission_mode.clone().map(Value::String),
        }
    }
}
