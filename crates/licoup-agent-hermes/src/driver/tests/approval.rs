use super::*;

#[test]
fn permission_request_parks_for_external_approval_when_session_exists() {
    let mut protocol = SessionProtocol::new(
        config(json!({}), "hello", "native-session"),
        HERMES_SESSION_DRIVER.driver_id,
    );
    initialize(&mut protocol);
    protocol.handle_message(json!({"jsonrpc": "2.0", "id": SESSION_REQUEST_ID, "result": null}));
    let effects = protocol.handle_message(json!({
        "jsonrpc": "2.0",
        "id": "approval-1",
        "method": "session/request_permission",
        "params": {
            "sessionId": "native-session",
            "options": [{"optionId": "allow-once", "kind": "allow_once", "name": "Allow once"}]
        }
    }));
    assert_eq!(effects.len(), 1);
    match &effects[0] {
        ProtocolEffect::AwaitExternalApproval {
            display_summary,
            option_id,
            ..
        } => {
            assert!(display_summary.contains("Hermes Agent requests permission"));
            assert_eq!(option_id.as_deref(), Some("allow-once"));
        }
        _ => panic!("permission request with a session must park for external approval"),
    }
}

#[test]
fn permission_request_without_session_fails_closed() {
    let mut protocol = SessionProtocol::new(
        config(json!({}), "hello", ""),
        HERMES_SESSION_DRIVER.driver_id,
    );
    // Skip session establishment so there is no durable pause handle.
    protocol.phase = ProtocolPhase::AwaitPrompt;
    let effects = protocol.handle_message(json!({
        "jsonrpc": "2.0",
        "id": "approval-1",
        "method": "session/request_permission",
        "params": {"options": []}
    }));
    assert!(effects.len() >= 1);
    match &effects[0] {
        ProtocolEffect::Send(message) => {
            assert_eq!(message["result"]["outcome"]["outcome"], "cancelled");
        }
        _ => panic!("missing pause handle must fail closed with an explicit denial"),
    }
    assert!(
        protocol
            .interaction_failure
            .as_ref()
            .is_some_and(|f| f.user_interaction_required)
    );
}

#[cfg(unix)]
#[test]
fn permission_denial_stops_autonomous_dispatch() {
    let mut protocol = SessionProtocol::new(
        config(json!({}), "hello", "native-session"),
        HERMES_SESSION_DRIVER.driver_id,
    );
    initialize(&mut protocol);
    protocol.handle_message(json!({"jsonrpc": "2.0", "id": SESSION_REQUEST_ID, "result": null}));
    protocol.interaction_failure = Some(ProtocolFailure::user_interaction(
        "session/request_permission",
        Some("native-session"),
        Some(&protocol.config.turn_id),
    ));
    let terminal = protocol.handle_message(json!({
        "jsonrpc": "2.0",
        "id": PROMPT_REQUEST_ID,
        "result": {"stopReason": "cancelled"}
    }));
    match &terminal[0] {
        ProtocolEffect::Fail(failure) => {
            assert!(failure.user_interaction_required);
            assert_eq!(failure.code, "hermes_user_interaction_required");
            assert_eq!(failure.turn_status.as_deref(), Some("cancelled"));
        }
        _ => panic!("denied permission turn must stop autonomous dispatch"),
    }
}
