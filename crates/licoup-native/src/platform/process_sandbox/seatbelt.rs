use anyhow::{Result, anyhow};
use std::path::Path;
use std::process::Command;

pub const CAPABILITY_COLLABORATION_LOOPBACK: &str = "platform-loopback-isolated-runtime-v1";

/// The platform's sandbox runner.
const SANDBOX_EXEC: &str = "/usr/bin/sandbox-exec";

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub enum SandboxError {
    Unavailable,
    PathInvalid,
}

impl SandboxError {
    fn collaboration_code(self) -> &'static str {
        match self {
            Self::Unavailable => "collaboration_local_server_reliable_sandbox_unavailable",
            Self::PathInvalid => "collaboration_local_server_sandbox_path_invalid",
        }
    }
}

/// Escape an absolute path for inclusion in a seatbelt profile literal.
pub fn seatbelt_literal(path: &Path) -> Result<String, SandboxError> {
    if !path.is_absolute() {
        return Err(SandboxError::PathInvalid);
    }
    let value = path.to_str().ok_or(SandboxError::PathInvalid)?;
    if value.chars().any(char::is_control) {
        return Err(SandboxError::PathInvalid);
    }
    Ok(value.replace('\\', "\\\\").replace('"', "\\\""))
}

#[cfg(target_os = "macos")]
fn verify_sandbox_exec() -> Result<(), SandboxError> {
    use std::os::unix::fs::{MetadataExt, PermissionsExt};

    const SANDBOX_EXEC: &str = "/usr/bin/sandbox-exec";
    let metadata =
        std::fs::symlink_metadata(SANDBOX_EXEC).map_err(|_| SandboxError::Unavailable)?;
    if !(metadata.is_file()
        && !metadata.file_type().is_symlink()
        && metadata.uid() == 0
        && metadata.permissions().mode() & 0o022 == 0)
    {
        return Err(SandboxError::Unavailable);
    }
    Ok(())
}

/// Collaboration local-server profile: write under `runtime_data`, bind/inbound on port.
pub fn collaboration_loopback_command(
    runner: &Path,
    manifest: &Path,
    snapshot: &Path,
    runtime_data: &Path,
    port: u16,
) -> Result<Command> {
    #[cfg(target_os = "macos")]
    {
        verify_sandbox_exec().map_err(|e| anyhow!(e.collaboration_code()))?;
        let runner_l = seatbelt_literal(runner).map_err(|e| anyhow!(e.collaboration_code()))?;
        let manifest_l = seatbelt_literal(manifest).map_err(|e| anyhow!(e.collaboration_code()))?;
        let snapshot_l = seatbelt_literal(snapshot).map_err(|e| anyhow!(e.collaboration_code()))?;
        let runtime_l =
            seatbelt_literal(runtime_data).map_err(|e| anyhow!(e.collaboration_code()))?;
        let profile = format!(
            concat!(
                "(version 1)",
                "(deny default)",
                "(import \"system.sb\")",
                "(allow process-exec (literal \"{runner}\"))",
                "(allow signal (target self))",
                "(allow file-read* file-test-existence ",
                "(literal \"{runner}\") (literal \"{manifest}\") (literal \"{snapshot}\") ",
                "(subpath \"{runtime_data}\"))",
                "(allow file-write* (subpath \"{runtime_data}\"))",
                "(allow network-bind (local tcp \"localhost:{port}\"))",
                "(allow network-inbound (local tcp \"localhost:{port}\"))"
            ),
            runner = runner_l,
            manifest = manifest_l,
            snapshot = snapshot_l,
            runtime_data = runtime_l,
            port = port,
        );
        let mut command = Command::new(SANDBOX_EXEC);
        command.args(["-p", &profile]).arg(runner);
        return Ok(command);
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = (runner, manifest, snapshot, runtime_data, port);
        Err(anyhow!(SandboxError::Unavailable.collaboration_code()))
    }
}

/// The platform's sandboxed invocation of one runner under one sealed profile.
///
/// The profile is the caller's — what it binds and why belongs to the Agent that
/// asked for it — and this primitive owns only three things: the runner, the way
/// a profile reaches it, and the rule that a profile this platform cannot
/// enforce is refused rather than run unsandboxed.
pub fn sandboxed_command(
    profile: &str,
    runner: &Path,
    extra_args: &[String],
) -> Result<Command, SandboxError> {
    #[cfg(target_os = "macos")]
    {
        verify_sandbox_exec()?;
        let mut command = Command::new(SANDBOX_EXEC);
        command.args(["-p", profile]).arg(runner);
        command.args(extra_args);
        Ok(command)
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = (profile, runner, extra_args);
        Err(SandboxError::Unavailable)
    }
}

#[cfg(all(test, target_os = "macos"))]
pub(crate) use tests::sandbox_exec_can_apply;

#[cfg(all(test, target_os = "macos"))]
mod tests {
    use super::*;
    use std::fs;
    use uuid::Uuid;

    pub(crate) fn sandbox_exec_can_apply() -> bool {
        // Outer CI/dev sandboxes can allow sandbox-exec while still denying
        // nested writes under /var/folders. Probe a literal write before
        // asserting Plan profile allow/deny behavior.
        let probe_root = std::env::temp_dir().join(format!("licoup-sb-probe-{}", Uuid::new_v4()));
        let _ = fs::create_dir_all(&probe_root);
        let probe_file = probe_root.join("probe.txt");
        let _ = fs::write(&probe_file, b"");
        let Ok(literal) = seatbelt_literal(&probe_file) else {
            let _ = fs::remove_dir_all(&probe_root);
            return false;
        };
        let profile = format!(
            "(version 1)(deny default)(import \"system.sb\")(allow process-exec (literal \"/usr/bin/tee\"))(allow file-read* file-test-existence (literal \"/usr/bin/tee\") (literal \"{literal}\"))(allow file-write* (literal \"{literal}\"))"
        );
        let mut command = std::process::Command::new("/usr/bin/sandbox-exec");
        command
            .args(["-p", &profile, "/usr/bin/tee"])
            .arg(&probe_file)
            .stdin(std::process::Stdio::piped());
        let ok = command
            .spawn()
            .ok()
            .and_then(|mut child| {
                use std::io::Write;
                child.stdin.as_mut()?.write_all(b"ok").ok()?;
                child.wait().ok()
            })
            .map(|status| {
                status.success() && fs::read_to_string(&probe_file).ok().as_deref() == Some("ok")
            })
            .unwrap_or(false);
        let _ = fs::remove_dir_all(probe_root);
        ok
    }

    #[test]
    fn collaboration_profile_runs_declared_runner() {
        if !sandbox_exec_can_apply() {
            return;
        }
        let root = std::env::temp_dir().join(format!("licoup-sb-collab-{}", Uuid::new_v4()));
        fs::create_dir(&root).unwrap();
        let manifest = root.join("manifest.json");
        let snapshot = root.join("snapshot.bin");
        let runtime_data = root.join("runtime-data");
        fs::write(&manifest, b"{}").unwrap();
        fs::write(&snapshot, b"snapshot").unwrap();
        fs::create_dir(&runtime_data).unwrap();
        let status = collaboration_loopback_command(
            Path::new("/usr/bin/true"),
            &manifest,
            &snapshot,
            &runtime_data,
            32_345,
        )
        .unwrap()
        .status()
        .unwrap();
        assert!(status.success());
        let _ = fs::remove_dir_all(root);
    }
}
