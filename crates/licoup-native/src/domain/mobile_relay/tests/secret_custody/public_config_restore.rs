use super::super::test_support::*;
#[test]
fn public_config_save_preserves_internal_mobile_token() {
    let dir = temp_dir("mobile-relay-preserve-token");
    let previous = set_portable_data_dir_override(Some(dir.to_path_buf()));
    let mut config = default_config();
    let store = Arc::new(EphemeralSecretStore::new());
    config["pairingId"] = json!("pair-preserve");
    config["mobileToken"] = json!("mobile-token-preserve-canary");
    let store_override: Arc<dyn SecureMeshSecretStore> = store.clone();
    let saved = with_mobile_relay_secret_store_override(store_override, || {
        save_config(&mut config)?;
        config_set(&json!({
            "pairingId": "pair-preserve",
            "mobileToken": "",
            "paired": true
        }))
    })
    .unwrap();
    assert_eq!(saved["config"]["mobileToken"], "");
    assert_eq!(
        saved["config"]["mobileTokenPresent"], true,
        "selected credential presence was not projected: {saved}"
    );

    let internal = load_config().unwrap();
    assert_eq!(internal["mobileToken"], "");
    let handle = native_secret_store_handle_for_namespace(
        MOBILE_RELAY_PLATFORM_SECRET_STORE_NAMESPACE,
        "mobileToken",
    )
    .unwrap();
    assert!(store.get_secret(&handle).unwrap().is_some());
    set_portable_data_dir_override(previous);
}

#[test]
fn native_secret_store_restores_selected_device_without_raw_json_overrides() {
    let dir = temp_dir("mobile-relay-native-secret-store-selected-device");
    let previous = set_portable_data_dir_override(Some(dir.to_path_buf()));
    let store = Arc::new(EphemeralSecretStore::new());
    let mut config = default_config();
    config["pairingId"] = json!("pair-active");
    config["mobileToken"] = json!("");
    config["pairedDevices"] = json!([
        {
            "id": "pc-selected",
            "pcClientId": "pc-selected",
            "pcClientName": "Selected Mac",
            "pairingId": "pair-selected",
            "mobileToken": "paired-device-secret-store-canary",
            "credentialPresent": true
        }
    ]);
    persist_config_secret_material_to_secret_store(
        &mut config,
        store.as_ref(),
        MOBILE_RELAY_PLATFORM_SECRET_STORE_NAMESPACE,
    )
    .unwrap();
    save_config(&mut config).unwrap();
    let baseline_session_count = store.authorization_session_count();

    let store_override: Arc<dyn SecureMeshSecretStore> = store.clone();
    let saved = with_mobile_relay_secret_store_override(store_override, || {
        config_set(&json!({
            "pairingId": "pair-selected",
            "mobileToken": "",
            "paired": true,
            "secretOverrideTransport": RUNTIME_SECRET_OVERRIDE_TRANSPORT,
            "secretOverrides": {
                "mobileRelayE2eeSecretStore": {
                    "contract": "rust_secure_mesh_secret_store_handle_v1",
                    "namespace": MOBILE_RELAY_PLATFORM_SECRET_STORE_NAMESPACE,
                    "rawJsonSecretOverridesUsed": false
                }
            }
        }))
    })
    .unwrap();

    assert_eq!(
        saved["config"]["mobileTokenPresent"], true,
        "selected credential presence was not projected: {saved}"
    );
    assert_eq!(
        saved["config"]["pairedDevices"][0]["credentialPresent"],
        true
    );
    assert_eq!(
        store.authorization_session_count(),
        baseline_session_count + 1
    );
    let persisted = load_config().unwrap();
    let serialized = serde_json::to_string(&persisted).unwrap();
    assert_eq!(persisted["mobileToken"], "");
    assert_eq!(persisted["pairedDevices"][0]["mobileToken"], "");
    assert!(!serialized.contains("paired-device-secret-store-canary"));
    assert_eq!(
        persisted["secretStorageStatus"]["selectedBackend"],
        "memory-only-ephemeral"
    );
    let paired_handle_key =
        paired_device_token_secret_store_key(&persisted["pairedDevices"][0]).unwrap();
    let paired_handle = native_secret_store_handle_for_namespace(
        MOBILE_RELAY_PLATFORM_SECRET_STORE_NAMESPACE,
        &paired_handle_key,
    )
    .unwrap();
    assert!(store.get_secret(&paired_handle).unwrap().is_some());

    set_portable_data_dir_override(previous);
}

#[test]
fn runtime_secret_overrides_require_platform_transport_marker() {
    let dir = temp_dir("mobile-relay-runtime-secret-overrides-marker");
    let previous = set_portable_data_dir_override(Some(dir.to_path_buf()));
    let mut config = default_config();
    config["pairingId"] = json!("pair-active");
    config["mobileToken"] = json!("");
    config["pairedDevices"] = json!([
        {
            "id": "pc-selected",
            "pcClientId": "pc-selected",
            "pcClientName": "Selected Mac",
            "pairingId": "pair-selected",
            "mobileToken": "",
            "credentialPresent": true
        }
    ]);
    save_config(&mut config).unwrap();

    let saved = config_set(&json!({
        "pairingId": "pair-selected",
        "mobileToken": "",
        "paired": true,
        "secretOverrides": {
            "pairedDevices": [
                {
                    "id": "pc-selected",
                    "pairingId": "pair-selected",
                    "mobileToken": "untrusted-runtime-override-canary"
                }
            ]
        }
    }))
    .unwrap();

    assert_eq!(saved["config"]["mobileTokenPresent"], false);
    assert_eq!(
        saved["config"]["pairedDevices"][0]["credentialPresent"],
        true
    );
    let persisted = load_config().unwrap();
    let serialized = serde_json::to_string(&persisted).unwrap();
    assert_eq!(persisted["mobileToken"], "");
    assert_eq!(persisted["pairedDevices"][0]["mobileToken"], "");
    assert!(!serialized.contains("untrusted-runtime-override-canary"));
    assert!(persisted.get("secretStorageStatus").is_none());

    set_portable_data_dir_override(previous);
}

#[test]
fn runtime_secret_overrides_reject_raw_token_fields() {
    let mut config = default_config();
    let token_error = match apply_runtime_secret_overrides(
        &mut config,
        &json!({
            "secretOverrideTransport": RUNTIME_SECRET_OVERRIDE_TRANSPORT,
            "secretOverrides": {
                "mobileToken": "mobile-token-raw-override-canary"
            }
        }),
    ) {
        Ok(_) => panic!("raw token secretOverrides must be rejected"),
        Err(error) => format!("{error}"),
    };
    assert!(token_error.contains("raw token secretOverrides are disabled"));

    let paired_error = match apply_runtime_secret_overrides(
        &mut config,
        &json!({
            "secretOverrideTransport": RUNTIME_SECRET_OVERRIDE_TRANSPORT,
            "secretOverrides": {
                "pairedDevices": [
                    {
                        "id": "pc-selected",
                        "pairingId": "pair-selected",
                        "mobileToken": "paired-token-raw-override-canary"
                    }
                ]
            }
        }),
    ) {
        Ok(_) => panic!("raw paired-device token secretOverrides must be rejected"),
        Err(error) => format!("{error}"),
    };
    assert!(paired_error.contains("raw token secretOverrides are disabled"));
}

#[test]
fn e2ee_status_rejects_private_key_material_in_portable_config() {
    let dir = temp_dir("mobile-relay-e2ee-status-portable-secret-store");
    let previous = set_portable_data_dir_override(Some(dir.to_path_buf()));
    let mut pc_config = default_config();
    let pc_descriptor = ensure_mobile_relay_endpoint_descriptor(
        &mut pc_config,
        &mut test_runtime_secret_material(stringify!(&mut pc_config)),
        "desktop_sidecar",
    )
    .unwrap();
    let mut mobile_config = default_config();
    ensure_mobile_relay_endpoint_descriptor(
        &mut mobile_config,
        &mut test_runtime_secret_material(stringify!(&mut mobile_config)),
        "mobile",
    )
    .unwrap();
    apply_peer_secure_mesh_descriptor(
        &mut mobile_config,
        &mut test_runtime_secret_material(stringify!(&mut mobile_config)),
        &pc_descriptor,
        true,
    )
    .unwrap();
    let private_key = test_runtime_e2ee_secret(
        stringify!(&mobile_config),
        MobileRelayE2eeSecretField::PrivateKey,
    );
    mobile_config["mobileRelayE2ee"]["privateKeyBase64url"] = json!(private_key.clone());
    save_config_raw(&mut mobile_config).unwrap();

    let status = e2ee_status(&json!({})).unwrap();

    assert_eq!(status["secureSessionEstablished"], false);
    assert_eq!(
        status["secretStore"]["selectedBackend"],
        "unsafe_portable_config"
    );
    assert_eq!(status["secretStore"]["privateKeyInSelectedCustody"], false);
    assert_eq!(
        status["secretStore"]["portableConfigPrivateKeyPresent"],
        true
    );
    assert_eq!(status["secretStore"]["unsafePersistenceDetected"], true);
    assert_eq!(
        status["secretStore"]["authorization"]["appPasswordPromptUsed"],
        false
    );
    assert_eq!(
        status["secretStore"]["custodyReason"],
        "secret_material_in_portable_config"
    );
    let serialized = serde_json::to_string(&status).unwrap();
    assert!(!serialized.contains(&private_key));

    set_portable_data_dir_override(previous);
}
