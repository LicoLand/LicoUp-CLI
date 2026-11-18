use super::*;

#[test]
fn missing_executable_returns_static_start_failure() {
    let config = config(json!({}), "hello", "");
    let identity = LaunchIdentity::new(
        "definitely-missing-claude-code-fixture",
        &config,
        Some(&absolute_test_cwd()),
    );
    let (_sender, receiver) = mpsc::sync_channel(1);
    let failure = PersistentTransport::spawn(&identity, receiver, 1024).unwrap_err();
    assert_eq!(failure.code, "claude_code_start_failed");
    assert!(!failure.message.contains("definitely-missing"));
}
