use super::*;

#[test]
fn fire_and_forget_extension_ui_events_never_park_or_fail() {
    let config = ProtocolConfig::from_params(
        &json!({}),
        "hello",
        "",
        Some(Path::new("/workspace/project")),
    )
    .unwrap();
    let mut protocol = PiProtocol::new(config);
    for method in [
        "notify",
        "setStatus",
        "setWidget",
        "setTitle",
        "set_editor_text",
    ] {
        let effects = protocol.handle_message(json!({
            "type": "extension_ui_request",
            "id": format!("ui-{method}"),
            "method": method,
        }));
        assert!(effects.is_empty());
    }
    assert_eq!(protocol.events.len(), 5);
}

#[test]
fn value_dialogs_emit_one_matching_native_response_shape() {
    for (method, request_fields, structured, expected_field) in [
        (
            "select",
            json!({"options": ["one", "two"]}),
            json!({"selected": "two"}),
            json!({"value": "two"}),
        ),
        (
            "input",
            json!({"placeholder": "Synthetic input"}),
            json!({"text": "answer"}),
            json!({"value": "answer"}),
        ),
        (
            "editor",
            json!({"prefill": "Synthetic text"}),
            json!({"text": "edited"}),
            json!({"value": "edited"}),
        ),
    ] {
        let config = ProtocolConfig::from_params(
            &json!({}),
            "hello",
            "",
            Some(Path::new("/workspace/project")),
        )
        .unwrap();
        let mut protocol = PiProtocol::new(config);
        protocol.session_id = Some("synthetic-session".to_string());
        let mut request = json!({
            "type": "extension_ui_request",
            "id": format!("ui-{method}"),
            "method": method,
            "title": "Synthetic dialog",
        });
        request
            .as_object_mut()
            .unwrap()
            .extend(request_fields.as_object().unwrap().clone());
        let ProtocolEffect::Interact(interaction) =
            protocol.handle_message(request).into_iter().next().unwrap()
        else {
            panic!("{method} must park");
        };
        licoup_foundation::platform::native_agent_interaction::resolve_scoped(
            interaction.callback_token(),
            Some("synthetic-session"),
            Some(&protocol.config.turn_id),
            structured,
        )
        .unwrap();
        let response = interaction.response(&protocol).unwrap();
        assert_eq!(response["type"], "extension_ui_response");
        assert_eq!(response["id"], format!("ui-{method}"));
        for (key, value) in expected_field.as_object().unwrap() {
            assert_eq!(&response[key], value);
        }
        assert!(response.get("result").is_none());
        assert!(response.get("method").is_none());
    }
}

#[test]
fn unknown_and_missing_extension_ui_methods_fail_exactly() {
    let config = ProtocolConfig::from_params(
        &json!({}),
        "hello",
        "",
        Some(Path::new("/workspace/project")),
    )
    .unwrap();
    let mut missing = PiProtocol::new(config.clone());
    let effects = missing.handle_message(json!({
        "type": "extension_ui_request",
        "id": "ui-missing"
    }));
    assert!(matches!(
        &effects[0],
        ProtocolEffect::Fail(failure)
            if failure.code == "pi_extension_ui_method_missing"
    ));

    let mut unknown = PiProtocol::new(config);
    let effects = unknown.handle_message(json!({
        "type": "extension_ui_request",
        "id": "ui-unknown",
        "method": "custom"
    }));
    assert!(matches!(
        &effects[0],
        ProtocolEffect::Fail(failure)
            if failure.code == "pi_extension_ui_method_unsupported"
    ));

    let mut missing_id = PiProtocol::new(
        ProtocolConfig::from_params(
            &json!({}),
            "hello",
            "",
            Some(Path::new("/workspace/project")),
        )
        .unwrap(),
    );
    missing_id.session_id = Some("synthetic-session".to_string());
    let effects = missing_id.handle_message(json!({
        "type": "extension_ui_request",
        "method": "input"
    }));
    assert!(matches!(
        &effects[0],
        ProtocolEffect::Fail(failure)
            if failure.code == "pi_extension_ui_request_invalid"
    ));
}
