use crate::gateway::GatewayEndpoint;
use crate::gateway_acp::errors::ProtocolFailure;
use crate::gateway_acp::params::text_param;
use crate::policy::DEFAULT_PORT;
use crate::port::gateway;
use licoup_agent_targets::platform::user_shell_environment;
use licoup_agent_targets::platform::virtual_machine::SshRuntimeConnection;
use licoup_foundation::platform::process_supervisor::SupervisedChild;
use serde_json::Value;
use std::io;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

pub(super) const ATTACH_ARGS_PREFIX: &[&str] = &["acp", "--url"];

#[derive(Debug)]
pub(super) struct LaunchSpec {
    pub(super) executable: String,
    pub(super) args: Vec<String>,
    pub(super) cwd: PathBuf,
    pub(super) runtime_connection: Option<SshRuntimeConnection>,
}

impl LaunchSpec {
    pub(super) fn for_gateway_attach(executable: &str, cwd: &Path, gateway_ws_url: &str) -> Self {
        Self {
            executable: executable.to_string(),
            args: ATTACH_ARGS_PREFIX
                .iter()
                .map(|value| (*value).to_string())
                .chain(std::iter::once(gateway_ws_url.to_string()))
                .collect(),
            cwd: cwd.to_path_buf(),
            runtime_connection: None,
        }
    }

    pub(super) fn for_virtual_machine(
        runtime_connection: &SshRuntimeConnection,
        cwd: &Path,
    ) -> Self {
        Self {
            executable: "ssh".to_string(),
            args: Vec::new(),
            cwd: cwd.to_path_buf(),
            runtime_connection: Some(runtime_connection.clone()),
        }
    }

    pub(super) fn spawn(&self) -> io::Result<SupervisedChild> {
        let mut command = match &self.runtime_connection {
            Some(connection) => connection
                .launch_acp_command("openclaw")
                .map_err(io::Error::other)?,
            None => {
                let mut command = Command::new(&self.executable);
                user_shell_environment::apply_to_command(&mut command);
                command.args(&self.args).current_dir(&self.cwd);
                // Keep the optional token out of argv and all projected diagnostics.
                if let Ok(token) = std::env::var("OPENCLAW_GATEWAY_TOKEN")
                    && !token.trim().is_empty()
                {
                    command.env("OPENCLAW_GATEWAY_TOKEN", token);
                }
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

/// Resolve the endpoint one attach names.
///
/// An explicit request parameter is the caller's own endpoint and is read here;
/// otherwise the client's Gateway lifecycle is asked to ensure one. That engine
/// is reached through [`gateway`], so this module states which parameters name
/// an endpoint and what each engine failure means, and owns no socket, no state
/// document and no process of its own.
pub(super) fn resolve_gateway_endpoint(
    executable: &str,
    params: &Value,
) -> Result<GatewayEndpoint, ProtocolFailure> {
    if let Some(ws_url) = text_param(params, &["gatewayWsUrl", "gatewayUrl", "wsUrl"]) {
        return Ok(explicit_gateway_endpoint(&ws_url));
    }
    gateway::ensure_attach_endpoint(executable).map_err(|error| {
        let code = error.to_string();
        let failure_code = if code.contains("openclaw_executable_missing") {
            "openclaw_executable_missing"
        } else if code.contains("port_exhausted") {
            "openclaw_gateway_port_exhausted"
        } else if code.contains("health_failed") {
            "openclaw_gateway_health_failed"
        } else {
            "openclaw_gateway_unavailable"
        };
        ProtocolFailure::new(
            failure_code,
            "OpenClaw Gateway is not available for attach.",
            "gateway/ensure",
        )
    })
}

fn explicit_gateway_endpoint(ws_url: &str) -> GatewayEndpoint {
    let trimmed = ws_url.trim().to_string();
    let http = if trimmed.starts_with("ws://") {
        format!("http://{}", trimmed.trim_start_matches("ws://"))
    } else if trimmed.starts_with("wss://") {
        format!("https://{}", trimmed.trim_start_matches("wss://"))
    } else {
        trimmed.replace("ws://", "http://")
    };
    let port = http
        .rsplit(':')
        .next()
        .and_then(|value| value.trim_end_matches('/').parse::<u16>().ok())
        .unwrap_or(DEFAULT_PORT);
    GatewayEndpoint {
        host: "127.0.0.1".to_string(),
        port,
        attach_url: http,
        ws_url: if trimmed.starts_with("ws") {
            trimmed
        } else {
            format!("ws://{}", trimmed.trim_start_matches("http://"))
        },
    }
}
