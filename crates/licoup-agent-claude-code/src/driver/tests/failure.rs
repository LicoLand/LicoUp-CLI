use super::*;

#[test]
fn failures_are_static_and_reset_policy_is_explicit() {
    let failure = ProtocolFailure::new(
        "claude_code_timeout",
        "Claude Code timed out before the turn completed.",
        "turn/wait",
    );
    assert!(requires_transport_reset(&failure));
    assert!(!failure.message.contains("session"));
    assert!(!requires_transport_reset(&ProtocolFailure::new(
        "claude_code_user_interaction_required",
        "Claude Code requires user interaction.",
        "permission/request",
    )));
}
