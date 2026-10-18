use super::*;

#[test]
fn permission_request_is_cancelled_and_stops_autonomous_dispatch() {
    let mut protocol = OpenClawProtocol::new(config(json!({}), "hello", "native-session"));
    initialize(&mut protocol);
    protocol.handle_message(json!({
        "jsonrpc": "2.0",
        "id": SESSION_REQUEST_ID,
        "result": {"sessionId": "native-session"}
    }));
    let effects = protocol.handle_message(json!({
        "jsonrpc": "2.0",
        "id": "approval-1",
        "method": "session/request_permission",
        "params": {"sessionId": "native-session", "options": []}
    }));
    assert_eq!(effects.len(), 2);
    let ProtocolEffect::Send(response) = &effects[0] else {
        panic!("permission request must receive explicit cancellation")
    };
    assert_eq!(response["result"]["outcome"]["outcome"], "cancelled");
    let ProtocolEffect::Fail(failure) = &effects[1] else {
        panic!("autonomous dispatch must stop")
    };
    assert!(failure.user_interaction_required);
    assert_eq!(protocol.phase, ProtocolPhase::Finished);
}
