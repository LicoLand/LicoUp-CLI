use super::*;

#[test]
fn new_session_and_prompt_keep_private_values_in_acp_stdin() {
    let prompt = "private-hermes-prompt";
    let mut protocol = SessionProtocol::new(
        config(json!({"model": "provider/model"}), prompt, ""),
        HERMES_SESSION_DRIVER.driver_id,
    );
    let cwd = absolute_test_cwd();
    let launch = LaunchSpec::new(HERMES_SESSION_DRIVER, "hermes", cwd.as_path());
    assert_eq!(launch.driver.launch_args, &["acp"]);
    assert!(
        !launch
            .driver
            .launch_args
            .iter()
            .any(|arg| arg.contains(prompt))
    );
    assert!(
        !launch
            .driver
            .launch_args
            .iter()
            .any(|arg| arg.contains("workspace"))
    );

    let session = sent_messages(initialize(&mut protocol));
    assert_eq!(session[0]["method"], "session/new");
    assert_eq!(session[0]["params"]["cwd"], cwd.to_string_lossy().as_ref());
    assert!(!session[0].to_string().contains(prompt));

    let model = sent_messages(protocol.handle_message(json!({
        "jsonrpc": "2.0",
        "id": SESSION_REQUEST_ID,
        "result": {"sessionId": "native-hermes-session", "models": {"currentModelId": "default"}}
    })));
    assert_eq!(model[0]["method"], "session/set_model");
    assert_eq!(model[0]["params"]["modelId"], "provider/model");
    let prompt_request = sent_messages(protocol.handle_message(json!({
        "id": MODEL_REQUEST_ID,
        "result": {}
    })));
    assert_eq!(prompt_request[0]["method"], "session/prompt");
    assert_eq!(prompt_request[0]["params"]["prompt"][0]["text"], prompt);
}

#[test]
fn unsupported_reasoning_override_fails_closed() {
    let cwd = absolute_test_cwd();
    let failure = ProtocolConfig::from_params(
        &json!({"reasoningEffort": "high"}),
        "hello",
        "",
        Some(cwd.as_path()),
    )
    .unwrap_err();
    assert_eq!(failure.code, "hermes_acp_reasoning_override_unsupported");
}

#[test]
fn private_instructions_fail_as_a_typed_capability_without_prompt_rewrite() {
    let cwd = absolute_test_cwd();
    let failure = ProtocolConfig::from_params(
        &json!({"privateInstructions": "private-system-canary"}),
        "exact-user-prompt",
        "",
        Some(cwd.as_path()),
    )
    .unwrap_err();
    assert_eq!(failure.code, "hermes_acp_private_instructions_unsupported");
    assert!(!failure.message.contains("canary"));
    assert!(!failure.message.contains("exact-user-prompt"));
}
