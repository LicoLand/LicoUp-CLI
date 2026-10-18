use super::*;

#[test]
fn interrupt_message_carries_no_session_or_prompt() {
    let message = interrupt_request();
    assert_eq!(message["type"], "control_request");
    assert_eq!(
        message.pointer("/request/subtype"),
        Some(&json!("interrupt"))
    );
    assert!(message.get("session_id").is_none());
    assert!(message.get("prompt").is_none());
}

#[test]
fn permission_denial_echoes_only_request_identity_and_static_error() {
    let config = config(json!({}), "hello", "native-session");
    let identity = LaunchIdentity::new("claude", &config, Some(&absolute_test_cwd()));
    let mut parser = ClaudeCodeParser::new(&config, &identity, Some("native-session".to_owned()));
    let line = serde_json::to_vec(&json!({
        "request_id": "request-1",
        "type": "control_request",
        "request": {"subtype": "can_use_tool", "input": {"secret": "value"}}
    }))
    .unwrap();
    let Some(ClaudeEffect::Control {
        response: Some(response),
    }) = parser.parse_line(&line).unwrap()
    else {
        panic!("control request was not parser-classified");
    };
    assert_eq!(
        response.pointer("/response/request_id"),
        Some(&json!("request-1"))
    );
    assert!(!response.to_string().contains("secret"));
}
