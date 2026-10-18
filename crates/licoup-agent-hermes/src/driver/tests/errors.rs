use super::*;

#[test]
fn protocol_failures_expose_only_static_redacted_messages() {
    let failure = ProtocolFailure::new(
        "hermes_acp_start_failed",
        "Hermes ACP could not be started.",
        "process/start",
    );
    assert_eq!(failure.message, "Hermes ACP could not be started.");
    assert!(!failure.message.contains("stderr"));
    assert!(!failure.message.contains("private"));
}
