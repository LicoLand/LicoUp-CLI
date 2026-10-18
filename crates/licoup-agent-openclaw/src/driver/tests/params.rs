use super::*;

#[test]
fn request_validation_normalizes_agent_and_keeps_private_values_off_command_config() {
    let config = config(
        json!({"reasoningEffort": "high", "openclawAgentId": " Ops Team "}),
        "private prompt",
        "",
    );
    assert_eq!(normalize_agent_id(" Ops Team "), "ops-team");
    assert_eq!(config.reasoning_effort.as_deref(), Some("high"));
    assert!(
        config
            .native_session_key
            .as_deref()
            .unwrap()
            .starts_with("agent:ops-team:acp:")
    );
    assert_eq!(config.prompt, "private prompt");
}

#[test]
fn conflicting_native_session_identity_fails_closed() {
    let failure = ProtocolConfig::from_params(
        &json!({"sessionKey": "different"}),
        "prompt",
        "requested",
        Some(absolute_test_cwd().as_path()),
        || Ok(Vec::new()),
    )
    .unwrap_err();
    assert_eq!(failure.code, "openclaw_acp_conflicting_session_id");
}

#[test]
fn private_instructions_fail_as_a_typed_capability_without_prompt_rewrite() {
    let failure = ProtocolConfig::from_params(
        &json!({"privateInstructions": "private-system-canary"}),
        "exact-user-prompt",
        "",
        Some(absolute_test_cwd().as_path()),
        || Ok(Vec::new()),
    )
    .unwrap_err();
    assert_eq!(
        failure.code,
        "openclaw_acp_private_instructions_unsupported"
    );
    assert!(!failure.message.contains("canary"));
    assert!(!failure.message.contains("exact-user-prompt"));
}
