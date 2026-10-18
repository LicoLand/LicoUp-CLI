use super::super::test_support::*;

#[test]
fn read_only_session_binding_uses_packaged_agents_without_send_readiness() {
    let all = allowed_agent_ids(&json!({}), "agent.sessions.list").unwrap();
    let ids = all.as_array().unwrap();
    assert_eq!(
        ids.len(),
        crate::platform::runtime_adapters::PACKAGED_RUNTIME_ADAPTER_IDS.len()
    );
    assert!(ids.iter().any(|id| id == "codex"));

    let narrowed = allowed_agent_ids(
        &json!({"allowedAgentIds": ["codex", "unsupported-fixture-agent"]}),
        "agent.sessions.list",
    )
    .unwrap();
    assert_eq!(narrowed, json!(["codex"]));
}

#[test]
fn completed_authority_generation_cannot_be_overwritten_by_pre_reset_snapshot() {
    let dir = temp_dir("mobile-relay-authority-generation-cas");
    let previous = set_portable_data_dir_override(Some(dir.to_path_buf()));
    let mut durable = load_config().unwrap();
    let mut pre_reset = durable.clone();
    begin_kt_authority_reset().unwrap();
    durable[AUTHORITY_GENERATION_FIELD] = json!(
        config_generation(&durable, AUTHORITY_GENERATION_FIELD)
            .unwrap()
            .checked_add(1)
            .unwrap()
    );
    save_config_raw_with_reset_policy(&mut durable, true).unwrap();
    complete_kt_authority_reset().unwrap();
    pre_reset["pcClientName"] = json!("stale-before-reset");

    let error = save_config(&mut pre_reset).unwrap_err().to_string();
    assert!(error.contains("snapshot is stale") || error.contains("authority generation"));
    let reloaded = load_config_without_persistence().unwrap();
    assert_eq!(
        reloaded[AUTHORITY_GENERATION_FIELD],
        durable[AUTHORITY_GENERATION_FIELD]
    );

    set_portable_data_dir_override(previous);
}

#[test]
fn selected_public_paired_device_restores_internal_token_without_exposure() {
    let dir = temp_dir("mobile-relay-select-redacted-device");
    let previous = set_portable_data_dir_override(Some(dir.to_path_buf()));
    let mut config = default_config();
    let store = Arc::new(EphemeralSecretStore::new());
    config["pairingId"] = json!("pair-active");
    config["mobileToken"] = json!("mobile-token-active-canary");
    config["pairedDevices"] = json!([
        {
            "id": "pc-active",
            "pcClientId": "pc-active",
            "pcClientName": "Active Mac",
            "pairingId": "pair-active",
            "mobileToken": "mobile-token-active-canary"
        },
        {
            "id": "pc-selected",
            "pcClientId": "pc-selected",
            "pcClientName": "Selected Mac",
            "pairingId": "pair-selected",
            "mobileToken": "mobile-token-selected-canary"
        }
    ]);
    let store_override: Arc<dyn SecureMeshSecretStore> = store.clone();
    let saved = with_mobile_relay_secret_store_override(store_override, || {
        save_config(&mut config)?;
        config_set(&json!({
            "pairingId": "pair-selected",
            "mobileToken": "",
            "paired": true
        }))
    })
    .unwrap();
    let internal = load_config().unwrap();
    assert_eq!(internal["pairingId"], "pair-selected");
    assert_eq!(internal["mobileToken"], "");
    assert_eq!(internal["pcClientId"], "pc-selected");
    assert_eq!(internal["pcClientName"], "Selected Mac");
    let serialized = serde_json::to_string(&saved).unwrap();
    assert!(!serialized.contains("mobile-token-selected-canary"));
    assert_eq!(saved["config"]["mobileTokenPresent"], true);
    let paired_handle_key =
        paired_device_token_secret_store_key(&internal["pairedDevices"][1]).unwrap();
    let paired_handle = native_secret_store_handle_for_namespace(
        MOBILE_RELAY_PLATFORM_SECRET_STORE_NAMESPACE,
        &paired_handle_key,
    )
    .unwrap();
    assert!(store.get_secret(&paired_handle).unwrap().is_some());

    set_portable_data_dir_override(previous);
}
