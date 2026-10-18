use anyhow::{Result, anyhow};
use serde_json::{Value, json};
use std::io;
use std::process::{Command, Stdio};
use std::thread;

use super::bounds::{PROCESS_FORCE_POLLS, PROCESS_GRACEFUL_POLLS, PROCESS_POLL_INTERVAL};
use super::state::{self, ServicePaths};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SpawnFailure {
    Missing,
    PermissionDenied,
    Start,
}

pub trait ServeRunner: Send + Sync {
    fn spawn(
        &self,
        executable: &str,
        host: &str,
        port: u16,
        configure: fn(&mut Command, &str, u16),
    ) -> Result<u32, SpawnFailure>;
}

pub struct CommandServeRunner;

impl ServeRunner for CommandServeRunner {
    fn spawn(
        &self,
        executable: &str,
        host: &str,
        port: u16,
        configure: fn(&mut Command, &str, u16),
    ) -> Result<u32, SpawnFailure> {
        let mut command = Command::new(executable);
        licoup_agent_targets::platform::user_shell_environment::apply_to_command(&mut command);
        configure(&mut command, host, port);
        spawn_detached(&mut command)
    }
}

pub fn spawn_detached(command: &mut Command) -> Result<u32, SpawnFailure> {
    command
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        command.process_group(0);
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt as WindowsCommandExt;
        command.creation_flags(0x0000_0008 | 0x0000_0200 | 0x0800_0000);
    }
    command
        .spawn()
        .map(|child| child.id())
        .map_err(|error| match error.kind() {
            io::ErrorKind::NotFound => SpawnFailure::Missing,
            io::ErrorKind::PermissionDenied => SpawnFailure::PermissionDenied,
            _ => SpawnFailure::Start,
        })
}

pub fn stop(paths: &ServicePaths, stop_failure: &'static str) -> Result<Value> {
    let pid = state::read_pid(&paths.pid_path)?;
    let mut touched = false;
    let mut forced = false;
    if let Some(pid) = pid
        && alive(Some(pid))
    {
        touched = true;
        if terminate(pid, false).is_err() {
            forced = true;
            let _ = terminate(pid, true);
        }
        for _ in 0..PROCESS_GRACEFUL_POLLS {
            if !alive(Some(pid)) {
                break;
            }
            thread::sleep(PROCESS_POLL_INTERVAL);
        }
        if alive(Some(pid)) {
            forced = true;
            let _ = terminate(pid, true);
            for _ in 0..PROCESS_FORCE_POLLS {
                if !alive(Some(pid)) {
                    break;
                }
                thread::sleep(PROCESS_POLL_INTERVAL);
            }
        }
        if alive(Some(pid)) {
            return Err(anyhow!(stop_failure));
        }
    }
    let _ = state::remove_pid(&paths.pid_path);
    Ok(json!({
        "ok": true,
        "status": if touched { "stopped" } else { "not-running" },
        "pid": pid.unwrap_or(0),
        "forced": forced
    }))
}

pub fn terminate_owned(pid: u32) {
    let _ = terminate(pid, false);
    for _ in 0..PROCESS_GRACEFUL_POLLS {
        if !alive(Some(pid)) {
            return;
        }
        thread::sleep(PROCESS_POLL_INTERVAL);
    }
    let _ = terminate(pid, true);
}

pub fn alive(pid: Option<u32>) -> bool {
    let Some(pid) = pid.filter(|pid| *pid != 0) else {
        return false;
    };
    #[cfg(unix)]
    {
        Command::new("kill")
            .arg("-0")
            .arg(pid.to_string())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .map(|status| status.success())
            .unwrap_or(false)
    }
    #[cfg(windows)]
    {
        Command::new("tasklist")
            .args(["/FI", &format!("PID eq {}", pid)])
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .output()
            .map(|output| {
                output.status.success()
                    && output.stdout.len() <= 64 * 1024
                    && String::from_utf8_lossy(&output.stdout).contains(&pid.to_string())
            })
            .unwrap_or(false)
    }
}

fn terminate(pid: u32, force: bool) -> Result<()> {
    #[cfg(unix)]
    {
        let signal = if force { "-KILL" } else { "-TERM" };
        let status = Command::new("kill")
            .arg(signal)
            .arg(pid.to_string())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()?;
        if status.success() {
            Ok(())
        } else {
            Err(anyhow!("local_service_terminate_failed"))
        }
    }
    #[cfg(windows)]
    {
        let mut args = vec!["/PID".to_string(), pid.to_string(), "/T".to_string()];
        if force {
            args.push("/F".to_string());
        }
        let status = Command::new("taskkill")
            .args(args)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()?;
        if status.success() {
            Ok(())
        } else {
            Err(anyhow!("local_service_terminate_failed"))
        }
    }
}
