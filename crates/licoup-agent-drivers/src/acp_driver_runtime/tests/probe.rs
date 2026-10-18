use super::*;

#[test]
fn acp_probe_rejects_relative_working_directory_before_launch() {
    let driver = AcpDriverSpec::new("test-acp", &["acp"]);
    let failure = probe_acp(driver, "unused", Path::new("relative"), 10, Some(16), 16).unwrap_err();
    assert_eq!(failure.code, "acp_working_directory_invalid");
}

#[test]
fn acp_probe_completes_capability_negotiation_through_the_bounded_channel() {
    let (dir, executable) = super::stdio_transport::compile_fake_agent("probe");
    let driver = AcpDriverSpec::new("test-acp", &["acp"]).with_identity("test-acp", "acp");
    let probe = probe_acp(
        driver,
        executable.to_string_lossy().as_ref(),
        dir.as_path(),
        10_000,
        Some(1024 * 1024),
        1024,
    )
    .expect("fake agent must complete capability negotiation");
    assert_eq!(probe.protocol_version, Some(1));
    let _ = fs::remove_dir_all(dir);
}
