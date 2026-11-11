use anyhow::Result;
use std::path::Path;
use std::process::Command;

pub(in crate::domain::collaboration_plugin::assembly) const CAPABILITY: &str =
    crate::platform::process_sandbox::CAPABILITY_COLLABORATION_LOOPBACK;

pub(super) fn command(
    runner: &Path,
    manifest: &Path,
    snapshot: &Path,
    runtime_data: &Path,
    port: u16,
) -> Result<Command> {
    crate::platform::process_sandbox::collaboration_loopback_command(
        runner,
        manifest,
        snapshot,
        runtime_data,
        port,
    )
}

#[cfg(all(test, target_os = "macos"))]
mod tests {
    use super::command;
    use std::fs;
    use uuid::Uuid;

    #[test]
    fn macos_profile_executes_only_the_declared_runner() {
        let root = std::env::temp_dir().join(format!("licoup-sandbox-test-{}", Uuid::new_v4()));
        fs::create_dir(&root).unwrap();
        let manifest = root.join("manifest.json");
        let snapshot = root.join("snapshot.bin");
        let runtime_data = root.join("runtime-data");
        fs::write(&manifest, b"{}").unwrap();
        fs::write(&snapshot, b"snapshot").unwrap();
        fs::create_dir(&runtime_data).unwrap();

        let status = command(
            std::path::Path::new("/usr/bin/true"),
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
