use super::test_support::*;

#[test]
fn mobile_relay_config_requires_one_canonical_station() {
    let dir = temp_dir("mobile-relay");
    let previous = set_portable_data_dir_override(Some(dir.to_path_buf()));

    let config = config_get(&json!({})).unwrap();
    assert_eq!(config["config"]["stationBaseUrl"], "");
    assert_eq!(config["config"]["relayEnabled"], false);
    let error = config_set(&json!({"relayEnabled": true}))
        .unwrap_err()
        .to_string();
    assert!(error.contains("station is not configured"));

    let saved = config_set(&json!({
        "stationBaseUrl": "https://station.example.test/",
        "relayEnabled": "true"
    }))
    .unwrap();
    assert_eq!(
        saved["config"]["stationBaseUrl"],
        "https://station.example.test"
    );
    assert_eq!(saved["config"]["relayEnabled"], true);

    set_portable_data_dir_override(previous);
}

#[test]
fn config_reset_pairing_clears_local_pairing_without_resetting_identity_or_station() {
    let dir = temp_dir("mobile-relay-reset-pairing");
    let previous = set_portable_data_dir_override(Some(dir.to_path_buf()));
    let mut config = default_config();
    config["stationBaseUrl"] = json!("https://station.example.test");
    config["pcClientId"] = json!("pc-stable");
    config["pcClientName"] = json!("Stable Mac");
    config["pairingId"] = json!("pair-stale");
    config["pcToken"] = json!("pc-token-stale");
    config["mobileToken"] = json!("mobile-token-stale");
    config["lastPairingCode"] = json!("123456");
    config["lastPairingExpiresAt"] = json!("2099-01-01T00:00:00Z");
    config["paired"] = json!(true);
    config["relayEnabled"] = json!(true);
    ensure_mobile_relay_endpoint_descriptor(
        &mut config,
        &mut test_runtime_secret_material(stringify!(&mut config)),
        "desktop_sidecar",
    )
    .unwrap();
    let endpoint_id = config["mobileRelayE2ee"]["endpointId"]
        .as_str()
        .unwrap()
        .to_string();
    let public_key = config["mobileRelayE2ee"]["publicKeyBase64url"]
        .as_str()
        .unwrap()
        .to_string();
    let session_id = config["mobileRelayE2ee"]["sessionId"]
        .as_str()
        .unwrap()
        .to_string();
    config["mobileRelayE2ee"]["peerEndpointId"] = json!("mobile-stale");
    config["mobileRelayE2ee"]["peerEndpointKind"] = json!("mobile");
    config["mobileRelayE2ee"]["peerPublicKeyBase64url"] = json!(random_base64url(32));
    config["mobileRelayE2ee"]["peerFingerprint"] = json!("peer-fingerprint-stale");
    config["mobileRelayE2ee"]["peerVerified"] = json!(true);
    config["mobileRelayPairingInvite"] = json!({"pairingId": "pair-stale"});
    config["pairedDevices"] = json!([{"pairingId": "pair-stale"}]);
    save_config(&mut config).unwrap();

    let saved = config_set(&json!({"resetPairing": true})).unwrap();
    assert_eq!(
        saved["config"]["stationBaseUrl"],
        "https://station.example.test"
    );
    assert_eq!(saved["config"]["pcClientId"], "pc-stable");
    assert_eq!(saved["config"]["pcClientName"], "Stable Mac");
    assert_eq!(saved["config"]["pairingId"], "");
    assert_eq!(saved["config"]["pcTokenPresent"], false);
    assert_eq!(saved["config"]["mobileTokenPresent"], false);
    assert_eq!(saved["config"]["paired"], false);
    assert_eq!(saved["config"]["relayEnabled"], false);
    assert!(saved["config"].get("pairedDevices").is_none());
    assert!(saved["config"].get("mobileRelayPairingInvite").is_none());

    let (internal, _) = load_config_with_runtime_secret_overrides(&json!({})).unwrap();
    assert_eq!(internal["mobileRelayE2ee"]["endpointId"], endpoint_id);
    assert_eq!(
        internal["mobileRelayE2ee"]["publicKeyBase64url"],
        public_key
    );
    assert_eq!(internal["mobileRelayE2ee"]["peerVerified"], false);
    assert!(internal["mobileRelayE2ee"].get("peerEndpointId").is_none());
    assert_ne!(internal["mobileRelayE2ee"]["sessionId"], session_id);
    assert!(
        internal["mobileRelayE2ee"]
            .get("pairingSecretBase64url")
            .is_none()
    );

    set_portable_data_dir_override(previous);
}

#[test]
fn e2ee_status_redacts_pairing_invite_secret() {
    let dir = temp_dir("mobile-relay-e2ee-status-redacts-pairing-invite");
    let previous = set_portable_data_dir_override(Some(dir.to_path_buf()));
    let mut config = default_config();
    let endpoint = ensure_mobile_relay_endpoint_descriptor(
        &mut config,
        &mut test_runtime_secret_material(stringify!(&mut config)),
        "desktop_sidecar",
    )
    .unwrap();
    config["mobileRelayPairingInvite"] = json!({
        "protocolVersion": MOBILE_RELAY_E2EE_PROTOCOL_VERSION,
        "oneTime": true,
        "createdAt": "2026-07-04T00:00:00Z",
        "stationBaseUrl": "https://station.example.test",
        "pcClientId": "pc-redacted-invite",
        "pcClientName": "LicoUp",
        "pairingId": "pair-redacted-invite",
        "pairingCode": "ABCDE-FGHIJ-KLMNO-PQRST",
        "pairingCodeHash": sha256_hex("ABCDE-FGHIJ-KLMNO-PQRST".as_bytes()),
        "pcSecureMesh": endpoint,
        "e2eePairingSecret": "pairing-invite-e2ee-secret-redaction-canary"
    });

    let pairing_invite = redacted_pairing_invite(config.get("mobileRelayPairingInvite"));
    assert_eq!(pairing_invite["e2eePairingSecretMaterial"], "redacted");
    assert!(pairing_invite.get("e2eePairingSecret").is_none());

    save_config(&mut config).unwrap();

    let status = e2ee_status(&json!({})).unwrap();

    let serialized = serde_json::to_string(&status).unwrap();
    assert!(!serialized.contains("pairing-invite-e2ee-secret-redaction-canary"));

    set_portable_data_dir_override(previous);
}

#[test]
fn config_load_clears_persisted_pairing_invite_and_code() {
    let dir = temp_dir("mobile-relay-clears-persisted-invite");
    let previous = set_portable_data_dir_override(Some(dir.to_path_buf()));
    let mut config = default_config();
    ensure_mobile_relay_endpoint_descriptor(
        &mut config,
        &mut test_runtime_secret_material(stringify!(&mut config)),
        "desktop_sidecar",
    )
    .unwrap();
    config["lastPairingCode"] = json!("ABCDE-FGHIJ-KLMNO-PQRST");
    config["lastPairingExpiresAt"] = json!("2099-01-01T00:00:00Z");
    config["mobileRelayPairingInvite"] = json!({
        "protocolVersion": MOBILE_RELAY_E2EE_PROTOCOL_VERSION,
        "pairingId": "pair-redacted-invite",
        "pairingCode": "ABCDE-FGHIJ-KLMNO-PQRST",
        "e2eePairingSecret": "pairing-invite-secret-status-canary"
    });
    save_config(&mut config).unwrap();

    let loaded = config_get(&json!({
        "authorize": true
    }))
    .unwrap();

    assert_eq!(loaded["config"]["lastPairingCode"], "");
    assert_eq!(loaded["config"]["lastPairingExpiresAt"], "");
    assert!(loaded["config"].get("mobileRelayPairingInvite").is_none());
    let persisted =
        serde_json::from_str::<Value>(&fs::read_to_string(config_path().unwrap()).unwrap())
            .unwrap();
    assert_eq!(persisted["lastPairingCode"], "");
    assert_eq!(persisted["lastPairingExpiresAt"], "");
    assert!(persisted.get("mobileRelayPairingInvite").is_none());
    let serialized = serde_json::to_string(&loaded).unwrap();
    assert!(!serialized.contains("pairing-invite-secret-status-canary"));

    set_portable_data_dir_override(previous);
}

#[test]
fn invalid_station_is_rejected_before_config_persistence() {
    let dir = temp_dir("mobile-relay-invalid");
    let previous = set_portable_data_dir_override(Some(dir.to_path_buf()));
    for denied in [
        "https://",
        "https://?station=station.example.test",
        "https://user@station.example.test",
        "https://station.example.test#fragment",
        "https://station.example.test:invalid",
        "https://station.example.test/api",
        "https://station.example.test?scope=one",
        "https://station.example.test\\@evil.test",
        "http://example.test",
        "http://localhost.evil.test",
        "http://127.0.0.1@evil.test",
        "http://127.1",
    ] {
        let result = config_set(&json!({
            "stationBaseUrl": denied
        }));
        assert!(result.is_err(), "accepted disallowed station");
        assert!(!config_path().unwrap().exists());
    }
    set_portable_data_dir_override(previous);
}

#[test]
fn station_origins_are_canonicalized_and_exact_loopback_http_is_allowed() {
    for (input, expected) in [
        (
            "HTTPS://Station.Example.Test:443/",
            "https://station.example.test",
        ),
        ("http://127.0.0.1:7228/", "http://127.0.0.1:7228"),
        ("http://localhost:7228", "http://localhost:7228"),
        ("http://[::1]:7228/", "http://[::1]:7228"),
    ] {
        assert_eq!(validated_station_base_url(input).unwrap(), expected);
    }
}

#[test]
fn invalid_pairing_invite_station_cannot_mutate_existing_pairing_state() {
    let mut config = default_config();
    config["pairingId"] = json!("existing-pairing");
    config["paired"] = json!(true);
    let before = config.clone();

    let result = apply_pairing_invite_params(
        &mut config,
        &json!({
            "invite": {
                "pairingId": "replacement-pairing",
                "stationBaseUrl": "https://trusted.example@evil.test#fragment"
            }
        }),
    );

    assert!(result.is_err());
    assert_eq!(config, before);
}

#[test]
fn pairing_create_returns_one_time_invite_without_persisting_code() {
    let dir = temp_dir("mobile-relay-one-time-create");
    let previous = set_portable_data_dir_override(Some(dir.to_path_buf()));

    config_set(&json!({
        "stationBaseUrl": "https://station.example.test",
        "pcClientId": "pc-one-time",
        "pcClientName": "LicoUp"
    }))
    .unwrap();
    let output = pairing_create(&with_station_params(json!({"targets": []}))).unwrap();

    assert_eq!(
        output["mobileRelayPairingInvite"]["pairingCode"],
        output["pairingCode"]
    );
    assert!(
        output["pairingCode"]
            .as_str()
            .is_some_and(|value| value.len() == 16)
    );
    assert_eq!(output["mobileRelayPairingInvite"]["oneTime"], true);
    assert_eq!(output["config"]["lastPairingCode"], "");
    assert_eq!(output["config"]["lastPairingExpiresAt"], "");
    assert!(output["config"].get("mobileRelayPairingInvite").is_none());

    let persisted =
        serde_json::from_str::<Value>(&fs::read_to_string(config_path().unwrap()).unwrap())
            .unwrap();
    assert_eq!(persisted["lastPairingCode"], "");
    assert_eq!(persisted["lastPairingExpiresAt"], "");
    assert!(persisted.get("mobileRelayPairingInvite").is_none());

    let invite = &output["mobileRelayPairingInvite"];
    assert!(invite["createdAt"].as_str().unwrap().contains('T'));
    assert!(invite["pairingCodeHash"].as_str().unwrap().len() >= 64);
    assert!(invite["pcSecureMesh"].is_object());
    assert!(
        invite["e2eePairingSecret"]
            .as_str()
            .is_some_and(|value| !value.is_empty())
    );
    assert_eq!(output["serverVisiblePairingState"], false);
    set_portable_data_dir_override(previous);
}

#[test]
fn pairing_claim_sends_one_time_context_and_clears_code() {
    let dir = temp_dir("mobile-relay-one-time-claim");
    let previous = set_portable_data_dir_override(Some(dir.to_path_buf()));

    let mut pc_config = default_config();
    let pc_descriptor = ensure_mobile_relay_endpoint_descriptor(
        &mut pc_config,
        &mut test_runtime_secret_material(stringify!(&mut pc_config)),
        "desktop_sidecar",
    )
    .unwrap();
    let pairing_secret = random_base64url(MOBILE_RELAY_KEY_BYTES);

    config_set(&json!({
        "stationBaseUrl": "https://station.example.test",
        "pcClientName": "LicoUp"
    }))
    .unwrap();

    let output = pairing_claim(&with_station_params(json!({
        "invite": {
            "protocolVersion": MOBILE_RELAY_E2EE_PROTOCOL_VERSION,
            "oneTime": true,
            "stationBaseUrl": "https://station.example.test",
            "pcClientId": "pc-one-time",
            "pcClientName": "LicoUp",
            "pairingId": "pair-one-time",
            "pairingCode": "ABCDE-FGHIJ-KLMNO-PQRST",
            "pcSecureMesh": pc_descriptor,
            "e2eePairingSecret": pairing_secret
        },
        "mobileDeviceName": "LicoUp Mobile",
        "mobileDeviceId": "mobile-one-time",
        "platform": "ios"
    })))
    .unwrap();

    assert_eq!(output["ok"], true);
    assert_eq!(output["config"]["lastPairingCode"], "");
    assert_eq!(output["config"]["lastPairingExpiresAt"], "");
    assert!(output["config"].get("mobileRelayPairingInvite").is_none());
    let persisted =
        serde_json::from_str::<Value>(&fs::read_to_string(config_path().unwrap()).unwrap())
            .unwrap();
    assert_eq!(persisted["lastPairingCode"], "");
    assert_eq!(persisted["lastPairingExpiresAt"], "");
    assert!(persisted.get("mobileRelayPairingInvite").is_none());

    assert_eq!(output["serverVisiblePairingState"], false);
    set_portable_data_dir_override(previous);
}

#[test]
fn pairing_claim_invite_e2ee_secret_completes_mobile_endpoint_descriptor() {
    let dir = temp_dir("mobile-relay-one-time-claim-invite-e2ee-secret");
    let previous = set_portable_data_dir_override(Some(dir.to_path_buf()));

    let mut pc_config = default_config();
    let pc_descriptor = ensure_mobile_relay_endpoint_descriptor(
        &mut pc_config,
        &mut test_runtime_secret_material(stringify!(&mut pc_config)),
        "desktop_sidecar",
    )
    .unwrap();
    let pairing_secret = random_base64url(MOBILE_RELAY_KEY_BYTES);

    config_set(&json!({
        "stationBaseUrl": "https://station.example.test",
        "pcClientName": "LicoUp"
    }))
    .unwrap();

    let output = pairing_claim(&with_station_params(json!({
        "invite": {
            "protocolVersion": MOBILE_RELAY_E2EE_PROTOCOL_VERSION,
            "oneTime": true,
            "stationBaseUrl": "https://station.example.test",
            "pcClientId": "pc-runtime-override",
            "pcClientName": "LicoUp",
            "pairingId": "pair-one-time",
            "pairingCode": "ABCDE-FGHIJ-KLMNO-PQRST",
            "pcSecureMesh": pc_descriptor,
            "e2eePairingSecret": pairing_secret
        },
        "mobileDeviceName": "LicoUp Android",
        "mobileDeviceId": "mobile-invite-e2ee-secret",
        "platform": "android"
    })))
    .unwrap();

    assert_eq!(output["ok"], true);
    assert_eq!(
        output["config"]["mobileRelayE2ee"]["endpointKind"],
        "mobile"
    );
    assert!(
        output["config"]["mobileRelayE2ee"]["endpointId"]
            .as_str()
            .unwrap()
            .starts_with("mobile_")
    );
    assert_eq!(
        output["config"]["mobileRelayE2ee"]["peerEndpointId"],
        pc_descriptor["endpointId"]
    );
    assert_eq!(
        output["config"]["mobileRelayE2ee"]["pairingSecretMaterial"],
        "redacted"
    );
    assert!(
        output["config"]["mobileRelayE2ee"]
            .get("pairingSecretBase64url")
            .is_none()
    );

    assert_eq!(
        output["outOfBandPairingResponse"]["mobileSecureMesh"]["endpointId"],
        output["config"]["mobileRelayE2ee"]["endpointId"]
    );
    let serialized_output = serde_json::to_string(&output).unwrap();
    assert!(!serialized_output.contains(&pairing_secret));
    assert_eq!(output["serverVisiblePairingState"], false);
    set_portable_data_dir_override(previous);
}

#[test]
fn new_pairing_invite_resets_stale_mobile_pairwise_state() {
    let dir = temp_dir("mobile-relay-new-invite-resets-pairwise-state");
    let previous = set_portable_data_dir_override(Some(dir.to_path_buf()));

    let mut pc_config = default_config();
    let mut mobile_config = default_config();
    pair_mobile_relay_configs(&mut pc_config, &mut mobile_config);
    pc_config["pairingId"] = json!("pair-old");
    mobile_config["pairingId"] = json!("pair-old");
    let stale_session_id = mobile_config["mobileRelayE2ee"]["sessionId"]
        .as_str()
        .unwrap()
        .to_string();
    assert!(
        mobile_config["mobileRelayE2ee"]
            .get("pendingPairwiseIntro")
            .is_none()
    );

    let pc_descriptor = ensure_mobile_relay_endpoint_descriptor(
        &mut pc_config,
        &mut test_runtime_secret_material(stringify!(&mut pc_config)),
        "desktop_sidecar",
    )
    .unwrap();
    let pairing_secret = random_base64url(MOBILE_RELAY_KEY_BYTES);
    let invite_params = json!({
        "invite": {
            "protocolVersion": MOBILE_RELAY_E2EE_PROTOCOL_VERSION,
            "oneTime": true,
            "stationBaseUrl": "https://station.example.test",
            "pcClientId": "pc-repairing",
            "pcClientName": "LicoUp",
            "pairingId": "pair-new",
            "pairingCode": "ABCDE-FGHIJ-KLMNO-PQRST",
            "pcSecureMesh": pc_descriptor.clone(),
            "e2eePairingSecret": pairing_secret
        }
    });
    let mut mobile_context = take_test_runtime_secret_context(stringify!(&mobile_config));
    apply_pairing_invite_params_with_context(
        &mut mobile_config,
        &invite_params,
        Some(&mut mobile_context),
    )
    .unwrap();
    restore_test_runtime_secret_context(stringify!(&mobile_config), mobile_context);

    assert_eq!(mobile_config["pairingId"], "pair-new");
    assert_eq!(
        test_runtime_e2ee_secret(
            stringify!(&mobile_config),
            MobileRelayE2eeSecretField::PairingSecret,
        ),
        pairing_secret
    );
    assert_eq!(mobile_config["mobileRelayE2ee"]["peerVerified"], true);
    assert_ne!(
        mobile_config["mobileRelayE2ee"]["sessionId"],
        stale_session_id
    );
    assert!(
        mobile_config["mobileRelayE2ee"]["pendingPairwiseIntro"].is_object(),
        "new mobile pairing must advertise a fresh pairwise intro"
    );
    assert!(
        mobile_config["mobileRelayE2ee"]
            .get("pairwiseAccepted")
            .is_none()
    );

    pc_config["pairingId"] = json!("pair-new");
    test_runtime_secret_material(stringify!(&pc_config))
        .replace_e2ee_secret(
            MobileRelayE2eeSecretField::PairingSecret,
            SecretBytes::try_from_string(pairing_secret).unwrap(),
        )
        .unwrap();
    let mobile_descriptor = ensure_mobile_relay_endpoint_descriptor(
        &mut mobile_config,
        &mut test_runtime_secret_material(stringify!(&mut mobile_config)),
        "mobile",
    )
    .unwrap();
    assert!(mobile_descriptor["pairwiseIntro"].is_object());
    let proof = mobile_relay_claim_proof_for_pair(
        &pc_config,
        &mut test_runtime_secret_material(stringify!(&pc_config)),
        "pair-new",
        &mobile_descriptor,
        &pc_descriptor,
    )
    .unwrap();
    apply_test_out_of_band_pairing_response(
        &mut pc_config,
        stringify!(&pc_config),
        &json!({
            "mobileSecureMesh": mobile_descriptor,
            "secureMeshClaimProof": proof
        }),
    )
    .unwrap();

    assert_eq!(pc_config["mobileRelayE2ee"]["peerVerified"], true);
    assert!(pc_config["mobileRelayE2ee"]["pairwiseAccepted"].is_object());

    set_portable_data_dir_override(previous);
}

#[test]
fn new_pairing_invite_resets_blank_pairing_id_with_stale_peer_state() {
    let dir = temp_dir("mobile-relay-new-invite-resets-blank-pairing-stale-peer");
    let previous = set_portable_data_dir_override(Some(dir.to_path_buf()));

    let mut pc_config = default_config();
    let mut mobile_config = default_config();
    pair_mobile_relay_configs(&mut pc_config, &mut mobile_config);
    mobile_config["pairingId"] = json!("");
    let stale_session_id = mobile_config["mobileRelayE2ee"]["sessionId"]
        .as_str()
        .unwrap()
        .to_string();
    assert!(
        mobile_config["mobileRelayE2ee"]["peerEndpointId"]
            .as_str()
            .unwrap()
            .starts_with("pc_")
    );

    let pc_descriptor = ensure_mobile_relay_endpoint_descriptor(
        &mut pc_config,
        &mut test_runtime_secret_material(stringify!(&mut pc_config)),
        "desktop_sidecar",
    )
    .unwrap();
    let pairing_secret = random_base64url(MOBILE_RELAY_KEY_BYTES);
    let invite_params = json!({
        "invite": {
            "protocolVersion": MOBILE_RELAY_E2EE_PROTOCOL_VERSION,
            "oneTime": true,
            "stationBaseUrl": "https://station.example.test",
            "pcClientId": "pc-repairing-blank",
            "pcClientName": "LicoUp",
            "pairingId": "pair-new-blank",
            "pairingCode": "ABCDE-FGHIJ-KLMNO-PQRST",
            "pcSecureMesh": pc_descriptor,
            "e2eePairingSecret": pairing_secret
        }
    });
    let mut mobile_context = take_test_runtime_secret_context(stringify!(&mobile_config));
    apply_pairing_invite_params_with_context(
        &mut mobile_config,
        &invite_params,
        Some(&mut mobile_context),
    )
    .unwrap();
    restore_test_runtime_secret_context(stringify!(&mobile_config), mobile_context);

    assert_eq!(mobile_config["pairingId"], "pair-new-blank");
    assert_eq!(
        test_runtime_e2ee_secret(
            stringify!(&mobile_config),
            MobileRelayE2eeSecretField::PairingSecret,
        ),
        pairing_secret
    );
    assert_ne!(
        mobile_config["mobileRelayE2ee"]["sessionId"],
        stale_session_id
    );
    assert!(mobile_config["mobileRelayE2ee"]["pendingPairwiseIntro"].is_object());
    assert!(
        mobile_config["mobileRelayE2ee"]
            .get("pairwiseAccepted")
            .is_none()
    );

    set_portable_data_dir_override(previous);
}

#[test]
fn pairing_claim_adopts_the_explicit_invite_station() {
    let dir = temp_dir("mobile-relay-invite-station-claim");
    let previous = set_portable_data_dir_override(Some(dir.to_path_buf()));

    let mut pc_config = default_config();
    let pc_descriptor = ensure_mobile_relay_endpoint_descriptor(
        &mut pc_config,
        &mut test_runtime_secret_material(stringify!(&mut pc_config)),
        "desktop_sidecar",
    )
    .unwrap();
    let pairing_secret = random_base64url(MOBILE_RELAY_KEY_BYTES);

    config_set(&json!({
        "stationBaseUrl": "https://fallback-station.example.test",
        "pcClientName": "LicoUp"
    }))
    .unwrap();

    let output = pairing_claim(&with_station_params(json!({
        "invite": {
            "protocolVersion": MOBILE_RELAY_E2EE_PROTOCOL_VERSION,
            "oneTime": true,
            "stationBaseUrl": "https://invite-station.example.test/",
            "pcClientId": "pc-one-time",
            "pcClientName": "LicoUp",
            "pairingId": "pair-one-time",
            "pairingCode": "ABCDE-FGHIJ-KLMNO-PQRST",
            "pcSecureMesh": pc_descriptor,
            "e2eePairingSecret": pairing_secret
        },
        "mobileDeviceName": "LicoUp Mobile",
        "mobileDeviceId": "mobile-one-time",
        "platform": "android"
    })))
    .unwrap();

    assert_eq!(output["ok"], true);
    assert_eq!(
        output["config"]["stationBaseUrl"],
        "https://invite-station.example.test"
    );
    assert_eq!(output["serverVisiblePairingState"], false);

    let persisted =
        serde_json::from_str::<Value>(&fs::read_to_string(config_path().unwrap()).unwrap())
            .unwrap();
    assert_eq!(
        persisted["stationBaseUrl"],
        "https://invite-station.example.test"
    );
    set_portable_data_dir_override(previous);
}

#[test]
fn out_of_band_pairing_response_rejects_tampered_intro_with_replayed_claim_proof() {
    let dir = temp_dir("mobile-relay-claim-proof-binds-intro");
    let previous = set_portable_data_dir_override(Some(dir.to_path_buf()));
    let pairing_id = "pair_intro_replay_rejected";
    let pairing_secret = random_base64url(MOBILE_RELAY_KEY_BYTES);
    let mut pc_config = default_config();
    pc_config["pairingId"] = json!(pairing_id);
    let pc_descriptor = ensure_mobile_relay_endpoint_descriptor(
        &mut pc_config,
        &mut test_runtime_secret_material(stringify!(&mut pc_config)),
        "desktop_sidecar",
    )
    .unwrap();
    test_runtime_secret_material(stringify!(&pc_config))
        .replace_e2ee_secret(
            MobileRelayE2eeSecretField::PairingSecret,
            SecretBytes::try_from_string(pairing_secret.clone()).unwrap(),
        )
        .unwrap();

    let mut mobile_config = default_config();
    mobile_config["pairingId"] = json!(pairing_id);
    ensure_mobile_relay_endpoint_descriptor(
        &mut mobile_config,
        &mut test_runtime_secret_material(stringify!(&mut mobile_config)),
        "mobile",
    )
    .unwrap();
    test_runtime_secret_material(stringify!(&mobile_config))
        .replace_e2ee_secret(
            MobileRelayE2eeSecretField::PairingSecret,
            SecretBytes::try_from_string(pairing_secret).unwrap(),
        )
        .unwrap();
    apply_peer_secure_mesh_descriptor(
        &mut mobile_config,
        &mut test_runtime_secret_material(stringify!(&mut mobile_config)),
        &pc_descriptor,
        true,
    )
    .unwrap();
    let mobile_descriptor = ensure_mobile_relay_endpoint_descriptor(
        &mut mobile_config,
        &mut test_runtime_secret_material(stringify!(&mut mobile_config)),
        "mobile",
    )
    .unwrap();
    let proof = mobile_relay_claim_proof_for_pair(
        &pc_config,
        &mut test_runtime_secret_material(stringify!(&pc_config)),
        pairing_id,
        &mobile_descriptor,
        &pc_descriptor,
    )
    .unwrap();
    let mut tampered_descriptor = mobile_descriptor;
    tampered_descriptor["pairwiseIntro"]["initiatorIdentityPublicKeyBase64url"] =
        json!(random_base64url(32));

    let error = apply_test_out_of_band_pairing_response(
        &mut pc_config,
        stringify!(&pc_config),
        &json!({
            "mobileSecureMesh": tampered_descriptor,
            "secureMeshClaimProof": proof
        }),
    )
    .unwrap_err()
    .to_string();
    assert!(error.contains("out-of-band claim proof is invalid"));

    assert!(
        peer_secure_mesh_descriptor(&pc_config).is_none(),
        "replayed claim proof must not verify a server-tampered pairwise intro"
    );
    assert_ne!(pc_config["mobileRelayE2ee"]["peerVerified"], true);
    assert!(
        pc_config["mobileRelayE2ee"]
            .get("pairwiseAccepted")
            .is_none()
    );

    set_portable_data_dir_override(previous);
}

#[test]
fn out_of_band_pairing_response_persists_revoked_peer_block_and_propagates_terminal_error() {
    let dir = temp_dir("mobile-relay-pairing-status-revoked-peer");
    let previous = set_portable_data_dir_override(Some(dir.to_path_buf()));
    let store = Arc::new(EphemeralSecretStore::new());
    let mobile_store: Arc<dyn SecureMeshSecretStore> = store.clone();
    let pairwise_store: Arc<dyn SecureMeshSecretStore> = store.clone();

    with_mobile_relay_secret_store_override(mobile_store, || {
        with_pairwise_secret_store_override(pairwise_store, || {
            let pairing_id = "pair-terminal-revocation";
            let pairing_secret = random_base64url(MOBILE_RELAY_KEY_BYTES);
            let mut pc_config = default_config();
            let mut mobile_config = default_config();
            pc_config["pairingId"] = json!(pairing_id);
            mobile_config["pairingId"] = json!(pairing_id);
            for variable in [stringify!(&pc_config), stringify!(&mobile_config)] {
                test_runtime_secret_material(variable).replace_e2ee_secret(
                    MobileRelayE2eeSecretField::PairingSecret,
                    SecretBytes::try_from_string(pairing_secret.clone())?,
                )?;
            }
            pair_mobile_relay_configs(&mut pc_config, &mut mobile_config);
            let local_endpoint_id = local_endpoint_state(
                &pc_config,
                &mut test_runtime_secret_material(stringify!(&pc_config)),
            )?
            .endpoint_id;
            let old_session_id = session_id(&pc_config)?;
            let mut revoked_mobile = ensure_mobile_relay_endpoint_descriptor(
                &mut mobile_config,
                &mut test_runtime_secret_material(stringify!(&mut mobile_config)),
                "mobile",
            )?;
            append_test_directory_state(&mut revoked_mobile, "revoked")?;
            let pc_descriptor = ensure_mobile_relay_endpoint_descriptor(
                &mut pc_config,
                &mut test_runtime_secret_material(stringify!(&mut pc_config)),
                "desktop_sidecar",
            )?;
            let proof = mobile_relay_claim_proof_for_pair(
                &pc_config,
                &mut test_runtime_secret_material(stringify!(&pc_config)),
                pairing_id,
                &revoked_mobile,
                &pc_descriptor,
            )?;

            let error = apply_test_out_of_band_pairing_response(
                &mut pc_config,
                stringify!(&pc_config),
                &json!({
                    "mobileSecureMesh": revoked_mobile,
                    "secureMeshClaimProof": proof,
                }),
            )
            .unwrap_err()
            .to_string();
            assert!(
                error.contains("terminal (revoked)"),
                "unexpected revoked-peer error: {error}"
            );
            assert_eq!(pc_config["mobileRelayE2ee"]["peerVerified"], false);
            assert!(
                pc_config["mobileRelayE2ee"]
                    .get("peerTrustRecord")
                    .is_none()
            );
            assert_eq!(
                pc_config["mobileRelayE2ee"]["keyTransparencyTerminalPeerBlock"]["state"],
                "revoked"
            );
            assert_eq!(
                pc_config["mobileRelayE2ee"]["keyTransparencyTerminalPeerBlock"]["redacted"],
                true
            );
            let durable = load_config_without_persistence()?;
            assert_eq!(
                durable["mobileRelayE2ee"]["keyTransparencyTerminalPeerBlock"]["state"],
                "revoked"
            );
            assert!(durable["mobileRelayE2ee"].get("peerTrustRecord").is_none());
            assert!(
                mobile_relay_pairwise_store_for_authority_reset()?
                    .load_session(&old_session_id, &local_endpoint_id)?
                    .is_none()
            );
            Ok(())
        })
    })
    .unwrap();

    set_portable_data_dir_override(previous);
}

#[test]
fn out_of_band_pairing_response_rejects_substituted_peer_without_claim_proof() {
    let dir = temp_dir("out-of-band-pairing-rejects-peer-substitution");
    let previous = set_portable_data_dir_override(Some(dir.to_path_buf()));
    let mut pc_config = default_config();
    let pc_descriptor = ensure_mobile_relay_endpoint_descriptor(
        &mut pc_config,
        &mut test_runtime_secret_material(stringify!(&mut pc_config)),
        "desktop_sidecar",
    )
    .unwrap();
    let pairing_id = "pair_peer_substitution";
    pc_config["pairingId"] = json!(pairing_id);

    let mut attacker_config = default_config();
    let attacker_descriptor = ensure_mobile_relay_endpoint_descriptor(
        &mut attacker_config,
        &mut test_runtime_secret_material(stringify!(&mut attacker_config)),
        "mobile",
    )
    .unwrap();
    let error = apply_test_out_of_band_pairing_response(
        &mut pc_config,
        stringify!(&pc_config),
        &json!({
            "mobileSecureMesh": attacker_descriptor,
            "secureMeshClaimProof": "forged-proof"
        }),
    )
    .unwrap_err()
    .to_string();
    assert!(error.contains("out-of-band claim proof is invalid"));

    assert!(
        peer_secure_mesh_descriptor(&pc_config).is_none(),
        "relay-supplied peer descriptor must not be trusted without a valid claim proof"
    );

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
    let mobile_descriptor = ensure_mobile_relay_endpoint_descriptor(
        &mut mobile_config,
        &mut test_runtime_secret_material(stringify!(&mut mobile_config)),
        "mobile",
    )
    .unwrap();
    let proof = mobile_relay_claim_proof_for_pair(
        &pc_config,
        &mut test_runtime_secret_material(stringify!(&pc_config)),
        pairing_id,
        &mobile_descriptor,
        &pc_descriptor,
    )
    .unwrap();
    apply_test_out_of_band_pairing_response(
        &mut pc_config,
        stringify!(&pc_config),
        &json!({
            "mobileSecureMesh": mobile_descriptor,
            "secureMeshClaimProof": proof
        }),
    )
    .unwrap();

    assert_eq!(
        peer_secure_mesh_descriptor(&pc_config)
            .and_then(|descriptor| descriptor.get("endpointKind").cloned())
            .and_then(|value| value.as_str().map(str::to_string)),
        Some("mobile".to_string())
    );
    assert_eq!(pc_config["mobileRelayE2ee"]["peerVerified"], true);
    set_portable_data_dir_override(previous);
}
