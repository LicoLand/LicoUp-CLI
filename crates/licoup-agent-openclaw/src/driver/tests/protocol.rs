use super::*;

#[test]
fn new_session_requires_gateway_key_before_prompting() {
    let mut protocol = OpenClawProtocol::new(config(json!({}), "hello", ""));
    initialize(&mut protocol);
    let effects = protocol.handle_message(json!({
        "jsonrpc": "2.0",
        "id": SESSION_REQUEST_ID,
        "result": {"sessionId": "process-local-session"}
    }));
    let ProtocolEffect::Fail(failure) = &effects[0] else {
        panic!("process-local ACP identity must not be resumable")
    };
    assert_eq!(failure.code, "openclaw_acp_native_session_id_missing");
    assert_eq!(protocol.phase, ProtocolPhase::Finished);
}

#[test]
fn cross_session_update_fails_before_output_is_accepted() {
    let mut protocol = OpenClawProtocol::new(config(json!({}), "hello", "native-session"));
    initialize(&mut protocol);
    protocol.handle_message(json!({
        "jsonrpc": "2.0",
        "id": SESSION_REQUEST_ID,
        "result": {"sessionId": "protocol-session"}
    }));
    let effects = protocol.handle_message(json!({
        "jsonrpc": "2.0",
        "method": "session/update",
        "params": {
            "sessionId": "other-session",
            "update": {
                "sessionUpdate": "agent_message_chunk",
                "content": {"type": "text", "text": "wrong"}
            }
        }
    }));
    let ProtocolEffect::Fail(failure) = &effects[0] else {
        panic!("cross-session output must fail closed")
    };
    assert_eq!(failure.code, "acp_session_mismatch");
    assert!(protocol.output.is_empty());
}

#[test]
fn initialized_protocol_sends_only_session_request_before_private_prompt() {
    let mut protocol = OpenClawProtocol::new(config(
        json!({"openclawAgentId": "ops"}),
        "private prompt",
        "",
    ));
    let initial = protocol.initial_request().unwrap();
    assert!(!initial.to_string().contains("private prompt"));
    let requests = sent_messages(initialize(&mut protocol));
    assert_eq!(requests[0]["method"], "session/new");
    assert!(!requests[0].to_string().contains("private prompt"));
}
