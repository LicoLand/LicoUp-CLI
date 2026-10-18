use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::fmt;
use std::path::Path;
use std::process::Command;

const MAX_HOST_BYTES: usize = 255;
const MAX_USER_BYTES: usize = 255;
const MAX_EXECUTABLE_BYTES: usize = 1024;
const MAX_WORKING_DIRECTORY_BYTES: usize = 4096;
const SSH_CONNECT_TIMEOUT_SECONDS: u8 = 10;

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
enum RuntimeProtocol {
    HermesTuiGateway,
}

#[derive(Clone, Debug, Eq, Hash, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct SshRuntimeConnection {
    kind: String,
    host: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    port: Option<u16>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    user: Option<String>,
    remote_executable: String,
    working_directory: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    runtime_protocol: Option<RuntimeProtocol>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RuntimeConnectionError {
    UnsupportedTarget,
    InvalidShape,
    InvalidKind,
    InvalidHost,
    InvalidPort,
    InvalidUser,
    InvalidExecutable,
    InvalidWorkingDirectory,
    InvalidProtocol,
}

impl fmt::Display for RuntimeConnectionError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.code())
    }
}

impl std::error::Error for RuntimeConnectionError {}

impl RuntimeConnectionError {
    pub const fn code(self) -> &'static str {
        match self {
            Self::UnsupportedTarget => "virtual_machine_target_unsupported",
            Self::InvalidShape => "virtual_machine_connection_invalid",
            Self::InvalidKind => "virtual_machine_connection_kind_invalid",
            Self::InvalidHost => "virtual_machine_host_invalid",
            Self::InvalidPort => "virtual_machine_port_invalid",
            Self::InvalidUser => "virtual_machine_user_invalid",
            Self::InvalidExecutable => "virtual_machine_executable_invalid",
            Self::InvalidWorkingDirectory => "virtual_machine_working_directory_invalid",
            Self::InvalidProtocol => "virtual_machine_runtime_protocol_invalid",
        }
    }
}

impl SshRuntimeConnection {
    pub fn from_params(
        params: &Value,
        target: &str,
    ) -> Result<Option<Self>, RuntimeConnectionError> {
        Self::from_value(params.get("runtimeConnection"), target)
    }

    pub fn from_value(
        value: Option<&Value>,
        target: &str,
    ) -> Result<Option<Self>, RuntimeConnectionError> {
        let Some(value) = value.filter(|value| !value.is_null()) else {
            return Ok(None);
        };
        if !supports_virtual_machine_target(target) {
            return Err(RuntimeConnectionError::UnsupportedTarget);
        }
        let connection: Self = serde_json::from_value(value.clone())
            .map_err(|_| RuntimeConnectionError::InvalidShape)?;
        connection.validate(target)?;
        Ok(Some(connection))
    }

    pub fn to_value(&self) -> Value {
        serde_json::to_value(self).unwrap_or(Value::Null)
    }

    pub fn remote_executable(&self) -> &str {
        &self.remote_executable
    }

    pub fn working_directory(&self) -> &str {
        &self.working_directory
    }

    pub fn is_hermes_tui_gateway(&self) -> bool {
        self.runtime_protocol == Some(RuntimeProtocol::HermesTuiGateway)
    }

    pub fn launch_acp_command(&self, target: &str) -> Result<Command, RuntimeConnectionError> {
        self.validate(target)?;
        if self.runtime_protocol.is_some() {
            return Err(RuntimeConnectionError::InvalidProtocol);
        }
        self.launch_ssh_command(&["acp"])
    }

    pub fn launch_hermes_tui_gateway_command(&self) -> Result<Command, RuntimeConnectionError> {
        self.validate("hermes")?;
        if !self.is_hermes_tui_gateway() {
            return Err(RuntimeConnectionError::InvalidProtocol);
        }
        self.launch_ssh_command(&["-u", "-m", "tui_gateway.entry"])
    }

    fn launch_ssh_command(&self, remote_args: &[&str]) -> Result<Command, RuntimeConnectionError> {
        let destination = match self.user.as_deref() {
            Some(user) => format!("{user}@{}", self.host),
            None => self.host.clone(),
        };
        let remote_args = remote_args
            .iter()
            .map(|value| posix_shell_quote(value))
            .collect::<Vec<_>>()
            .join(" ");
        let remote_command = format!(
            "cd {} && exec {} {}",
            posix_shell_quote(&self.working_directory),
            posix_shell_quote(&self.remote_executable),
            remote_args,
        );
        let mut command = Command::new("ssh");
        // The local ssh client observes the user shell environment (proxy,
        // agent socket, PATH); `SendEnv=-*` keeps it from crossing the wire.
        crate::platform::user_shell_environment::apply_to_command(&mut command);
        command
            .arg("-T")
            .args(["-o", "BatchMode=yes"])
            .args(["-o", "StrictHostKeyChecking=yes"])
            .args(["-o", "ClearAllForwardings=yes"])
            .args(["-o", "ForwardAgent=no"])
            .args(["-o", "ForwardX11=no"])
            .args(["-o", "ForwardX11Trusted=no"])
            .args(["-o", "PermitLocalCommand=no"])
            .args(["-o", "ControlMaster=no"])
            .args(["-o", "ControlPath=none"])
            .args(["-o", "ControlPersist=no"])
            .args(["-o", "SendEnv=-*"])
            .args(["-o", "RequestTTY=no"])
            .args(["-o", "NumberOfPasswordPrompts=0"])
            .arg("-o")
            .arg(format!("ConnectTimeout={SSH_CONNECT_TIMEOUT_SECONDS}"));
        if let Some(port) = self.port {
            command.arg("-p").arg(port.to_string());
        }
        command.arg(destination).arg(remote_command);
        Ok(command)
    }

    fn validate(&self, target: &str) -> Result<(), RuntimeConnectionError> {
        if !supports_virtual_machine_target(target) {
            return Err(RuntimeConnectionError::UnsupportedTarget);
        }
        if self.kind != "ssh" {
            return Err(RuntimeConnectionError::InvalidKind);
        }
        if !valid_host(&self.host) {
            return Err(RuntimeConnectionError::InvalidHost);
        }
        if self.port == Some(0) {
            return Err(RuntimeConnectionError::InvalidPort);
        }
        if self.user.as_deref().is_some_and(|user| !valid_user(user)) {
            return Err(RuntimeConnectionError::InvalidUser);
        }
        if !valid_remote_executable(&self.remote_executable) {
            return Err(RuntimeConnectionError::InvalidExecutable);
        }
        if !valid_remote_working_directory(&self.working_directory) {
            return Err(RuntimeConnectionError::InvalidWorkingDirectory);
        }
        if self.runtime_protocol.is_some() && target != "hermes" {
            return Err(RuntimeConnectionError::InvalidProtocol);
        }
        Ok(())
    }
}

pub fn supports_virtual_machine_target(target: &str) -> bool {
    matches!(target, "openclaw" | "hermes")
}

pub fn is_absolute_acp_working_directory(path: &Path) -> bool {
    path.is_absolute() || path.to_str().is_some_and(valid_remote_working_directory)
}

pub fn is_valid_guest_working_directory(path: &Path) -> bool {
    path.to_str().is_some_and(valid_remote_working_directory)
}

fn valid_host(value: &str) -> bool {
    bounded_trimmed(value, MAX_HOST_BYTES)
        && !value.starts_with('-')
        && value.bytes().all(|byte| {
            byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'-' | b'_' | b':' | b'[' | b']')
        })
}

fn valid_user(value: &str) -> bool {
    bounded_trimmed(value, MAX_USER_BYTES)
        && !value.starts_with('-')
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'-' | b'_'))
}

fn valid_remote_executable(value: &str) -> bool {
    bounded_trimmed(value, MAX_EXECUTABLE_BYTES)
        && !value.starts_with('-')
        && !value
            .bytes()
            .any(|byte| matches!(byte, b'\0' | b'\n' | b'\r'))
}

fn valid_remote_working_directory(value: &str) -> bool {
    bounded_trimmed(value, MAX_WORKING_DIRECTORY_BYTES)
        && value.starts_with('/')
        && !value
            .bytes()
            .any(|byte| matches!(byte, b'\0' | b'\n' | b'\r'))
}

fn bounded_trimmed(value: &str, max_bytes: usize) -> bool {
    !value.is_empty() && value.len() <= max_bytes && value.trim() == value
}

fn posix_shell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn guest_path(segments: &[&str]) -> String {
        format!("/{}", segments.join("/"))
    }

    fn fixture() -> SshRuntimeConnection {
        let executable = guest_path(&["opt", "hermes agent", "bin", "hermes"]);
        let working_directory = guest_path(&["srv", "project's workspace"]);
        SshRuntimeConnection::from_value(
            Some(&json!({
                "kind": "ssh",
                "host": "vm.example",
                "port": 2222,
                "user": "agent-user",
                "remoteExecutable": executable,
                "workingDirectory": working_directory
            })),
            "hermes",
        )
        .unwrap()
        .unwrap()
    }

    #[test]
    fn ssh_launch_is_batch_only_and_shell_quotes_remote_values() {
        let command = fixture().launch_acp_command("hermes").unwrap();
        assert_eq!(command.get_program(), "ssh");
        let args = command
            .get_args()
            .map(|value| value.to_string_lossy().into_owned())
            .collect::<Vec<_>>();
        assert!(args.windows(2).any(|pair| pair == ["-o", "BatchMode=yes"]));
        assert!(
            args.windows(2)
                .any(|pair| pair == ["-o", "StrictHostKeyChecking=yes"])
        );
        assert!(
            args.windows(2)
                .any(|pair| pair == ["-o", "ForwardAgent=no"])
        );
        assert!(
            args.windows(2)
                .any(|pair| pair == ["-o", "ControlMaster=no"])
        );
        assert!(args.windows(2).any(|pair| pair == ["-o", "SendEnv=-*"]));
        assert_eq!(args[args.len() - 2], "agent-user@vm.example");
        let expected_working_directory =
            posix_shell_quote(&guest_path(&["srv", "project's workspace"]));
        let expected_executable =
            posix_shell_quote(&guest_path(&["opt", "hermes agent", "bin", "hermes"]));
        assert_eq!(
            args.last().unwrap(),
            &format!("cd {expected_working_directory} && exec {expected_executable} 'acp'")
        );
    }

    #[test]
    fn hermes_gateway_launch_is_fixed_and_target_scoped() {
        let executable = guest_path(&["opt", "hermes", "venv", "bin", "python"]);
        let working_directory = guest_path(&["srv", "project"]);
        let connection = SshRuntimeConnection::from_value(
            Some(&json!({
                "kind": "ssh",
                "host": "vm.example",
                "remoteExecutable": executable,
                "workingDirectory": working_directory,
                "runtimeProtocol": "hermes-tui-gateway"
            })),
            "hermes",
        )
        .unwrap()
        .unwrap();

        assert!(connection.is_hermes_tui_gateway());
        assert!(connection.launch_acp_command("hermes").is_err());
        let command = connection.launch_hermes_tui_gateway_command().unwrap();
        let args = command
            .get_args()
            .map(|value| value.to_string_lossy().into_owned())
            .collect::<Vec<_>>();
        assert_eq!(
            args.last().unwrap(),
            &format!(
                "cd {} && exec {} '-u' '-m' 'tui_gateway.entry'",
                posix_shell_quote(&guest_path(&["srv", "project"])),
                posix_shell_quote(&guest_path(&["opt", "hermes", "venv", "bin", "python"]))
            )
        );

        let value = connection.to_value();
        assert_eq!(value["runtimeProtocol"], "hermes-tui-gateway");
        assert!(SshRuntimeConnection::from_value(Some(&value), "openclaw").is_err());
    }

    #[test]
    fn connection_rejects_credentials_and_command_injection_shapes() {
        let working_directory = guest_path(&["srv", "project"]);
        assert!(
            SshRuntimeConnection::from_value(
                Some(&json!({
                    "kind": "ssh",
                    "host": "-oProxyCommand=bad",
                    "remoteExecutable": "hermes",
                    "workingDirectory": working_directory
                })),
                "hermes"
            )
            .is_err()
        );
        let mut credential_shape = json!({
            "kind": "ssh",
            "host": "vm.example",
            "remoteExecutable": "hermes",
            "workingDirectory": guest_path(&["srv", "project"])
        });
        credential_shape.as_object_mut().unwrap().insert(
            ["pass", "word"].concat(),
            Value::String(["synthetic", "value"].join("-")),
        );
        assert!(SshRuntimeConnection::from_value(Some(&credential_shape), "hermes").is_err());
    }

    #[test]
    fn guest_working_directories_are_posix_absolute_and_bounded() {
        assert!(is_valid_guest_working_directory(Path::new(&guest_path(&[
            "srv", "project"
        ]))));
        assert!(!is_valid_guest_working_directory(Path::new("project")));
        assert!(!is_valid_guest_working_directory(Path::new(
            &["C:", "workspace"].join("\\")
        )));
        let line_break = format!("{}\nother", guest_path(&["srv", "project"]));
        assert!(!is_valid_guest_working_directory(Path::new(&line_break)));
    }
}
