use super::capabilities::AcpSessionDriverSpec;
use super::errors::ProtocolFailure;
use licoup_agent_targets::platform::virtual_machine::SshRuntimeConnection;
use licoup_agent_targets::platform::virtual_machine::is_absolute_acp_working_directory;
use licoup_foundation::platform::process_supervisor::SupervisedChild;
use serde_json::Value;
use std::io;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use uuid::Uuid;

#[derive(Clone, Debug)]
pub struct ProtocolConfig {
    pub prompt: String,
    pub requested_session_id: String,
    pub cwd: String,
    pub model: Option<String>,
    pub turn_id: String,
    pub mcp_servers: Vec<Value>,
}

impl ProtocolConfig {
    pub fn from_params(
        params: &Value,
        prompt: &str,
        session_id: &str,
        cwd: Option<&Path>,
    ) -> Result<Self, ProtocolFailure> {
        if prompt.trim().is_empty() {
            return Err(ProtocolFailure::new(
                "hermes_empty_prompt",
                "Hermes Agent requires a non-empty message.",
                "request/validate",
            ));
        }
        if params
            .get("privateInstructions")
            .and_then(Value::as_str)
            .is_some_and(|value| !value.is_empty())
        {
            return Err(ProtocolFailure::new(
                "hermes_acp_private_instructions_unsupported",
                "Hermes ACP does not expose a private instruction channel.",
                "capability/private-instructions",
            ));
        }
        if text_param(params, &["reasoningEffort", "reasoning_effort"]).is_some() {
            return Err(ProtocolFailure::new(
                "hermes_acp_reasoning_override_unsupported",
                "Hermes ACP does not expose a per-session reasoning-effort override.",
                "capability/reasoning",
            ));
        }
        if explicit_value(params, &["sandbox", "sandboxMode"]).is_some() {
            return Err(ProtocolFailure::new(
                "hermes_acp_sandbox_override_unsupported",
                "Hermes ACP inherits the native sandbox configuration and has no per-turn override.",
                "capability/sandbox",
            ));
        }
        if explicit_value(params, &["approvalPolicy", "approval_policy"]).is_some() {
            return Err(ProtocolFailure::new(
                "hermes_acp_approval_override_unsupported",
                "Hermes ACP approvals require an explicit client approval response.",
                "capability/approval",
            ));
        }
        let cwd = cwd
            .filter(|path| is_absolute_acp_working_directory(path))
            .map(|path| path.to_string_lossy().to_string())
            .ok_or_else(|| {
                ProtocolFailure::new(
                    "hermes_acp_absolute_cwd_required",
                    "Hermes ACP requires an absolute working directory.",
                    "request/validate",
                )
            })?;
        Ok(Self {
            prompt: prompt.to_string(),
            requested_session_id: session_id.trim().to_string(),
            cwd,
            model: text_param(params, &["model", "modelId"]),
            turn_id: Uuid::new_v4().to_string(),
            mcp_servers: Vec::new(),
        })
    }

    pub fn is_resume(&self) -> bool {
        !self.requested_session_id.is_empty()
    }

    pub fn load_collaboration_mcp(&mut self, runtime_id: &str) -> Result<(), ProtocolFailure> {
        self.mcp_servers = crate::runtime_adapters::port::collaboration_acp_servers(runtime_id)
            .map_err(|_| {
                ProtocolFailure::new(
                    "hermes_acp_mcp_registration_invalid",
                    "The optional MCP registration could not be validated safely.",
                    "session/mcp",
                )
            })?;
        Ok(())
    }
}

#[derive(Debug)]
pub struct LaunchSpec {
    pub executable: String,
    pub driver: AcpSessionDriverSpec,
    pub cwd: PathBuf,
    pub runtime_connection: Option<SshRuntimeConnection>,
}

impl LaunchSpec {
    pub fn new(driver: AcpSessionDriverSpec, executable: &str, cwd: &Path) -> Self {
        Self {
            executable: executable.to_string(),
            driver,
            cwd: cwd.to_path_buf(),
            runtime_connection: None,
        }
    }

    pub fn with_runtime_connection(
        mut self,
        runtime_connection: Option<SshRuntimeConnection>,
    ) -> Self {
        self.runtime_connection = runtime_connection;
        self
    }

    pub fn spawn(&self) -> io::Result<SupervisedChild> {
        let mut command = match &self.runtime_connection {
            Some(connection) => connection
                .launch_acp_command(self.driver.runtime_id)
                .map_err(io::Error::other)?,
            None => {
                let mut command = Command::new(&self.executable);
                licoup_agent_targets::platform::user_shell_environment::apply_to_command(
                    &mut command,
                );
                command.args(self.driver.launch_args).current_dir(&self.cwd);
                command
            }
        };
        command
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        SupervisedChild::spawn(&mut command)
    }
}

fn explicit_value<'a>(params: &'a Value, keys: &[&str]) -> Option<&'a Value> {
    keys.iter()
        .find_map(|key| params.get(*key))
        .filter(|value| !value.is_null())
}

fn text_param(params: &Value, keys: &[&str]) -> Option<String> {
    keys.iter()
        .find_map(|key| params.get(*key).and_then(Value::as_str))
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
}
