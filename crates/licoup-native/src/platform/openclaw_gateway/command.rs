use std::path::Path;
use std::process::Command;

use super::super::local_service::process::{self, SpawnFailure};
use super::config;

pub(super) trait GatewayRunner: Send + Sync {
    fn spawn(
        &self,
        executable: &str,
        port: u16,
        runtime_dir: &Path,
        config_path: &Path,
    ) -> Result<u32, SpawnFailure>;
}

pub(super) struct CommandGatewayRunner;

impl GatewayRunner for CommandGatewayRunner {
    fn spawn(
        &self,
        executable: &str,
        port: u16,
        runtime_dir: &Path,
        config_path: &Path,
    ) -> Result<u32, SpawnFailure> {
        config::ensure_minimal(config_path, port).map_err(|_| SpawnFailure::Start)?;
        let mut command = Command::new(executable);
        super::super::user_shell_environment::apply_to_command(&mut command);
        command
            .args([
                "gateway",
                "--port",
                &port.to_string(),
                "--bind",
                "loopback",
                "--allow-unconfigured",
                "--auth",
                "none",
                "run",
            ])
            .env("OPENCLAW_STATE_DIR", runtime_dir)
            .env("OPENCLAW_CONFIG_PATH", config_path)
            .env("OPENCLAW_GATEWAY_PORT", port.to_string())
            .env_remove("OPENCLAW_GATEWAY_TOKEN")
            .env_remove("OPENCLAW_GATEWAY_PASSWORD");
        process::spawn_detached(&mut command)
    }
}
