use super::super::executable;
use serde_json::json;

#[test]
fn explicit_executable_wins_and_missing_absolute_paths_fail_closed() {
    let resolved = executable::resolve(
        &json!({"executable": "/definitely/missing/local-agent"}),
        &[],
        "fallback",
    );
    assert_eq!(resolved, "/definitely/missing/local-agent");
    assert!(!executable::available(&resolved));
}

#[cfg(unix)]
#[test]
fn which_path_searches_the_given_path_dirs_in_order() {
    use std::os::unix::fs::PermissionsExt;
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let root =
        std::env::temp_dir().join(format!("licoup-which-path-{}-{stamp}", std::process::id()));
    let shell_dir = root.join("shell-bin");
    let process_dir = root.join("process-bin");
    std::fs::create_dir_all(&shell_dir).unwrap();
    std::fs::create_dir_all(&process_dir).unwrap();
    let shell_binary = shell_dir.join("fixture-agent-cli");
    std::fs::write(&shell_binary, "#!/bin/sh\n").unwrap();
    std::fs::set_permissions(&shell_binary, std::fs::Permissions::from_mode(0o755)).unwrap();
    let process_binary = process_dir.join("fixture-agent-cli");
    std::fs::write(&process_binary, "#!/bin/sh\n").unwrap();

    // A CLI on the user shell PATH is found, and it outranks the same name on
    // the process PATH fallback.
    let found = executable::which_path_in_dirs(
        "fixture-agent-cli",
        &[shell_dir.clone(), process_dir.clone()],
    );
    assert_eq!(found.as_deref(), Some(shell_binary.as_path()));
    let fallback =
        executable::which_path_in_dirs("fixture-agent-cli", std::slice::from_ref(&process_dir));
    assert_eq!(fallback.as_deref(), Some(process_binary.as_path()));
    assert!(executable::which_path_in_dirs("fixture-agent-cli", &[]).is_none());
    assert!(
        executable::which_path_in_dirs("fixture-missing-cli", std::slice::from_ref(&shell_dir))
            .is_none()
    );

    let _ = std::fs::remove_dir_all(&root);
}
