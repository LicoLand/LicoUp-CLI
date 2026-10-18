use super::*;

#[test]
fn new_session_applies_settings_then_reports_protocol_finish() {
    let mut protocol = new_protocol(json!({"model": "provider/model"}), "private", "");
    let effects = protocol.handle_message(initialize_response(true, true));
    assert!(matches!(effects[0], ProtocolEffect::Send(_)));
    let effects = protocol.handle_message(json!({
        "jsonrpc": "2.0",
        "id": SESSION_REQUEST_ID,
        "result": {
            "sessionId": "native-session",
            "configOptions": [{
                "id": "model", "name": "Model", "type": "select",
                "currentValue": "default", "options": [
                    {"value": "default", "name": "Default"},
                    {"value": "provider/model", "name": "Requested"}
                ]
            }]
        }
    }));
    let ProtocolEffect::Send(setting) = &effects[0] else {
        panic!("expected setting request")
    };
    assert_eq!(setting["method"], "session/set_config_option");
    let effects = protocol.handle_message(json!({
        "jsonrpc": "2.0", "id": FIRST_CONFIG_REQUEST_ID,
        "result": {"configOptions": [{
            "id": "model", "name": "Model", "type": "select",
            "currentValue": "provider/model", "options": []
        }]}
    }));
    let ProtocolEffect::Send(prompt) = &effects[0] else {
        panic!("expected prompt request")
    };
    assert_eq!(prompt["method"], "session/prompt");
    assert_eq!(prompt["params"]["prompt"][0]["text"], "private");
    protocol.handle_message(json!({
        "jsonrpc": "2.0", "method": "session/update",
        "params": {"sessionId": "native-session", "update": {
            "sessionUpdate": "agent_message_chunk", "content": {"type": "text", "text": "final"}
        }}
    }));
    let effects = protocol.handle_message(json!({
        "jsonrpc": "2.0", "id": PROMPT_REQUEST_ID,
        "result": {"stopReason": "end_turn"}
    }));
    let ProtocolEffect::Complete(outcome) = &effects[0] else {
        panic!("expected completion")
    };
    assert_eq!(outcome.output, "final");
    assert_eq!(outcome.session_id, "native-session");
    assert_eq!(outcome.effective.model.as_deref(), Some("provider/model"));
    assert!(outcome.transitions.iter().any(|transition| matches!(
        transition,
        licoup_agent_adapter_sdk::Transition::Text { text, .. }
            if text == "final"
    )));
}

#[test]
fn protocol_finish_fails_closed_when_no_agent_output_was_reported() {
    let mut protocol = new_protocol(json!({}), "private", "");
    protocol.handle_message(initialize_response(true, true));
    protocol.handle_message(json!({
        "jsonrpc": "2.0", "id": SESSION_REQUEST_ID,
        "result": {"sessionId": "native-session", "configOptions": []}
    }));
    let effects = protocol.handle_message(json!({
        "jsonrpc": "2.0", "id": PROMPT_REQUEST_ID,
        "result": {"stopReason": "end_turn"}
    }));
    let ProtocolEffect::Fail(failure) = &effects[0] else {
        panic!("empty output at protocol finish must fail")
    };
    assert_eq!(failure.code, "acp_final_message_missing");
    assert_eq!(failure.turn_status.as_deref(), Some("end_turn"));
    assert_eq!(protocol.phase, ProtocolPhase::Finished);
}

#[test]
fn pre_binding_session_updates_are_tolerated_until_the_session_response_arrives() {
    let mut protocol = new_protocol(json!({}), "private", "");
    protocol.handle_message(initialize_response(true, true));
    // Real Copilot emits available_commands_update for the conversation that
    // is still being created, before the session/new response arrives.
    let effects = protocol.handle_message(json!({
        "jsonrpc": "2.0", "method": "session/update",
        "params": {"sessionId": "pending-native-session", "update": {
            "sessionUpdate": "available_commands_update",
            "availableCommands": [{"name": "compact", "description": "Summarize"}]
        }}
    }));
    assert!(effects.is_empty());
    assert_eq!(protocol.phase, ProtocolPhase::AwaitSession);

    let effects = protocol.handle_message(json!({
        "jsonrpc": "2.0", "id": SESSION_REQUEST_ID,
        "result": {"sessionId": "pending-native-session", "configOptions": []}
    }));
    let ProtocolEffect::Send(prompt) = &effects[0] else {
        panic!("expected the prompt request after a tolerated pre-bind update")
    };
    assert_eq!(prompt["method"], "session/prompt");
    assert_eq!(
        protocol.session_id.as_deref(),
        Some("pending-native-session")
    );
}

#[test]
fn malformed_pre_binding_session_update_still_fails_closed() {
    let mut protocol = new_protocol(json!({}), "private", "");
    protocol.handle_message(initialize_response(true, true));
    let effects = protocol.handle_message(json!({
        "jsonrpc": "2.0", "method": "session/update",
        "params": {"sessionId": "pending-native-session", "update": {
            "sessionUpdate": "not-a-real-update-kind"
        }}
    }));
    let ProtocolEffect::Fail(failure) = &effects[0] else {
        panic!("a malformed pre-bind update must fail the protocol")
    };
    assert_eq!(failure.code, "acp_session_update_invalid");
    assert_eq!(protocol.phase, ProtocolPhase::Finished);
}

#[test]
fn interleaved_updates_keep_both_public_views_complete_and_ordered() {
    let mut protocol = new_protocol(json!({}), "private", "");
    protocol.handle_message(initialize_response(true, true));
    protocol.handle_message(json!({
        "jsonrpc": "2.0", "id": SESSION_REQUEST_ID,
        "result": {"sessionId": "native-session", "configOptions": []}
    }));

    for text in ["first ", "second ", "third"] {
        let effects = protocol.handle_message(json!({
            "jsonrpc": "2.0", "method": "session/update",
            "params": {"sessionId": "native-session", "update": {
                "sessionUpdate": "agent_message_chunk",
                "content": {"type": "text", "text": text}
            }}
        }));
        assert!(effects.is_empty());
        let effects = protocol.handle_message(json!({
            "jsonrpc": "2.0", "method": "session/update",
            "params": {"sessionId": "native-session", "update": {
                "sessionUpdate": "available_commands_update",
                "availableCommands": [{"name": "compact", "description": "Summarize"}]
            }}
        }));
        assert!(effects.is_empty());
    }

    let effects = protocol.handle_message(json!({
        "jsonrpc": "2.0", "id": PROMPT_REQUEST_ID,
        "result": {"stopReason": "end_turn"}
    }));
    let ProtocolEffect::Complete(outcome) = &effects[0] else {
        panic!("expected completion after the interleaved updates")
    };
    assert_eq!(outcome.output, "first second third");
    assert!(outcome.transitions.iter().any(|transition| matches!(
        transition,
        licoup_agent_adapter_sdk::Transition::Text { text, .. }
            if text == "first second third"
    )));
}
