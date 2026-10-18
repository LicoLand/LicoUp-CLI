use super::*;

#[test]
fn permission_request_is_cancelled_and_reported_as_user_interaction() {
    let mut protocol = new_protocol(json!({}), "prompt", "");
    protocol.handle_message(initialize_response(true, true));
    protocol.handle_message(json!({
        "jsonrpc": "2.0", "id": SESSION_REQUEST_ID,
        "result": {"sessionId": "native", "configOptions": []}
    }));
    let effects = protocol.handle_message(json!({
        "jsonrpc": "2.0", "id": 91, "method": "session/request_permission",
        "params": {"sessionId": "native", "options": []}
    }));
    assert_eq!(effects.len(), 2);
    let ProtocolEffect::Send(response) = &effects[0] else {
        panic!("expected permission response")
    };
    assert_eq!(response["result"]["outcome"]["outcome"], "cancelled");
    let ProtocolEffect::Send(cancel) = &effects[1] else {
        panic!("expected ACP cancel notification")
    };
    assert_eq!(cancel["method"], acp::SESSION_CANCEL_METHOD);
    assert_eq!(cancel["params"]["sessionId"], "native");
    assert!(cancel.get("id").is_none());
    let effects = protocol.handle_message(json!({
        "jsonrpc": "2.0", "id": PROMPT_REQUEST_ID,
        "result": {"stopReason": "cancelled"}
    }));
    let ProtocolEffect::Fail(failure) = &effects[0] else {
        panic!("expected interaction failure")
    };
    assert!(failure.user_interaction_required);
}

#[test]
fn explicit_allow_all_selects_only_a_one_shot_permission_option() {
    let mut protocol = new_protocol(json!({"allowAll": true}), "prompt", "");
    protocol.handle_message(initialize_response(true, true));
    protocol.handle_message(json!({
        "jsonrpc": "2.0", "id": SESSION_REQUEST_ID,
        "result": {"sessionId": "native", "configOptions": []}
    }));
    let effects = protocol.handle_message(json!({
        "jsonrpc": "2.0", "id": 91, "method": "session/request_permission",
        "params": {
            "sessionId": "native",
            "options": [
                {"optionId": "persist", "kind": "allow_always", "name": "Always"},
                {"optionId": "once", "kind": "allow_once", "name": "Once"},
                {"optionId": "deny", "kind": "reject_once", "name": "Deny"}
            ]
        }
    }));
    assert_eq!(effects.len(), 1);
    let ProtocolEffect::Send(response) = &effects[0] else {
        panic!("expected one-shot permission response")
    };
    assert_eq!(response["result"]["outcome"]["outcome"], "selected");
    assert_eq!(response["result"]["outcome"]["optionId"], "once");
    assert!(protocol.interaction_failure.is_none());
}

#[test]
fn explicit_allow_all_still_fails_closed_without_a_one_shot_allow_option() {
    let mut protocol = new_protocol(json!({"allowAll": true}), "prompt", "");
    protocol.handle_message(initialize_response(true, true));
    protocol.handle_message(json!({
        "jsonrpc": "2.0", "id": SESSION_REQUEST_ID,
        "result": {"sessionId": "native", "configOptions": []}
    }));
    let effects = protocol.handle_message(json!({
        "jsonrpc": "2.0", "id": 91, "method": "session/request_permission",
        "params": {
            "sessionId": "native",
            "options": [
                {"optionId": "persist", "kind": "allow_always", "name": "Always"},
                {"optionId": "deny", "kind": "reject_once", "name": "Deny"}
            ]
        }
    }));
    assert_eq!(effects.len(), 2);
    let ProtocolEffect::Send(response) = &effects[0] else {
        panic!("expected fail-closed permission response")
    };
    assert_eq!(response["result"]["outcome"]["outcome"], "cancelled");
    assert!(protocol.interaction_failure.is_some());
}
