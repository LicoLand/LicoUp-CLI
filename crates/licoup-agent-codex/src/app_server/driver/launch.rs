//! Starting one official Codex app-server for one turn.
//!
//! The program is the installed Codex client, its arguments are the app-server
//! stdio mode, and thread continuity belongs to the server's own `thread.id`.
//! There is no parallel daemon and no prompt-bearing argv channel: the package
//! writes the prompt over the protocol it owns.
//!
//! Two environment answers are the caller's, not this module's:
//!
//! - **The child's whole environment.** [`CodexLaunchSpec::with_environment`]
//!   binds exactly the set the caller names and clears everything else, which is
//!   what a launch from the client (the user's own login shell) and a launch
//!   from a test both need. A spec with no environment leaves the child the
//!   package process's own environment — the honest answer for a package started
//!   by the extension host under a scrubbed envelope.
//! - **The portable LicoUp root.** It is resolved from this process's own
//!   selection, never from a captured shell value, so the root the child sees is
//!   the root the client is really using.
//!
//! What this module does add on its own is the delegation context: a turn that
//! the Subagent mesh dispatched exports four caller identifiers, and the
//! provider-spawned child reads them under the names the released Subagent MCP
//! server allowlists.

use licoup_foundation::platform::process_supervisor::SupervisedChild;
use serde_json::Value;
use std::ffi::OsString;
use std::io;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

/// The delegation context a dispatched turn exports, as parameter name to
/// child variable name.
///
/// A value is a validated opaque identifier and is never persisted here.
const SUBAGENT_CALLER_CONTEXT: &[(&str, &str)] = &[
    ("agentId", "LICOUP_MCP_CALLER_PROVIDER"),
    ("conversationId", "LICOUP_MCP_CONVERSATION_ID"),
    ("membershipId", "LICOUP_MCP_MEMBERSHIP_ID"),
    ("parentDispatchId", "LICOUP_MCP_PARENT_DISPATCH_ID"),
];

/// One app-server launch: the program, its fixed arguments, the working
/// directory and the environment the caller binds for the child.
#[derive(Debug)]
pub struct CodexLaunchSpec {
    pub executable: String,
    pub args: Vec<String>,
    pub cwd: Option<PathBuf>,
    /// The child's whole environment, when the caller names one. `None` keeps
    /// the package process's own environment.
    pub environment: Option<Vec<(String, String)>>,
}

impl CodexLaunchSpec {
    /// The official stdio app-server owns thread continuity through its
    /// `thread.id`; no parallel daemon or prompt-bearing argv channel exists.
    pub fn new(executable: &str, cwd: Option<&Path>) -> Self {
        Self {
            executable: executable.to_string(),
            args: vec!["app-server".to_string(), "--stdio".to_string()],
            cwd: cwd.map(Path::to_path_buf),
            environment: None,
        }
    }

    /// Bind the child's whole environment.
    pub fn with_environment(mut self, environment: Vec<(String, String)>) -> Self {
        self.environment = Some(environment);
        self
    }

    pub fn spawn(&self) -> io::Result<SupervisedChild> {
        self.spawn_with_context(None)
    }

    pub fn spawn_with_context(&self, params: Option<&Value>) -> io::Result<SupervisedChild> {
        let mut command = Command::new(&self.executable);
        command
            .args(&self.args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        if let Some(cwd) = self.cwd.as_ref() {
            command.current_dir(cwd);
        }
        apply_launch_environment(&mut command, params, self.environment.as_deref())?;
        SupervisedChild::spawn(&mut command)
    }
}

/// Resolve this process's portable root and bind it, with the delegation
/// context, into the child.
pub fn apply_launch_environment(
    command: &mut Command,
    params: Option<&Value>,
    environment: Option<&[(String, String)]>,
) -> io::Result<()> {
    let root = licoup_foundation::platform::paths::selected_data_home()
        .map_err(|_| io::Error::other("cannot resolve selected LicoUp data root"))?
        .path
        .into_os_string();
    apply_launch_environment_with_root(command, params, environment, Some(root));
    Ok(())
}

pub fn apply_launch_environment_with_root(
    command: &mut Command,
    params: Option<&Value>,
    environment: Option<&[(String, String)]>,
    portable_root: Option<OsString>,
) {
    if let Some(environment) = environment {
        command.env_clear();
        for (key, value) in environment {
            command.env(key, value);
        }
    }
    command.env_remove("LICOUP_HOME");
    command.env_remove("LICOUP_PORTABLE_DIR");
    if let Some(root) = portable_root.filter(|value| !value.is_empty()) {
        command.env("LICOUP_HOME", root.clone());
        command.env("LICOUP_PORTABLE_DIR", root);
    }
    if let Some(params) = params {
        apply_subagent_caller_context(command, params);
    }
}

/// Bind the exact Membership-scoped caller context the child may later present
/// through the Subagent MCP connector.
fn apply_subagent_caller_context(command: &mut Command, params: &Value) {
    for (key, env_key) in SUBAGENT_CALLER_CONTEXT {
        if let Some(value) = params
            .get(*key)
            .and_then(Value::as_str)
            .filter(|value| valid_context_identifier(value))
        {
            command.env(env_key, value);
        }
    }
}

fn valid_context_identifier(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 256
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b':' | b'.' | b'_' | b'-'))
}
