use super::*;

#[test]
fn native_authentication_error_is_structured_user_interaction() {
    let failure = failure_from_response(
        &json!({"error": {"code": -32000, "message": "private runtime detail"}}),
        "acp_session_rejected",
        "fallback",
        "session/setup",
        Some("native"),
    );
    assert_eq!(failure.code, "acp_authentication_required");
    assert!(failure.user_interaction_required);
    assert_eq!(failure.request_method.as_deref(), Some("authenticate"));
    assert!(!failure.message.contains("private"));
}
