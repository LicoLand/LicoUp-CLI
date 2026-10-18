use super::super::test_support::*;
#[test]
fn secure_mesh_envelope_command_is_transport_only() {
    let dir = temp_dir("mobile-relay-unverified-secure-command");
    let previous = set_portable_data_dir_override(Some(dir.to_path_buf()));
    let command = json!({
        "type": SECURE_MESH_ENVELOPE_COMMAND,
        "payload": {
            "envelope": secure_envelope_fixture()
        }
    });
    let visible_command = redacted_relay_command(&command);
    assert_eq!(visible_command["secureEnvelopePresent"], true);
    let error = execute_secure_envelope_command(&command, &json!({}))
        .unwrap_err()
        .to_string();
    assert!(
        error.contains("mobile relay"),
        "unexpected redacted secure command rejection: {error}"
    );
    assert!(
        !error.contains(
            command["payload"]["envelope"]["ciphertext"]
                .as_str()
                .unwrap()
        )
    );
    assert!(
        secure_envelope_param(&json!({
            "envelope": secure_envelope_fixture()
        }))
        .is_some()
    );
    set_portable_data_dir_override(previous);
}

#[test]
fn secure_envelope_validation_rejects_malicious_relay_shapes_before_decrypt() {
    // Lico Arc v1 rejects extra fields via deny_unknown_fields.
    let mut oversized = secure_envelope_fixture();
    oversized["unknownField"] = json!("should be rejected");
    assert!(
        validate_secure_envelope(&oversized)
            .unwrap_err()
            .to_string()
            .contains("JSON is invalid")
    );

    let mut invalid_base64 = secure_envelope_fixture();
    invalid_base64["ciphertext"] = json!("not base64!");
    assert!(validate_secure_envelope(&invalid_base64).is_err());

    let mut invalid_expiry = secure_envelope_fixture();
    invalid_expiry["expiresAt"] = json!("not-a-timestamp");
    assert!(
        validate_secure_envelope(&invalid_expiry)
            .unwrap_err()
            .to_string()
            .contains("expiresAt")
    );

    let mut bad_contract = secure_envelope_fixture();
    bad_contract["contractVersion"] = json!("unsupported.v1");
    assert!(
        validate_secure_envelope(&bad_contract)
            .unwrap_err()
            .to_string()
            .contains("contract version")
    );
}

#[test]
fn mobile_relay_e2ee_round_trips_command_and_result_without_plaintext() {
    let dir = temp_dir("mobile-relay-e2ee-roundtrip");
    let previous = set_portable_data_dir_override(Some(dir.to_path_buf()));
    let mut pc_config = default_config();
    let mut mobile_config = default_config();
    pair_mobile_relay_configs(&mut pc_config, &mut mobile_config);

    assert_eq!(
        session_id(&pc_config).unwrap(),
        session_id(&mobile_config).unwrap()
    );

    let command_body = json!({
        "schema": crate::core::secure_mesh::SECURE_MESH_COMMAND_PROTOCOL_VERSION,
        "commandId": "cmd_mobile_test",
        "commandKind": "agent.message.send",
        "senderIdentity": {
            "endpointId": local_endpoint_state(&mobile_config, &mut test_runtime_secret_material(stringify!(&mobile_config))).unwrap().endpoint_id,
            "identityFingerprint": local_endpoint_state(&mobile_config, &mut test_runtime_secret_material(stringify!(&mobile_config))).unwrap().fingerprint,
            "trustState": "verified",
            "endpointKind": "mobile"
        },
        "targetBinding": {
            "targetEndpointId": local_endpoint_state(&pc_config, &mut test_runtime_secret_material(stringify!(&pc_config))).unwrap().endpoint_id,
            "targetAgentId": "codex",
            "workspaceId": "default"
        },
        "riskClass": "safe_write",
        "requiresUserConfirmation": false,
        "idempotencyKey": "idem_mobile_test",
        "createdAt": now_iso(),
        "expiresAt": timestamp_after_seconds(MOBILE_RELAY_COMMAND_TTL_SECONDS).unwrap(),
        "body": {
            "text": "plaintext-canary-mobile-relay"
        }
    });
    let command_envelope = seal_mobile_relay_payload(
        &mobile_config,
        &mut test_runtime_secret_material(stringify!(&mobile_config)),
        crate::core::secure_mesh_crypto::SecureMeshPayloadKind::Command,
        &command_body,
    )
    .unwrap();
    assert_eq!(
        command_envelope["contractVersion"],
        licoup_protocol_bindings::licoarc_relay::LICOARC_RELAY_CONTRACT_VERSION
    );
    let command_wire = serde_json::to_string(&command_envelope).unwrap();
    assert!(!command_wire.contains("plaintext-canary-mobile-relay"));
    assert!(!command_wire.contains("agent.message.send"));

    let opened_command = open_mobile_relay_payload(
        &pc_config,
        &mut test_runtime_secret_material(stringify!(&pc_config)),
        &command_envelope,
        crate::core::secure_mesh_crypto::SecureMeshPayloadKind::Command,
    )
    .unwrap();
    let opened_command_json: Value = serde_json::from_slice(&opened_command).unwrap();
    assert_eq!(
        opened_command_json["body"]["text"],
        "plaintext-canary-mobile-relay"
    );

    let result_body = json!({
        "ok": true,
        "result": "plaintext-result-canary"
    });
    let result_envelope = seal_mobile_relay_payload(
        &pc_config,
        &mut test_runtime_secret_material(stringify!(&pc_config)),
        crate::core::secure_mesh_crypto::SecureMeshPayloadKind::ResultPayload,
        &result_body,
    )
    .unwrap();
    assert_eq!(
        result_envelope["contractVersion"],
        licoup_protocol_bindings::licoarc_relay::LICOARC_RELAY_CONTRACT_VERSION
    );
    let result_wire = serde_json::to_string(&result_envelope).unwrap();
    assert!(!result_wire.contains("plaintext-result-canary"));

    let opened_result = open_mobile_relay_payload(
        &mobile_config,
        &mut test_runtime_secret_material(stringify!(&mobile_config)),
        &result_envelope,
        crate::core::secure_mesh_crypto::SecureMeshPayloadKind::ResultPayload,
    )
    .unwrap();
    let opened_result_json: Value = serde_json::from_slice(&opened_result).unwrap();
    assert_eq!(opened_result_json["result"], "plaintext-result-canary");

    set_portable_data_dir_override(previous);
}

#[test]
fn mobile_relay_file_key_envelope_hides_attachment_key_and_opens_file_after_decrypt() {
    let dir = temp_dir("mobile-relay-file-key-envelope");
    let previous = set_portable_data_dir_override(Some(dir.to_path_buf()));
    let mut pc_config = default_config();
    let mut mobile_config = default_config();
    pair_mobile_relay_configs(&mut pc_config, &mut mobile_config);

    let file_key_bytes = [93u8; 32];
    let file_key = crate::core::secure_mesh_file::FileRootKey::from_bytes(file_key_bytes);
    let file_key_base64url = general_purpose::URL_SAFE_NO_PAD.encode(file_key_bytes);
    let manifest = crate::core::secure_mesh_file::SecureMeshFileManifest {
        file_id: "relay-file-key-canary-id".to_string(),
        file_name: "relay-private-file-key-canary.txt".to_string(),
        mime_type: "text/plain".to_string(),
        relative_path: "relay/private-file-key-canary".to_string(),
        total_size: 33,
        chunk_size: 33,
        chunk_count: 1,
    };
    let mobile_material = test_runtime_secret_material(stringify!(&mobile_config));
    let pc_material = test_runtime_secret_material(stringify!(&pc_config));
    let source_endpoint = local_endpoint_state(&mobile_config, &mobile_material).unwrap();
    let target_endpoint = local_endpoint_state(&pc_config, &pc_material).unwrap();
    let file_hash = format!(
        "sha256:{}",
        general_purpose::URL_SAFE_NO_PAD
            .encode(Sha256::digest(b"relay file body plaintext canary"))
    );
    let manifest_context =
        crate::core::secure_mesh_file::SecureMeshFileProtectionContext::for_pairwise_device(
            crate::core::secure_mesh_crypto::SecureMeshContentContext::new(
                "env_relay_file_manifest_key_wrap",
                "msg_relay_file_manifest_key_wrap",
                "mailbox_relay_file_key_wrap",
                &source_endpoint.endpoint_id,
                &target_endpoint.endpoint_id,
                session_id(&mobile_config).unwrap(),
                "2026-01-01T00:00:00.000Z",
                "2026-01-01T00:10:00.000Z",
            ),
            manifest.file_id.clone(),
            manifest.chunk_count,
            file_hash.clone(),
            1_800_000_000,
        )
        .unwrap();
    let encrypted_manifest =
        crate::core::secure_mesh_file::seal_file_manifest(&file_key, &manifest_context, &manifest)
            .unwrap();
    let chunk = crate::core::secure_mesh_file::SecureMeshFileChunk {
        file_id: manifest.file_id.clone(),
        chunk_index: 0,
        bytes: b"relay file body plaintext canary".to_vec(),
    };
    let chunk_context =
        crate::core::secure_mesh_file::SecureMeshFileProtectionContext::for_pairwise_device(
            crate::core::secure_mesh_crypto::SecureMeshContentContext::new(
                "env_relay_file_chunk_key_wrap",
                "msg_relay_file_chunk_key_wrap",
                "mailbox_relay_file_key_wrap",
                &source_endpoint.endpoint_id,
                &target_endpoint.endpoint_id,
                session_id(&mobile_config).unwrap(),
                "2026-01-01T00:00:00.000Z",
                "2026-01-01T00:10:00.000Z",
            ),
            manifest.file_id.clone(),
            manifest.chunk_count,
            file_hash,
            1_800_000_000,
        )
        .unwrap();
    let encrypted_chunk =
        crate::core::secure_mesh_file::seal_file_chunk(&file_key, &chunk_context, &chunk).unwrap();
    drop(source_endpoint);
    drop(target_endpoint);
    drop(mobile_material);
    drop(pc_material);

    let file_key_payload = json!({
        "kind": "secure_mesh.file_key",
        "fileKeyBase64url": file_key_base64url,
        "fileId": manifest.file_id,
        "fileKeyCanary": "relay-file-key-secret-canary",
        "manifestCiphertextHash": encrypted_manifest.ciphertext_hash,
        "chunkCiphertextHash": encrypted_chunk.ciphertext_hash
    });
    let file_key_envelope = seal_mobile_relay_payload(
        &mobile_config,
        &mut test_runtime_secret_material(stringify!(&mobile_config)),
        crate::core::secure_mesh_crypto::SecureMeshPayloadKind::Command,
        &file_key_payload,
    )
    .unwrap();
    let server_wire = serde_json::to_string(&file_key_envelope).unwrap();
    for forbidden in [
        "relay-file-key-canary-id",
        "relay-private-file-key-canary.txt",
        "relay/private-file-key-canary",
        "relay-file-key-secret-canary",
        "relay file body plaintext canary",
        file_key_base64url.as_str(),
    ] {
        assert!(
            !server_wire.contains(forbidden),
            "mobile relay file-key envelope leaked {forbidden}"
        );
    }

    let wrong_kind_error = open_mobile_relay_payload(
        &pc_config,
        &mut test_runtime_secret_material(stringify!(&pc_config)),
        &file_key_envelope,
        crate::core::secure_mesh_crypto::SecureMeshPayloadKind::ResultPayload,
    )
    .unwrap_err()
    .to_string();
    assert!(wrong_kind_error.contains("AAD hash mismatch"));

    let opened = open_mobile_relay_payload(
        &pc_config,
        &mut test_runtime_secret_material(stringify!(&pc_config)),
        &file_key_envelope,
        crate::core::secure_mesh_crypto::SecureMeshPayloadKind::Command,
    )
    .unwrap();
    let opened_json: Value = serde_json::from_slice(&opened).unwrap();
    assert_eq!(opened_json["fileKeyCanary"], "relay-file-key-secret-canary");
    let recovered_key = crate::core::secure_mesh_file::FileRootKey::from_bytes(
        general_purpose::URL_SAFE_NO_PAD
            .decode(opened_json["fileKeyBase64url"].as_str().unwrap())
            .unwrap()
            .try_into()
            .unwrap(),
    );
    let opened_manifest = crate::core::secure_mesh_file::open_file_manifest(
        &recovered_key,
        &manifest_context,
        &encrypted_manifest,
    )
    .unwrap();
    assert_eq!(opened_manifest.file_id, "relay-file-key-canary-id");
    let opened_chunk = crate::core::secure_mesh_file::open_file_chunk(
        &recovered_key,
        &chunk_context,
        &encrypted_chunk,
    )
    .unwrap();
    assert_eq!(opened_chunk.bytes, b"relay file body plaintext canary");

    let replay_error = open_mobile_relay_payload(
        &pc_config,
        &mut test_runtime_secret_material(stringify!(&pc_config)),
        &file_key_envelope,
        crate::core::secure_mesh_crypto::SecureMeshPayloadKind::Command,
    )
    .unwrap_err()
    .to_string();
    assert!(replay_error.contains("replay detected"));

    set_portable_data_dir_override(previous);
}

#[test]
fn mobile_relay_file_key_envelope_metadata_boundary_is_exhaustive() {
    let dir = temp_dir("mobile-relay-file-key-envelope-boundary");
    let previous = set_portable_data_dir_override(Some(dir.to_path_buf()));
    let mut pc_config = default_config();
    let mut mobile_config = default_config();
    pair_mobile_relay_configs(&mut pc_config, &mut mobile_config);

    let file_key_base64url = general_purpose::URL_SAFE_NO_PAD.encode([77u8; 32]);
    let envelope = seal_mobile_relay_payload(
        &mobile_config,
        &mut test_runtime_secret_material(stringify!(&mobile_config)),
        crate::core::secure_mesh_crypto::SecureMeshPayloadKind::Command,
        &json!({
            "kind": "secure_mesh.file_key",
            "fileKeyBase64url": file_key_base64url,
            "fileId": "relay-file-boundary-private-id",
            "fileName": "relay-file-boundary-private-name.txt",
            "fileKeyCanary": "relay-file-boundary-private-key-canary"
        }),
    )
    .unwrap();
    let object = envelope.as_object().unwrap();
    let mut visible_keys = object.keys().map(String::as_str).collect::<Vec<_>>();
    visible_keys.sort_unstable();
    let mut expected_keys = vec![
        "ciphertext",
        "contractVersion",
        "envelopeId",
        "expiresAt",
        "mailboxId",
    ];
    expected_keys.sort_unstable();
    assert_eq!(visible_keys, expected_keys);
    let server_wire = serde_json::to_string(&envelope).unwrap();
    for forbidden in [
        "\"kind\"",
        "\"fileKeyBase64url\"",
        "\"fileId\"",
        "\"fileName\"",
        "\"fileKeyCanary\"",
        "secure_mesh.file_key",
        "relay-file-boundary-private-id",
        "relay-file-boundary-private-name.txt",
        "relay-file-boundary-private-key-canary",
        file_key_base64url.as_str(),
    ] {
        assert!(
            !server_wire.contains(forbidden),
            "mobile relay file-key metadata boundary leaked {forbidden}"
        );
    }

    set_portable_data_dir_override(previous);
}

#[test]
fn mailbox_rotation_boundary_accepts_current_and_previous_epoch_only() {
    let dir = temp_dir("mobile-relay-mailbox-rotation-overlap");
    let previous = set_portable_data_dir_override(Some(dir.to_path_buf()));
    let mut pc_config = default_config();
    let mut mobile_config = default_config();
    pair_mobile_relay_configs(&mut pc_config, &mut mobile_config);
    let pc_material = test_runtime_secret_material(stringify!(&pc_config));
    let pc = local_endpoint_state(&pc_config, &pc_material).unwrap();

    let accepted = local_canonical_mailbox_tokens_at_epoch(&pc_config, &pc_material, 420).unwrap();
    let current =
        canonical_mailbox_token(&pc_material, &pc.endpoint_id, &pc.endpoint_kind, 420).unwrap();
    let previous_epoch =
        canonical_mailbox_token(&pc_material, &pc.endpoint_id, &pc.endpoint_kind, 419).unwrap();
    let expired =
        canonical_mailbox_token(&pc_material, &pc.endpoint_id, &pc.endpoint_kind, 418).unwrap();

    assert_eq!(accepted, vec![current, previous_epoch]);
    assert!(!accepted.contains(&expired));
    assert_eq!(
        local_canonical_mailbox_tokens_at_epoch(&pc_config, &pc_material, 0)
            .unwrap()
            .len(),
        1
    );
    set_portable_data_dir_override(previous);
}
