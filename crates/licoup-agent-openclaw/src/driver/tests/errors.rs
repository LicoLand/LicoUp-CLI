use super::*;

#[test]
fn interaction_failure_is_static_and_preserves_minimum_context() {
    let failure = ProtocolFailure::user_interaction(
        "session/request_permission",
        Some("native-session"),
        Some("turn-1"),
    );
    assert_eq!(failure.code, "openclaw_user_interaction_required");
    assert!(failure.user_interaction_required);
    assert_eq!(
        failure.request_method.as_deref(),
        Some("session/request_permission")
    );
    assert!(!failure.message.contains("native-session"));
}
