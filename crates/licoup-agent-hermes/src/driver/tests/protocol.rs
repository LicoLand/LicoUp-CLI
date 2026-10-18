use super::*;

#[test]
fn exact_resume_uses_session_load_inside_json_rpc() {
    let mut protocol = SessionProtocol::new(
        config(json!({}), "hello", "native-session"),
        HERMES_SESSION_DRIVER.driver_id,
    );
    let session = sent_messages(initialize(&mut protocol));
    assert_eq!(session[0]["method"], "session/load");
    assert_eq!(session[0]["params"]["sessionId"], "native-session");
}

#[test]
fn interrupted_turn_keeps_native_session_for_exact_continue() {
    let mut protocol = SessionProtocol::new(
        config(json!({}), "hello", "native-session"),
        HERMES_SESSION_DRIVER.driver_id,
    );
    initialize(&mut protocol);
    protocol.handle_message(json!({"jsonrpc": "2.0", "id": SESSION_REQUEST_ID, "result": null}));
    let effects = protocol.handle_message(json!({
        "jsonrpc": "2.0",
        "id": PROMPT_REQUEST_ID,
        "result": {"stopReason": "cancelled"}
    }));
    match &effects[0] {
        ProtocolEffect::Fail(failure) => {
            assert_eq!(failure.code, "hermes_acp_turn_not_completed");
            assert_eq!(failure.session_id.as_deref(), Some("native-session"));
            assert_eq!(failure.turn_status.as_deref(), Some("cancelled"));
        }
        _ => panic!("interrupted turn must fail closed while retaining native session id"),
    }
}

#[test]
fn exact_resume_returns_requested_identity_after_prompt() {
    let mut protocol = SessionProtocol::new(
        config(json!({}), "hello", "native-session"),
        HERMES_SESSION_DRIVER.driver_id,
    );
    let session = sent_messages(initialize(&mut protocol));
    assert_eq!(session[0]["method"], "session/load");
    assert_eq!(session[0]["params"]["sessionId"], "native-session");
    let effects = protocol.handle_message(json!({
        "jsonrpc": "2.0",
        "id": SESSION_REQUEST_ID,
        "result": {"sessionId": "native-session"}
    }));
    // Prompt submission must follow only after the returned identity matched.
    let sent = sent_messages(effects);
    assert_eq!(sent[0]["method"], "session/prompt");
    assert_eq!(
        protocol.session_id.as_deref(),
        Some("native-session"),
        "the resumed durable identity must equal the requested identity"
    );
}

#[test]
fn resume_returning_a_different_identity_fails_before_prompt() {
    let mut protocol = SessionProtocol::new(
        config(json!({}), "hello", "native-session"),
        HERMES_SESSION_DRIVER.driver_id,
    );
    initialize(&mut protocol);
    let effects = protocol.handle_message(json!({
        "jsonrpc": "2.0",
        "id": SESSION_REQUEST_ID,
        "result": {"sessionId": "other-session"}
    }));
    match &effects[0] {
        ProtocolEffect::Fail(failure) => {
            assert_eq!(failure.code, "hermes_acp_session_mismatch");
            assert_eq!(failure.session_id.as_deref(), Some("native-session"));
        }
        _ => panic!("a load response for a different session must fail closed"),
    }
    assert_eq!(
        protocol.phase,
        ProtocolPhase::Finished,
        "no prompt may follow"
    );
}

#[test]
fn create_without_native_identity_fails_closed() {
    let mut protocol = SessionProtocol::new(
        config(json!({}), "hello", ""),
        HERMES_SESSION_DRIVER.driver_id,
    );
    initialize(&mut protocol);
    let effects = protocol.handle_message(json!({
        "jsonrpc": "2.0",
        "id": SESSION_REQUEST_ID,
        "result": {"models": {"currentModelId": "default"}}
    }));
    match &effects[0] {
        ProtocolEffect::Fail(failure) => {
            assert!(
                failure.code == "acp_session_response_invalid"
                    || failure.code == "hermes_acp_session_id_missing",
                "create without a native id must fail with a named error, got {}",
                failure.code
            );
        }
        _ => panic!("create without a native id must not start a prompt"),
    }
    assert_eq!(protocol.phase, ProtocolPhase::Finished);
}
