use super::*;

#[test]
fn setting_application_requires_the_requested_value() {
    let change = ConfigChange {
        id: "model".to_string(),
        value: ConfigValue::Select("provider/model".to_string()),
    };
    assert!(setting_applied(
        &[json!({
            "id": "model",
            "currentValue": "provider/model"
        })],
        &change
    ));
    assert!(!setting_applied(
        &[json!({
            "id": "model",
            "currentValue": "different"
        })],
        &change
    ));
}

#[test]
fn unsupported_requested_setting_fails_before_prompt_dispatch() {
    let config = ProtocolConfig::from_params(
        &json!({"model": "provider/model"}),
        "prompt",
        "native",
        Some(absolute_test_cwd().as_path()),
    )
    .unwrap();
    let failure = requested_config_changes(&config.settings, &[], Some("native")).unwrap_err();
    assert_eq!(failure.code, "acp_setting_unsupported");
    assert_eq!(failure.session_id.as_deref(), Some("native"));
}

#[test]
fn unset_model_prefers_the_advertised_router_default_over_a_stale_current_value() {
    // Real Copilot 1.0.46: currentValue gpt-5-mini is advertised as current
    // while the account backend rejects it; "auto" is the advertised routing
    // entry that resolves to a supported model.
    let options = json!([{
        "id": "model", "name": "Model", "type": "select",
        "currentValue": "gpt-5-mini",
        "options": [
            {"value": "auto", "name": "Auto"},
            {"value": "gpt-5-mini", "name": "GPT-5 mini"},
            {"value": "gpt-4.1", "name": "GPT-4.1"}
        ]
    }]);
    let config = ProtocolConfig::from_params(
        &json!({}),
        "prompt",
        "native",
        Some(absolute_test_cwd().as_path()),
    )
    .unwrap();
    let changes = requested_config_changes(
        &config.settings,
        options.as_array().unwrap(),
        Some("native"),
    )
    .unwrap();
    assert_eq!(changes.len(), 1);
    assert_eq!(changes[0].id, "model");
    assert!(matches!(
        &changes[0].value,
        ConfigValue::Select(value) if value == "auto"
    ));
}

#[test]
fn unset_model_keeps_the_agent_default_when_no_router_value_is_advertised() {
    for options in [
        json!([{
            "id": "model", "name": "Model", "type": "select",
            "currentValue": "provider/default",
            "options": [{"value": "provider/default", "name": "Default"}]
        }]),
        json!([{
            "id": "model", "name": "Model", "type": "select",
            "currentValue": "auto",
            "options": [{"value": "auto", "name": "Auto"}]
        }]),
        json!([]),
    ] {
        let config = ProtocolConfig::from_params(
            &json!({}),
            "prompt",
            "native",
            Some(absolute_test_cwd().as_path()),
        )
        .unwrap();
        let changes = requested_config_changes(
            &config.settings,
            options.as_array().unwrap(),
            Some("native"),
        )
        .unwrap();
        assert!(
            changes.is_empty(),
            "no router default must leave the session model untouched: {options}"
        );
    }
}

#[test]
fn explicit_model_selection_wins_over_the_router_default() {
    let options = json!([{
        "id": "model", "name": "Model", "type": "select",
        "currentValue": "gpt-5-mini",
        "options": [
            {"value": "auto", "name": "Auto"},
            {"value": "gpt-4.1", "name": "GPT-4.1"}
        ]
    }]);
    let config = ProtocolConfig::from_params(
        &json!({"model": "gpt-4.1"}),
        "prompt",
        "native",
        Some(absolute_test_cwd().as_path()),
    )
    .unwrap();
    let changes = requested_config_changes(
        &config.settings,
        options.as_array().unwrap(),
        Some("native"),
    )
    .unwrap();
    assert_eq!(changes.len(), 1);
    assert!(matches!(
        &changes[0].value,
        ConfigValue::Select(value) if value == "gpt-4.1"
    ));
}
