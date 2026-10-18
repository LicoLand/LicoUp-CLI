use super::*;

#[test]
fn exact_resume_uses_session_load_and_exact_gateway_key() {
    let config = config(json!({}), "hello", "native-session");
    let request = session_request(&config).unwrap();
    assert_eq!(request["method"], "session/load");
    assert_eq!(request["params"]["sessionId"], "native-session");
    assert_eq!(request["params"]["_meta"]["sessionKey"], "native-session");
    assert_eq!(request["params"]["_meta"]["requireExisting"], true);
}

#[test]
fn opening_update_binds_gateway_key_and_rejects_response_mismatch() {
    let config = config(json!({}), "hello", "");
    let mut binding = SessionBinding::new(&config);
    let update = validated_update(json!({
        "sessionUpdate": "session_info_update",
        "_meta": {"sessionKey": "native-session"}
    }));
    binding.capture_opening_update(&update);
    let failure = binding
        .reconcile_open_response(
            &config,
            Some("different-protocol-session".to_string()),
            "session/new",
        )
        .unwrap_err();
    assert_eq!(failure.code, "openclaw_acp_session_mismatch");
    assert_eq!(binding.native_id(), Some("native-session"));
}

#[test]
fn new_session_without_gateway_key_is_not_reported_as_resumable() {
    let config = config(json!({}), "hello", "");
    let mut binding = SessionBinding::new(&config);
    let failure = binding
        .reconcile_open_response(
            &config,
            Some("process-local-session".to_string()),
            "session/new",
        )
        .unwrap_err();
    assert_eq!(failure.code, "openclaw_acp_native_session_id_missing");
}

#[test]
fn resume_without_returned_protocol_identity_fails_before_prompt() {
    let config = config(json!({}), "hello", "native-session");
    let mut binding = SessionBinding::new(&config);
    let failure = binding
        .reconcile_open_response(&config, None, "session/load")
        .unwrap_err();
    assert_eq!(failure.code, "openclaw_acp_session_id_missing");
}

#[test]
fn resume_with_different_returned_protocol_identity_fails_before_prompt() {
    let config = config(json!({}), "hello", "native-session");
    let mut binding = SessionBinding::new(&config);
    let failure = binding
        .reconcile_open_response(
            &config,
            Some("different-session".to_string()),
            "session/load",
        )
        .unwrap_err();
    assert_eq!(failure.code, "openclaw_acp_session_mismatch");
}
