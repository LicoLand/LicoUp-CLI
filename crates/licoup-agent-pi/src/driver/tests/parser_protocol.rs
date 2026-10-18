use super::*;

#[test]
fn new_session_prompt_stays_on_stdio_channel() {
    let config = ProtocolConfig::from_params(
        &json!({}),
        "private-pi-prompt",
        "",
        Some(absolute_test_cwd().as_path()),
    )
    .unwrap();
    let mut protocol = PiProtocol::new(config);
    let request = protocol.initial_request();
    assert_eq!(request["type"], "get_state");
    let effects = protocol.handle_message(json!({
        "id": "lico-pi-initial-state",
        "type": "response",
        "command": "get_state",
        "success": true,
        "data": {"sessionId": "new-session"}
    }));
    let ProtocolEffect::Send(prompt) = &effects[0] else {
        panic!("initial state must advance to prompt");
    };
    assert_eq!(prompt["type"], "prompt");
    assert_eq!(prompt["message"], "private-pi-prompt");
    let launch = LaunchSpec::new("pi", absolute_test_cwd().as_path());
    assert!(
        !launch
            .args
            .iter()
            .any(|arg| arg.contains("private-pi-prompt"))
    );
}

#[test]
fn provider_error_without_assistant_text_surfaces_gateway_credential_failure() {
    let config = ProtocolConfig::from_params(
        &json!({}),
        "who are you",
        "",
        Some(absolute_test_cwd().as_path()),
    )
    .unwrap();
    let mut protocol = PiProtocol::new(config);
    let _ = protocol.initial_request();
    let _ = protocol.handle_message(json!({
        "type": "response",
        "command": "get_state",
        "success": true,
        "data": {"sessionId": "pi-err-1"}
    }));
    let _ = protocol.handle_message(json!({
        "type": "response",
        "command": "prompt",
        "success": true
    }));
    let _ = protocol.handle_message(json!({
        "type": "message_end",
        "message": {
            "role": "assistant",
            "content": [],
            "stopReason": "error",
            "errorMessage": "503: {\"code\":\"gateway_credential_unavailable\",\"message\":\"gateway_credential_unavailable\"}"
        }
    }));
    let settled = protocol.handle_message(json!({"type": "agent_settled"}));
    assert!(matches!(
        &settled[0],
        ProtocolEffect::Send(request) if request["type"] == "get_last_assistant_text"
    ));
    let _ = protocol.handle_message(json!({
        "type": "response",
        "command": "get_last_assistant_text",
        "success": true,
        "data": {"text": null}
    }));
    let finished = protocol.handle_message(json!({
        "type": "response",
        "command": "get_state",
        "success": true,
        "data": {"sessionId": "pi-err-1"}
    }));
    assert!(matches!(
        &finished[0],
        ProtocolEffect::Fail(failure)
            if failure.code == "pi_gateway_credentials_unavailable"
    ));
}

#[test]
fn text_end_content_recovers_when_deltas_were_missed() {
    let config =
        ProtocolConfig::from_params(&json!({}), "hello", "", Some(absolute_test_cwd().as_path()))
            .unwrap();
    let mut protocol = PiProtocol::new(config);
    let _ = protocol.initial_request();
    let _ = protocol.handle_message(json!({
        "type": "response",
        "command": "get_state",
        "success": true,
        "data": {"sessionId": "pi-text-end"}
    }));
    let _ = protocol.handle_message(json!({
        "type": "response",
        "command": "prompt",
        "success": true
    }));
    let _ = protocol.handle_message(json!({
        "type": "message_update",
        "assistantMessageEvent": {
            "type": "text_end",
            "contentIndex": 0,
            "content": "recovered reply"
        }
    }));
    let _ = protocol.handle_message(json!({"type": "agent_settled"}));
    let _ = protocol.handle_message(json!({
        "type": "response",
        "command": "get_last_assistant_text",
        "success": true,
        "data": {"text": null}
    }));
    let finished = protocol.handle_message(json!({
        "type": "response",
        "command": "get_state",
        "success": true,
        "data": {"sessionId": "pi-text-end"}
    }));
    let ProtocolEffect::Complete(outcome) = &finished[0] else {
        panic!("expected complete turn");
    };
    assert_eq!(outcome.output, "recovered reply");
}

#[test]
fn switched_state_must_confirm_requested_identity() {
    let config = resume_config(
        "continue",
        "expected-session",
        absolute_test_cwd().join("session.jsonl"),
    );
    let mut protocol = PiProtocol::new(config);
    let _ = protocol.initial_request();
    let switch = protocol.handle_message(json!({
        "id": "lico-pi-switch",
        "type": "response",
        "command": "switch_session",
        "success": true,
        "data": {"cancelled": false}
    }));
    assert!(matches!(switch[0], ProtocolEffect::Send(_)));
    let state = protocol.handle_message(json!({
        "id": "lico-pi-switched-state",
        "type": "response",
        "command": "get_state",
        "success": true,
        "data": {"sessionId": "wrong-session"}
    }));
    assert!(matches!(
        &state[0],
        ProtocolEffect::Fail(failure) if failure.code == "pi_session_identity_mismatch"
    ));
}
