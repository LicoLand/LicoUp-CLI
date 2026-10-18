use super::*;

#[cfg(unix)]
#[test]
fn probe_uses_only_fixed_acp_commands_and_never_projects_stderr() {
    let root = unique_temp_dir("hermes-acp-probe");
    let executable = root.join("fake-hermes-probe");
    write_executable(
        &executable,
        r#"#!/bin/sh
if [ "$1" = "acp" ] && [ "$2" = "--check" ] && [ "$#" -eq 2 ]; then
  printf '%s\n' 'Hermes ACP check OK'
  printf '%s\n' 'private-stderr-canary' >&2
  exit 0
fi
if [ "$1" = "acp" ] && [ "$2" = "--version" ] && [ "$#" -eq 2 ]; then
  printf '%s\n' 'Hermes public-version'
  printf '%s\n' 'private-version-stderr' >&2
  exit 0
fi
exit 40
"#,
    );
    let result = probe_driver(executable.to_str().unwrap(), 10_000, 16 * 1024);
    assert!(result.available);
    assert!(result.supported);
    assert_eq!(result.version.as_deref(), Some("Hermes public-version"));
    assert_ne!(result.version.as_deref(), Some("private-version-stderr"));
    let _ = fs::remove_dir_all(root);
}
