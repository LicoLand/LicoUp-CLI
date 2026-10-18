//! Detached execution of the generated native update scripts.

use std::{
    fs,
    process::{Command, Stdio},
};

use anyhow::{Context, Result, ensure};

use super::plan::ApplyPlan;

/// Spawns the generated script detached (or synchronously when `plan.wait` is
/// set by tests). The script survives the CLI exit and performs the
/// exit-wait, replacement and relaunch on its own schedule.
pub(super) fn spawn_apply_script(plan: &ApplyPlan, argv: &[String]) -> Result<()> {
    let log = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&plan.log_path)
        .context("failed to open client update script log")?;
    let stderr = log
        .try_clone()
        .context("failed to clone client update script log")?;
    #[cfg(unix)]
    {
        let mut command = Command::new("/bin/sh");
        command
            .arg(&plan.script_path)
            .current_dir(std::env::temp_dir())
            .stdout(Stdio::from(log))
            .stderr(Stdio::from(stderr));
        for value in argv {
            command.arg(value);
        }
        let mut child = command
            .spawn()
            .context("failed to spawn client update script")?;
        if plan.wait {
            let status = child
                .wait()
                .context("failed to wait for client update script")?;
            ensure!(
                status.success(),
                "client update script exited with {status}"
            );
        }
        Ok(())
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;

        let mut command = Command::new(resolve_powershell()?);
        command
            .args(["-NoProfile", "-ExecutionPolicy", "Bypass", "-File"])
            .arg(&plan.script_path)
            .current_dir(std::env::temp_dir())
            .stdout(Stdio::from(log))
            .stderr(Stdio::from(stderr))
            // CREATE_NO_WINDOW | DETACHED_PROCESS
            .creation_flags(0x0800_0008);
        for value in argv {
            command.arg(value);
        }
        let mut child = command
            .spawn()
            .context("failed to spawn client update script")?;
        if plan.wait {
            let status = child
                .wait()
                .context("failed to wait for client update script")?;
            ensure!(
                status.success(),
                "client update script exited with {status}"
            );
        }
        Ok(())
    }
}

#[cfg(windows)]
fn resolve_powershell() -> Result<std::path::PathBuf> {
    if let Some(system_root) = std::env::var_os("SystemRoot") {
        let candidate = std::path::Path::new(&system_root)
            .join("System32")
            .join("WindowsPowerShell")
            .join("v1.0")
            .join("powershell.exe");
        if candidate.is_file() {
            return Ok(candidate);
        }
    }
    Ok(std::path::PathBuf::from("powershell.exe"))
}
