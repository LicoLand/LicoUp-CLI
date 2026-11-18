use super::*;

#[test]
fn capability_probe_uses_bounded_stdout_and_discards_stderr() {
    let (directory, executable) = compile_fake_openclaw("lico-openclaw-probe");
    let result = probe(executable.to_string_lossy().as_ref(), 5_000, 16 * 1024);
    assert!(result.available);
    assert!(result.supported);
    assert_eq!(result.version.as_deref(), Some("OpenClaw test-version"));
    assert_eq!(
        first_nonempty_line(b"\n version \n").as_deref(),
        Some("version")
    );
    let _ = fs::remove_dir_all(directory);
}
