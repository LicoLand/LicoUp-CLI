use super::*;

#[test]
fn capability_probe_uses_only_bounded_version_and_help_commands() {
    let (directory, executable) = compile_fake_claude("lico-claude-probe");
    let capability = probe(executable.to_string_lossy().as_ref(), 5_000, 16 * 1024);
    assert!(capability.available);
    assert!(capability.version_command_ok);
    assert!(capability.help_command_ok);
    assert!(capability.stdin_prompt);
    assert!(capability.structured_stream);
    assert!(!capability.interactive_approval_events);
    let _ = fs::remove_dir_all(directory);
}
