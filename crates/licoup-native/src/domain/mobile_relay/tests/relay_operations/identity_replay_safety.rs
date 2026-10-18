use super::super::test_support::*;
#[test]
fn legitimate_peer_identity_rotation_is_terminal_until_explicit_repair() {
    let dir = temp_dir("mobile-relay-legitimate-rotation-terminal");
    let previous = set_portable_data_dir_override(Some(dir.to_path_buf()));
    let store = Arc::new(EphemeralSecretStore::new());
    let mobile_store: Arc<dyn SecureMeshSecretStore> = store.clone();
    let pairwise_store: Arc<dyn SecureMeshSecretStore> = store.clone();

    with_mobile_relay_secret_store_override(mobile_store, || {
        with_pairwise_secret_store_override(pairwise_store, || {
            let mut pc_config = default_config();
            let mut mobile_config = default_config();
            pair_mobile_relay_configs(&mut pc_config, &mut mobile_config);
            let prior_identity = local_endpoint_state(
                &pc_config,
                &mut test_runtime_secret_material(stringify!(&pc_config)),
            )?
            .device_identity()?;
            rotate_mobile_relay_local_identity_for_repair(
                &mut pc_config,
                &mut test_runtime_secret_material(stringify!(&mut pc_config)),
            )?;
            let rotated_descriptor = ensure_mobile_relay_endpoint_descriptor(
                &mut pc_config,
                &mut test_runtime_secret_material(stringify!(&mut pc_config)),
                "desktop_sidecar",
            )?;
            let rotated_identity =
                pairwise_prekey_bundle_from_descriptor(&rotated_descriptor)?.endpoint_identity;
            assert_eq!(rotated_identity.endpoint_id, prior_identity.endpoint_id);
            assert!(rotated_identity.rotation_epoch > prior_identity.rotation_epoch);
            assert_ne!(
                rotated_identity.identity_public_key,
                prior_identity.identity_public_key
            );

            let error = apply_peer_secure_mesh_descriptor(
                &mut mobile_config,
                &mut test_runtime_secret_material(stringify!(&mut mobile_config)),
                &rotated_descriptor,
                true,
            )
            .unwrap_err()
            .to_string();
            assert!(error.contains("terminal (key_changed)"));
            assert_eq!(mobile_config["mobileRelayE2ee"]["peerVerified"], false);
            assert!(
                mobile_config["mobileRelayE2ee"]
                    .get("peerTrustRecord")
                    .is_none()
            );
            assert_eq!(
                mobile_config["mobileRelayE2ee"]["keyTransparencyTerminalPeerBlock"]["state"],
                "key_changed"
            );
            let durable = load_config_without_persistence()?;
            assert_eq!(
                durable["mobileRelayE2ee"]["keyTransparencyTerminalPeerBlock"]["state"],
                "key_changed"
            );
            Ok(())
        })
    })
    .unwrap();

    set_portable_data_dir_override(previous);
}

#[test]
fn out_of_band_mobile_response_cannot_replace_pinned_pc_identity() {
    let dir = temp_dir("out-of-band-mobile-response-pinned-pc");
    let previous = set_portable_data_dir_override(Some(dir.to_path_buf()));
    let mut pinned_pc_config = default_config();
    let pinned_pc_descriptor = ensure_mobile_relay_endpoint_descriptor(
        &mut pinned_pc_config,
        &mut test_runtime_secret_material(stringify!(&mut pinned_pc_config)),
        "desktop_sidecar",
    )
    .unwrap();
    let mut mobile_config = default_config();
    mobile_config["pairingId"] = json!("pair_pinned_pc");
    ensure_mobile_relay_endpoint_descriptor(
        &mut mobile_config,
        &mut test_runtime_secret_material(stringify!(&mut mobile_config)),
        "mobile",
    )
    .unwrap();
    apply_peer_secure_mesh_descriptor(
        &mut mobile_config,
        &mut test_runtime_secret_material(stringify!(&mut mobile_config)),
        &pinned_pc_descriptor,
        true,
    )
    .unwrap();
    let pinned_descriptor = peer_secure_mesh_descriptor(&mobile_config).unwrap();
    let pinned_fingerprint = mobile_config["mobileRelayE2ee"]["peerDeviceTrustFingerprint"].clone();
    let pinned_trust_record = mobile_config["mobileRelayE2ee"]["peerTrustRecord"].clone();

    let mut attacker_pc_config = default_config();
    let attacker_pc_descriptor = ensure_mobile_relay_endpoint_descriptor(
        &mut attacker_pc_config,
        &mut test_runtime_secret_material(stringify!(&mut attacker_pc_config)),
        "desktop_sidecar",
    )
    .unwrap();
    assert_ne!(pinned_pc_descriptor, attacker_pc_descriptor);
    let error = apply_test_out_of_band_pairing_response(
        &mut mobile_config,
        stringify!(&mobile_config),
        &json!({
            "mobileSecureMesh": attacker_pc_descriptor,
            "secureMeshClaimProof": "forged-proof"
        }),
    )
    .unwrap_err()
    .to_string();
    assert!(error.contains("out-of-band claim proof is invalid"));

    assert_eq!(
        peer_secure_mesh_descriptor(&mobile_config).unwrap(),
        pinned_descriptor
    );
    assert_eq!(
        mobile_config["mobileRelayE2ee"]["peerDeviceTrustFingerprint"],
        pinned_fingerprint
    );
    assert_eq!(
        mobile_config["mobileRelayE2ee"]["peerTrustRecord"],
        pinned_trust_record
    );
    assert_eq!(mobile_config["mobileRelayE2ee"]["peerVerified"], true);
    set_portable_data_dir_override(previous);
}

#[test]
fn tampered_mobile_relay_command_envelope_is_rejected_before_execution() {
    let dir = temp_dir("mobile-relay-tampered-command");
    let previous = set_portable_data_dir_override(Some(dir.to_path_buf()));
    let (mut pc_config, _mobile_config, envelope) = paired_command_envelope_fixture();
    persist_test_runtime_secret_material(stringify!(&pc_config)).unwrap();
    save_config(&mut pc_config).unwrap();

    let mut tampered = envelope;
    tampered["envelopeId"] = json!(general_purpose::URL_SAFE_NO_PAD.encode([0x7fu8; 24]));
    let error = execute_secure_envelope_command(
        &json!({
            "type": SECURE_MESH_ENVELOPE_COMMAND,
            "envelope": tampered
        }),
        &json!({}),
    )
    .unwrap_err()
    .to_string();
    assert!(
        error.contains("AAD hash mismatch") || error.contains("authentication failed"),
        "unexpected tamper error: {error}"
    );

    set_portable_data_dir_override(previous);
}

#[test]
fn commands_sync_redacts_malicious_station_crypto_errors() {
    let dir = temp_dir("mobile-relay-sync-redacted-crypto-error");
    let previous = set_portable_data_dir_override(Some(dir.to_path_buf()));
    let (mut pc_config, _mobile_config, mut envelope) = paired_command_envelope_fixture();
    let mut carrier = general_purpose::URL_SAFE_NO_PAD
        .decode(envelope["ciphertext"].as_str().unwrap())
        .unwrap();
    carrier[16] ^= 0x01;
    envelope["ciphertext"] = json!(general_purpose::URL_SAFE_NO_PAD.encode(carrier));
    let station = CanonicalStation::start(6, vec![envelope]);
    pc_config["pairingId"] = json!("pair_sync_redacted_crypto_error");
    pc_config["pcToken"] = json!("pc-token-sync-redacted-crypto-error");
    pc_config["relayEnabled"] = json!(true);
    pc_config["stationBaseUrl"] = json!(station.url());
    persist_test_runtime_secret_material(stringify!(&pc_config)).unwrap();
    save_config(&mut pc_config).unwrap();

    let output = commands_sync(&with_station_params(json!({"targets": []}))).unwrap();
    assert_eq!(output["completed"][0]["ok"], false);
    assert_eq!(
        output["completed"][0]["completion"]["code"],
        SECURE_MESH_ENDPOINT_CRYPTO_RUNTIME_FAILED_CODE
    );
    assert_eq!(
        output["completed"][0]["error"],
        SECURE_MESH_ENDPOINT_CRYPTO_RUNTIME_FAILED_DETAIL
    );
    let serialized = serde_json::to_string(&output).unwrap();
    assert!(!serialized.contains("authentication failed"));
    assert!(!serialized.contains("AAD hash mismatch"));
    station.assert_operations(&[
        BadTowerStationOperation::LeaseMailbox,
        BadTowerStationOperation::LeaseMailbox,
        BadTowerStationOperation::LeaseMailbox,
        BadTowerStationOperation::ReceiveEnvelopes,
        BadTowerStationOperation::LeaseMailbox,
        BadTowerStationOperation::ReceiveEnvelopes,
    ]);

    station.join();
    set_portable_data_dir_override(previous);
}

#[test]
fn mobile_relay_command_error_result_redacts_internal_detail() {
    let dir = temp_dir("mobile-relay-command-redacted-internal-error");
    let previous = set_portable_data_dir_override(Some(dir.to_path_buf()));
    let (mut pc_config, mobile_config, _envelope) = paired_command_envelope_fixture();
    persist_test_runtime_secret_material(stringify!(&pc_config)).unwrap();
    save_config(&mut pc_config).unwrap();
    let invalid_command_payload = json!({
        "schema": "unsupported-schema-local-secret-canary",
        "body": {
            "text": "malicious-relay-command-error-canary"
        }
    });
    let envelope = seal_mobile_relay_payload(
        &mobile_config,
        &mut test_runtime_secret_material(stringify!(&mobile_config)),
        crate::core::secure_mesh_crypto::SecureMeshPayloadKind::Command,
        &invalid_command_payload,
    )
    .unwrap();

    let result_envelope = execute_secure_envelope_command(
        &json!({
            "type": SECURE_MESH_ENVELOPE_COMMAND,
            "envelope": envelope
        }),
        &json!({}),
    )
    .unwrap();
    let result = opened_result_payload(&mobile_config, &result_envelope);
    assert_eq!(result["ok"], false);
    assert_eq!(
        result["code"],
        SECURE_MESH_ENDPOINT_CRYPTO_RUNTIME_FAILED_CODE
    );
    assert_eq!(
        result["error"],
        SECURE_MESH_ENDPOINT_CRYPTO_RUNTIME_FAILED_DETAIL
    );
    assert_eq!(result["bodyRedacted"], true);
    let serialized = serde_json::to_string(&result).unwrap();
    assert!(!serialized.contains("unsupported-schema-local-secret-canary"));
    assert!(!serialized.contains("malicious-relay-command-error-canary"));

    set_portable_data_dir_override(previous);
}

#[test]
fn replayed_mobile_relay_command_envelope_does_not_execute_twice() {
    let dir = temp_dir("mobile-relay-replayed-command");
    let previous = set_portable_data_dir_override(Some(dir.to_path_buf()));
    let (mut pc_config, mobile_config, envelope) = paired_command_envelope_fixture();
    persist_test_runtime_secret_material(stringify!(&pc_config)).unwrap();
    save_config(&mut pc_config).unwrap();
    let command = json!({
        "type": SECURE_MESH_ENVELOPE_COMMAND,
        "envelope": envelope
    });

    let first_result_envelope = execute_secure_envelope_command(&command, &json!({})).unwrap();
    let first_result = opened_result_payload(&mobile_config, &first_result_envelope);
    assert_eq!(first_result["evaluation"]["code"], "execute");
    assert_eq!(first_result["execution"]["outcome"], "result");

    let second_result = execute_secure_envelope_command(&command, &json!({}));
    assert!(
        second_result
            .unwrap_err()
            .to_string()
            .contains("pairwise message replay detected")
    );

    set_portable_data_dir_override(previous);
}

#[test]
fn mobile_relay_result_replay_proof_rejects_second_open_without_plaintext() {
    let dir = temp_dir("mobile-relay-result-replay-proof");
    let previous = set_portable_data_dir_override(Some(dir.to_path_buf()));
    let (mut pc_config, mobile_config, envelope) = paired_command_envelope_fixture();
    persist_test_runtime_secret_material(stringify!(&pc_config)).unwrap();
    save_config(&mut pc_config).unwrap();
    let result_envelope = execute_secure_envelope_command(
        &json!({
            "type": SECURE_MESH_ENVELOPE_COMMAND,
            "envelope": envelope
        }),
        &json!({}),
    )
    .unwrap();
    let response_summary = secure_result_response_summary(&json!({
        "ok": true,
        "command": {
            "commandId": "cmd_mobile_relay_replay_fixture",
            "status": "completed",
            "resultEnvelope": result_envelope.clone()
        },
        "ackPurge": {
            "purged": true
        }
    }));
    let proof = result_envelope_replay_proof(
        &mobile_config,
        &mut test_runtime_secret_material(stringify!(&mobile_config)),
        &result_envelope,
        response_summary,
    )
    .unwrap();
    assert_eq!(proof["ok"], true);
    assert_eq!(proof["firstOpenOk"], true);
    assert_eq!(proof["firstOpenBodyRedacted"], true);
    assert_eq!(proof["replayRejected"], true);
    assert_eq!(proof["bodyRedacted"], true);
    let serialized = serde_json::to_string(&proof).unwrap();
    assert!(!serialized.contains("cmd_mobile_relay_replay_fixture"));
    assert!(!serialized.contains("idem_mobile_relay_replay_fixture"));
    assert!(!serialized.contains("limit"));

    set_portable_data_dir_override(previous);
}
