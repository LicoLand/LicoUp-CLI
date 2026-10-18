use super::*;

#[test]
fn resume_uses_load_when_advertised_and_requires_returned_native_id() {
    let mut protocol = new_protocol(json!({}), "next", "existing-native");
    let effects = protocol.handle_message(initialize_response(true, true));
    let ProtocolEffect::Send(request) = &effects[0] else {
        panic!("expected load request")
    };
    assert_eq!(request["method"], "session/load");
    assert_eq!(request["params"]["sessionId"], "existing-native");
    let effects = protocol.handle_message(json!({
        "jsonrpc": "2.0", "id": SESSION_REQUEST_ID,
        "result": {"sessionId": "existing-native"}
    }));
    assert!(matches!(effects[0], ProtocolEffect::Send(_)));
    assert_eq!(protocol.session_id.as_deref(), Some("existing-native"));
}

#[test]
fn restored_session_without_returned_identity_fails_before_prompt() {
    let mut protocol = new_protocol(json!({}), "next", "existing-native");
    let effects = protocol.handle_message(initialize_response(true, true));
    let ProtocolEffect::Send(load) = &effects[0] else {
        panic!("expected load request")
    };
    assert_eq!(load["method"], "session/load");
    assert_eq!(load["params"]["sessionId"], "existing-native");

    let effects = protocol.handle_message(json!({
        "jsonrpc": "2.0",
        "id": SESSION_REQUEST_ID,
        "result": {
            "configOptions": [{
                "id": "pace",
                "name": "Pace",
                "type": "select",
                "currentValue": "steady",
                "options": [{"value": "steady", "name": "Steady"}]
            }]
        }
    }));
    let ProtocolEffect::Fail(failure) = &effects[0] else {
        panic!("expected missing returned identity failure")
    };
    assert_eq!(failure.code, "acp_session_id_missing");
    assert_eq!(failure.session_id.as_deref(), Some("existing-native"));
    assert_eq!(protocol.phase, ProtocolPhase::Finished);
}

#[test]
fn direct_reconciliation_rejects_missing_resume_identity() {
    let config = ProtocolConfig::from_params(
        &json!({}),
        "next",
        "expected-native",
        Some(absolute_test_cwd().as_path()),
    )
    .unwrap();
    let failure = reconcile_acp_session_id(&config, AcpSessionPlan::Resume, None).unwrap_err();
    assert_eq!(failure.code, "acp_session_id_missing");
    assert_eq!(failure.session_id.as_deref(), Some("expected-native"));
}

#[test]
fn restored_session_matching_id_object_preserves_state_and_completes_exactly() {
    let mut protocol = new_protocol(json!({}), "next", "existing-native");
    let effects = protocol.handle_message(initialize_response(true, true));
    let ProtocolEffect::Send(load) = &effects[0] else {
        panic!("expected load request")
    };
    assert_eq!(load["method"], "session/load");
    assert_eq!(load["params"]["sessionId"], "existing-native");

    let effects = protocol.handle_message(json!({
        "jsonrpc": "2.0",
        "id": SESSION_REQUEST_ID,
        "result": {
            "sessionId": "existing-native",
            "modes": {
                "currentModeId": "review",
                "availableModes": [{"id": "review", "name": "Review"}]
            },
            "configOptions": [{
                "id": "pace",
                "name": "Pace",
                "type": "select",
                "currentValue": "steady",
                "options": [{"value": "steady", "name": "Steady"}]
            }]
        }
    }));
    let ProtocolEffect::Send(prompt) = &effects[0] else {
        panic!("expected prompt on the restored session")
    };
    assert_eq!(prompt["method"], "session/prompt");
    assert_eq!(prompt["params"]["sessionId"], "existing-native");
    assert_eq!(protocol.session_id.as_deref(), Some("existing-native"));
    assert_eq!(
        protocol.modes,
        Some(json!({
            "currentModeId": "review",
            "availableModes": [{"id": "review", "name": "Review"}]
        }))
    );
    assert_eq!(protocol.config_options.len(), 1);
    assert_eq!(protocol.config_options[0]["id"], "pace");

    let effects = protocol.handle_message(json!({
        "jsonrpc": "2.0",
        "method": "session/update",
        "params": {
            "sessionId": "existing-native",
            "update": {
                "sessionUpdate": "agent_message_chunk",
                "content": {"type": "text", "text": "continued"}
            }
        }
    }));
    assert!(effects.is_empty());

    let effects = protocol.handle_message(json!({
        "jsonrpc": "2.0",
        "id": PROMPT_REQUEST_ID,
        "result": {"stopReason": "end_turn"}
    }));
    let ProtocolEffect::Complete(outcome) = &effects[0] else {
        panic!("expected restored-session completion")
    };
    assert_eq!(outcome.session_id, "existing-native");
    assert_eq!(outcome.thread_id, "existing-native");
    assert_eq!(outcome.output, "continued");
    assert_eq!(protocol.phase, ProtocolPhase::Finished);
}

#[test]
fn resume_fails_closed_when_agent_does_not_advertise_it() {
    let mut protocol = new_protocol(json!({}), "next", "existing-native");
    let effects = protocol.handle_message(initialize_response(false, false));
    let ProtocolEffect::Fail(failure) = &effects[0] else {
        panic!("expected failure")
    };
    assert_eq!(failure.code, "acp_resume_unsupported");
}

#[test]
fn rejected_native_resume_never_falls_back_to_a_new_session() {
    let mut protocol = new_protocol(json!({}), "next", "missing-native");
    let effects = protocol.handle_message(initialize_response(true, true));
    let ProtocolEffect::Send(load) = &effects[0] else {
        panic!("expected load request")
    };
    assert_eq!(load["method"], "session/load");
    let effects = protocol.handle_message(json!({
        "jsonrpc": "2.0", "id": SESSION_REQUEST_ID,
        "error": {"code": -32002, "message": "not found"}
    }));
    assert_eq!(effects.len(), 1);
    let ProtocolEffect::Fail(failure) = &effects[0] else {
        panic!("expected resume failure")
    };
    assert_eq!(failure.code, "acp_native_session_not_found");
    assert_eq!(protocol.phase, ProtocolPhase::Finished);
}

#[test]
fn returned_resume_id_must_match_the_requested_native_session() {
    let config = ProtocolConfig::from_params(
        &json!({}),
        "next",
        "expected-native",
        Some(absolute_test_cwd().as_path()),
    )
    .unwrap();
    let failure = reconcile_acp_session_id(
        &config,
        AcpSessionPlan::Load,
        Some("different-native".to_string()),
    )
    .unwrap_err();
    assert_eq!(failure.code, "acp_session_id_mismatch");
    assert_eq!(failure.session_id.as_deref(), Some("expected-native"));
}

#[test]
fn mismatched_load_response_id_fails_without_starting_a_new_session() {
    let mut protocol = new_protocol(json!({}), "next", "expected-native");
    let effects = protocol.handle_message(initialize_response(true, true));
    let ProtocolEffect::Send(load) = &effects[0] else {
        panic!("expected load request")
    };
    assert_eq!(load["method"], "session/load");

    let effects = protocol.handle_message(json!({
        "jsonrpc": "2.0", "id": SESSION_REQUEST_ID,
        "result": {"sessionId": "different-native"}
    }));

    assert_eq!(effects.len(), 1);
    let ProtocolEffect::Fail(failure) = &effects[0] else {
        panic!("expected identity mismatch failure")
    };
    assert_eq!(failure.code, "acp_session_id_mismatch");
    assert_eq!(failure.session_id.as_deref(), Some("expected-native"));
    assert_eq!(protocol.phase, ProtocolPhase::Finished);
}
