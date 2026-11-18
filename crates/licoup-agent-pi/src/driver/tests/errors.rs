use super::*;

#[test]
fn failures_keep_static_redacted_messages_and_only_bound_ids() {
    let failure = ProtocolFailure::new(
        "pi_rpc_failed",
        "Pi RPC did not complete the request.",
        "protocol",
    )
    .with_session(Some(" native-session "))
    .with_turn("turn-id");
    assert_eq!(failure.session_id.as_deref(), Some("native-session"));
    assert_eq!(failure.turn_id.as_deref(), Some("turn-id"));
    assert!(!failure.message.contains("private"));
}
