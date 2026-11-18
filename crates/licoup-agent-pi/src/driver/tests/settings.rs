use super::*;

#[test]
fn model_override_resolves_bare_selector_and_uses_rpc() {
    let config = ProtocolConfig::from_params(
        &json!({"model": "model-without-provider"}),
        "hello",
        "",
        Some(absolute_test_cwd().as_path()),
    )
    .unwrap();
    let mut protocol = PiProtocol::new(config);
    let _ = protocol.initial_request();
    let effects = protocol.handle_message(json!({
        "id": "lico-pi-initial-state",
        "type": "response",
        "command": "get_state",
        "success": true,
        "data": {"sessionId": "session-model"}
    }));
    let ProtocolEffect::Send(request) = &effects[0] else {
        panic!("bare model must advance to native model resolution");
    };
    assert_eq!(request["type"], "get_available_models");

    let effects = protocol.handle_message(json!({
        "id": "lico-pi-available-models",
        "type": "response",
        "command": "get_available_models",
        "success": true,
        "data": {"models": [
            {"provider": "provider", "id": "model-without-provider"},
            {"provider": "other", "id": "other-model"}
        ]}
    }));
    let ProtocolEffect::Send(request) = &effects[0] else {
        panic!("resolved model must advance to model configuration");
    };
    assert_eq!(request["type"], "set_model");
    assert_eq!(request["provider"], "provider");
    assert_eq!(request["modelId"], "model-without-provider");
}

#[test]
fn ambiguous_bare_model_requires_provider() {
    let config = ProtocolConfig::from_params(
        &json!({"model": "shared-model"}),
        "hello",
        "",
        Some(absolute_test_cwd().as_path()),
    )
    .unwrap();
    let mut protocol = PiProtocol::new(config);
    let _ = protocol.initial_request();
    let _ = protocol.handle_message(json!({
        "type": "response",
        "command": "get_state",
        "success": true,
        "data": {"sessionId": "session-model"}
    }));
    let effects = protocol.handle_message(json!({
        "type": "response",
        "command": "get_available_models",
        "success": true,
        "data": {"models": [
            {"provider": "one", "id": "shared-model"},
            {"provider": "two", "id": "shared-model"}
        ]}
    }));
    assert!(matches!(
        &effects[0],
        ProtocolEffect::Fail(failure) if failure.code == "pi_model_provider_required"
    ));
}

#[test]
fn qualified_model_override_uses_rpc_directly() {
    let config = ProtocolConfig::from_params(
        &json!({"model": "provider/model"}),
        "hello",
        "",
        Some(absolute_test_cwd().as_path()),
    )
    .unwrap();
    let mut protocol = PiProtocol::new(config);
    let _ = protocol.initial_request();
    let effects = protocol.handle_message(json!({
        "id": "lico-pi-initial-state",
        "type": "response",
        "command": "get_state",
        "success": true,
        "data": {"sessionId": "session-model"}
    }));
    let ProtocolEffect::Send(request) = &effects[0] else {
        panic!("initial state must advance to model configuration");
    };
    assert_eq!(request["type"], "set_model");
    assert_eq!(request["provider"], "provider");
    assert_eq!(request["modelId"], "model");
}

#[test]
fn thinking_override_uses_the_rpc_configuration_step() {
    let config = ProtocolConfig::from_params(
        &json!({"thinking": "high"}),
        "hello",
        "",
        Some(absolute_test_cwd().as_path()),
    )
    .unwrap();
    let mut protocol = PiProtocol::new(config);
    let _ = protocol.initial_request();
    let effects = protocol.handle_message(json!({
        "id": "lico-pi-initial-state",
        "type": "response",
        "command": "get_state",
        "success": true,
        "data": {"sessionId": "session-thinking"}
    }));
    let ProtocolEffect::Send(request) = &effects[0] else {
        panic!("initial state must advance to thinking configuration");
    };
    assert_eq!(request["type"], "set_thinking_level");
    assert_eq!(request["level"], "high");
}
