use anyhow::{Result, anyhow, ensure};
use std::path::Path;
use std::process::Command;

#[cfg(target_os = "macos")]
use super::seatbelt_literal;

/// Build one no-network, no-shell strategy script command.
pub(crate) fn strategy_script_command(
    executable: &Path,
    runtime_root: &Path,
    script: &Path,
    revision_root: &Path,
    scratch: &Path,
) -> Result<Command> {
    ensure!(
        [executable, runtime_root, script, revision_root, scratch]
            .into_iter()
            .all(Path::is_absolute),
        "strategy_sandbox_path_invalid"
    );
    ensure!(
        script.starts_with(revision_root),
        "strategy_sandbox_path_invalid"
    );
    #[cfg(target_os = "macos")]
    {
        let sandbox = Path::new("/usr/bin/sandbox-exec");
        let metadata = std::fs::symlink_metadata(sandbox)
            .map_err(|_| anyhow!("strategy_sandbox_unavailable"))?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::{MetadataExt, PermissionsExt};
            ensure!(
                metadata.is_file()
                    && !metadata.file_type().is_symlink()
                    && metadata.uid() == 0
                    && metadata.permissions().mode() & 0o022 == 0,
                "strategy_sandbox_unavailable"
            );
        }
        let executable_path = executable.to_path_buf();
        ensure!(
            executable_path
                .to_str()
                .is_some_and(|value| !value.contains('\\') && !value.contains('"')),
            "strategy_sandbox_path_invalid"
        );
        let executable = seatbelt_literal(&executable_path)
            .map_err(|_| anyhow!("strategy_sandbox_path_invalid"))?;
        let runtime_root =
            seatbelt_literal(runtime_root).map_err(|_| anyhow!("strategy_sandbox_path_invalid"))?;
        let script_literal =
            seatbelt_literal(script).map_err(|_| anyhow!("strategy_sandbox_path_invalid"))?;
        let revision = seatbelt_literal(revision_root)
            .map_err(|_| anyhow!("strategy_sandbox_path_invalid"))?;
        let scratch_literal =
            seatbelt_literal(scratch).map_err(|_| anyhow!("strategy_sandbox_path_invalid"))?;
        let profile = format!(
            concat!(
                "(version 1)",
                "(deny default)",
                "(import \"system.sb\")",
                "(deny network*)",
                "(allow process-exec (literal \"{executable}\"))",
                "(allow signal (target self))",
                "(allow file-read* file-test-existence ",
                "(literal \"{executable}\") (literal \"{script}\") ",
                "(subpath \"{runtime_root}\") (subpath \"{revision}\"))",
                "(allow file-write* (subpath \"{scratch}\"))"
            ),
            executable = executable,
            runtime_root = runtime_root,
            script = script_literal,
            revision = revision,
            scratch = scratch_literal,
        );
        let mut command = Command::new(sandbox);
        command
            .args(["-p", &profile])
            .arg(&executable_path)
            .arg(script)
            .current_dir(scratch)
            .env_clear()
            .env("PYTHONNOUSERSITE", "1")
            .env("PYTHONDONTWRITEBYTECODE", "1")
            .env("NODE_OPTIONS", "--no-addons");
        return Ok(command);
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = (executable, runtime_root, script, revision_root, scratch);
        Err(anyhow!("strategy_sandbox_unavailable"))
    }
}
