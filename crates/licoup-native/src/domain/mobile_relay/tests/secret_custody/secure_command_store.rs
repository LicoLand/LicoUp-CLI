use super::super::test_support::*;
#[test]
fn secure_command_create_rejects_raw_runtime_e2ee_secret_overrides() {
    let dir = temp_dir("mobile-relay-secure-command-raw-runtime-e2ee-overrides");
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
    let signing_key = test_runtime_e2ee_secret(
        stringify!(&mobile_config),
        MobileRelayE2eeSecretField::SigningKey,
    );
    let signed_prekey_private_key = test_runtime_e2ee_secret(
        stringify!(&mobile_config),
        MobileRelayE2eeSecretField::SignedPrekeyPrivateKey,
    );
    let one_time_prekey_private_key = test_runtime_e2ee_secret(
        stringify!(&mobile_config),
        MobileRelayE2eeSecretField::OneTimePrekeyPrivateKey,
    );
    let one_time_mlkem1024_prekey_seed = test_runtime_e2ee_secret(
        stringify!(&mobile_config),
        MobileRelayE2eeSecretField::OneTimeMlKem1024PrekeySeed,
    );
    let pairing_secret = test_runtime_e2ee_secret(
        stringify!(&mobile_config),
        MobileRelayE2eeSecretField::PairingSecret,
    );
    mobile_config["pairingId"] = json!("pair_raw_runtime_e2ee_override");
    mobile_config["mobileToken"] = json!("");
    mobile_config["mobileRelayE2ee"]
        .as_object_mut()
        .unwrap()
        .remove("privateKeyBase64url");
    mobile_config["mobileRelayE2ee"]
        .as_object_mut()
        .unwrap()
        .remove("signingKeyBase64url");
    mobile_config["mobileRelayE2ee"]
        .as_object_mut()
        .unwrap()
        .remove("signedPrekeyPrivateKeyBase64url");
    mobile_config["mobileRelayE2ee"]
        .as_object_mut()
        .unwrap()
        .remove("oneTimePrekeyPrivateKeyBase64url");
    mobile_config["mobileRelayE2ee"]
        .as_object_mut()
        .unwrap()
        .remove("oneTimeMlKem1024PrekeySeedBase64url");
    mobile_config["mobileRelayE2ee"]
        .as_object_mut()
        .unwrap()
        .remove("pairingSecretBase64url");
    mobile_config["mobileRelayE2ee"]["privateKeyMaterial"] = json!("redacted");
    mobile_config["mobileRelayE2ee"]["signingKeyMaterial"] = json!("redacted");
    mobile_config["mobileRelayE2ee"]["signedPrekeyPrivateKeyMaterial"] = json!("redacted");
    mobile_config["mobileRelayE2ee"]["oneTimePrekeyPrivateKeyMaterial"] = json!("redacted");
    mobile_config["mobileRelayE2ee"]["oneTimeMlKem1024PrekeySeedMaterial"] = json!("redacted");
    mobile_config["mobileRelayE2ee"]["pairingSecretMaterial"] = json!("redacted");
    save_config(&mut mobile_config).unwrap();

    let error = command_create_secure(&json!({
        "commandKind": "agent.message.send",
        "targetAgentId": "codex",
        "workspaceId": "default",
        "body": {
            "text": "raw-runtime-override-plaintext-canary"
        },
        "secretOverrideTransport": RUNTIME_SECRET_OVERRIDE_TRANSPORT,
        "secretOverrides": {
            "mobileRelayE2ee": {
                "privateKeyBase64url": private_key,
                "signingKeyBase64url": signing_key,
                "signedPrekeyPrivateKeyBase64url": signed_prekey_private_key,
                "oneTimePrekeyPrivateKeyBase64url": one_time_prekey_private_key,
                "oneTimeMlKem1024PrekeySeedBase64url": one_time_mlkem1024_prekey_seed,
                "pairingSecretBase64url": pairing_secret
            }
        }
    }))
    .unwrap_err();
    assert!(
        error
            .to_string()
            .contains("raw E2EE secretOverrides are disabled")
    );

    let persisted = serde_json::to_string(&load_config().unwrap()).unwrap();
    for canary in [
        "raw-runtime-override-plaintext-canary",
        private_key.as_str(),
        signing_key.as_str(),
        signed_prekey_private_key.as_str(),
        one_time_prekey_private_key.as_str(),
        one_time_mlkem1024_prekey_seed.as_str(),
        pairing_secret.as_str(),
    ] {
        assert!(
            !persisted.contains(canary),
            "raw runtime E2EE override leaked to config: {canary}"
        );
    }
    set_portable_data_dir_override(previous);
}

#[test]
fn secure_command_create_uses_mobile_relay_secret_store_override_without_raw_e2ee_json() {
    let dir = temp_dir("mobile-relay-secure-command-secret-store-override");
    let previous = set_portable_data_dir_override(Some(dir.to_path_buf()));

    let store = Arc::new(EphemeralSecretStore::new());
    let mut pc_config = default_config();
    let mut mobile_config = default_config();
    let setup_store_override: Arc<dyn SecureMeshSecretStore> = store.clone();
    with_mobile_relay_secret_store_override(setup_store_override, || {
        pair_mobile_relay_configs(&mut pc_config, &mut mobile_config);
        Ok(())
    })
    .unwrap();
    let private_key = test_runtime_e2ee_secret(
        stringify!(&mobile_config),
        MobileRelayE2eeSecretField::PrivateKey,
    );
    let signing_key = test_runtime_e2ee_secret(
        stringify!(&mobile_config),
        MobileRelayE2eeSecretField::SigningKey,
    );
    let signed_prekey_private_key = test_runtime_e2ee_secret(
        stringify!(&mobile_config),
        MobileRelayE2eeSecretField::SignedPrekeyPrivateKey,
    );
    let one_time_prekey_private_key = test_runtime_e2ee_secret(
        stringify!(&mobile_config),
        MobileRelayE2eeSecretField::OneTimePrekeyPrivateKey,
    );
    let one_time_mlkem1024_prekey_seed = test_runtime_e2ee_secret(
        stringify!(&mobile_config),
        MobileRelayE2eeSecretField::OneTimeMlKem1024PrekeySeed,
    );
    // Start the bounded station only after the expensive pairing setup. Under
    // a fully parallel suite, starting it first can exhaust its accept window
    // before the request is ready and turn a valid transport assertion flaky.
    let station = CanonicalStation::start(1, Vec::new());
    mobile_config["pairingId"] = json!("pair_secret_store_override_station");
    mobile_config["mobileToken"] = json!("mobile-token-secret-store-override-canary");
    mobile_config["relayEnabled"] = json!(true);
    mobile_config["stationBaseUrl"] = json!(station.url());
    let setup_store_override: Arc<dyn SecureMeshSecretStore> = store.clone();
    with_mobile_relay_secret_store_override(setup_store_override, || {
        save_test_config_with_runtime_secret_context(&mut mobile_config, stringify!(&mobile_config))
    })
    .unwrap();

    let store_override: Arc<dyn SecureMeshSecretStore> = store.clone();
    let create_response = with_mobile_relay_secret_store_override(store_override, || {
        command_create_secure(&with_station_params(json!({
            "commandKind": "agent.message.send",
            "targetAgentId": "codex",
            "workspaceId": "default",
            "body": {
                "text": "secret-store-override-plaintext-canary"
            },
            "secretOverrideTransport": RUNTIME_SECRET_OVERRIDE_TRANSPORT,
            "secretOverrides": {
                "mobileRelayE2eeSecretStore": {
                    "contract": "rust_secure_mesh_secret_store_handle_v1",
                    "namespace": MOBILE_RELAY_PLATFORM_SECRET_STORE_NAMESPACE,
                    "rawJsonSecretOverridesUsed": false
                }
            }
        })))
    })
    .unwrap();
    assert_eq!(create_response["ok"], true);

    let request = station.request_body(0);
    let request_body = serde_json::from_str::<Value>(&request).unwrap();
    assert_eq!(
        request_body
            .as_object()
            .unwrap()
            .keys()
            .map(String::as_str)
            .collect::<std::collections::BTreeSet<_>>(),
        [
            "ciphertext",
            "contractVersion",
            "envelopeId",
            "expiresAt",
            "mailboxId",
        ]
        .into_iter()
        .collect()
    );
    assert!(!request.contains(SECURE_MESH_ENVELOPE_COMMAND));
    for canary in [
        "secret-store-override-plaintext-canary",
        "mobile-token-secret-store-override-canary",
        private_key.as_str(),
        signing_key.as_str(),
        signed_prekey_private_key.as_str(),
        one_time_prekey_private_key.as_str(),
        one_time_mlkem1024_prekey_seed.as_str(),
        "privateKeyBase64url",
        "signingKeyBase64url",
        "signedPrekeyPrivateKeyBase64url",
        "oneTimePrekeyPrivateKeyBase64url",
        "oneTimeMlKem1024PrekeySeedBase64url",
    ] {
        assert!(
            !request.contains(canary),
            "secret-store override request leaked {canary}"
        );
    }
    let persisted = serde_json::to_string(&load_config().unwrap()).unwrap();
    for secret in [
        private_key.as_str(),
        signing_key.as_str(),
        signed_prekey_private_key.as_str(),
        one_time_prekey_private_key.as_str(),
        one_time_mlkem1024_prekey_seed.as_str(),
    ] {
        assert!(!persisted.contains(secret));
    }

    station.assert_operations(&[BadTowerStationOperation::SendEnvelope]);
    station.join();
    set_portable_data_dir_override(previous);
}
