use super::*;

#[test]
fn session_update_for_another_session_fails_closed() {
    let mut protocol = new_protocol(json!({}), "private", "");
    protocol.handle_message(initialize_response(true, true));
    protocol.handle_message(json!({
        "jsonrpc": "2.0",
        "id": SESSION_REQUEST_ID,
        "result": {"sessionId": "native-session", "configOptions": []}
    }));
    let effects = protocol.handle_message(json!({
        "jsonrpc": "2.0",
        "method": "session/update",
        "params": {"sessionId": "other", "update": {
            "sessionUpdate": "agent_message_chunk",
            "content": {"type": "text", "text": "wrong"}
        }}
    }));
    let ProtocolEffect::Fail(failure) = &effects[0] else {
        panic!("cross-session output must fail the protocol")
    };
    assert_eq!(failure.code, "acp_session_mismatch");
    assert_eq!(protocol.phase, ProtocolPhase::Finished);
    assert!(protocol.output.is_empty());
}
