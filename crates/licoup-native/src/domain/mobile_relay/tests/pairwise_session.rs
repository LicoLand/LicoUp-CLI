use super::test_support::*;

#[test]
fn production_pairwise_store_reuses_selected_memory_custody_and_purges_after_restart() {
    let dir = temp_dir("mobile-relay-pairwise-selected-memory-restart");
    let previous = set_portable_data_dir_override(Some(dir.to_path_buf()));
    let first_store = Arc::new(EphemeralSecretStore::new());
    let first_override: Arc<dyn SecureMeshSecretStore> = first_store.clone();
    let (session_id, local_endpoint_id) =
        with_mobile_relay_secret_store_override(first_override, || {
            let mut pc_config = default_config();
            let mut mobile_config = default_config();
            pair_mobile_relay_configs(&mut pc_config, &mut mobile_config);
            let material = test_runtime_secret_material(stringify!(&mobile_config));
            let endpoint = local_endpoint_state(&mobile_config, &material)?;
            let pairwise_store = mobile_relay_pairwise_store()?;
            assert_eq!(pairwise_store.secret_store_backend(), first_store.backend());
            let handles = pairwise_store.referenced_secret_snapshot_handles()?;
            assert!(!handles.is_empty());
            assert!(
                handles
                    .iter()
                    .all(|handle| first_store.get_secret(handle).unwrap().is_some())
            );
            Ok((endpoint.session_id, endpoint.endpoint_id))
        })
        .unwrap();
    drop(first_store);

    let restarted_store: Arc<dyn SecureMeshSecretStore> = Arc::new(EphemeralSecretStore::new());
    with_mobile_relay_secret_store_override(restarted_store, || {
        let pairwise_store = mobile_relay_pairwise_store()?;
        assert!(
            pairwise_store
                .read_record(&session_id, &local_endpoint_id)?
                .is_none()
        );
        assert!(
            pairwise_store
                .referenced_secret_snapshot_handles()?
                .is_empty()
        );
        Ok(())
    })
    .unwrap();
    set_portable_data_dir_override(previous);
}

#[test]
fn mobile_relay_pairwise_initialization_requires_pqxdh_prekey_bundle() {
    let dir = temp_dir("mobile-relay-pqxdh-prekey-required");
    let previous = set_portable_data_dir_override(Some(dir.to_path_buf()));
    let mut pc_config = default_config();
    let mut pc_descriptor = ensure_mobile_relay_endpoint_descriptor(
        &mut pc_config,
        &mut test_runtime_secret_material(stringify!(&mut pc_config)),
        "desktop_sidecar",
    )
    .unwrap();
    pc_descriptor
        .as_object_mut()
        .unwrap()
        .remove("preKeyBundle");

    let mut mobile_config = default_config();
    ensure_mobile_relay_endpoint_descriptor(
        &mut mobile_config,
        &mut test_runtime_secret_material(stringify!(&mut mobile_config)),
        "mobile",
    )
    .unwrap();
    let error = apply_peer_secure_mesh_descriptor(
        &mut mobile_config,
        &mut test_runtime_secret_material(stringify!(&mut mobile_config)),
        &pc_descriptor,
        true,
    )
    .unwrap_err()
    .to_string();
    assert!(error.contains("missing preKeyBundle"));
    assert!(peer_secure_mesh_descriptor(&mobile_config).is_none());

    set_portable_data_dir_override(previous);
}

#[test]
fn mobile_relay_pqxdh_descriptor_publishes_signed_mlkem_prekey_without_seed() {
    let dir = temp_dir("mobile-relay-pqxdh-mlkem-prekey-material");
    let previous = set_portable_data_dir_override(Some(dir.to_path_buf()));
    let mut config = default_config();
    let descriptor = ensure_mobile_relay_endpoint_descriptor(
        &mut config,
        &mut test_runtime_secret_material(stringify!(&mut config)),
        "desktop_sidecar",
    )
    .unwrap();
    let state = &config["mobileRelayE2ee"];
    let seed = test_runtime_e2ee_secret(
        stringify!(&config),
        MobileRelayE2eeSecretField::OneTimeMlKem1024PrekeySeed,
    );
    let seed_bytes = decode_fixed_base64url::<ML_KEM_1024_KEY_GENERATION_SEED_BYTES>(
        &seed,
        "test ML-KEM-1024 prekey seed",
    )
    .unwrap();
    let curve_secret = decode_key_32(
        &test_runtime_e2ee_secret(
            stringify!(&config),
            MobileRelayE2eeSecretField::OneTimePrekeyPrivateKey,
        ),
        "test curve one-time prekey",
    )
    .unwrap();
    assert_ne!(
        &seed_bytes[..MOBILE_RELAY_KEY_BYTES],
        curve_secret.as_slice()
    );

    let bundle = pairwise_prekey_bundle_from_descriptor(&descriptor).unwrap();
    assert_eq!(
        bundle.one_time_mlkem1024_prekey.public_key.len(),
        ML_KEM_1024_PUBLIC_KEY_BYTES
    );
    assert_eq!(
        bundle.one_time_mlkem1024_prekey.prekey_id,
        descriptor_text(state, "oneTimeMlKem1024PrekeyId").unwrap()
    );
    assert!(!serde_json::to_string(&descriptor).unwrap().contains(&seed));

    set_portable_data_dir_override(previous);
}

#[test]
fn mobile_relay_pqxdh_descriptor_rejects_missing_mlkem_prekey_and_unsupported_protocol() {
    let dir = temp_dir("mobile-relay-pqxdh-strict-descriptor");
    let previous = set_portable_data_dir_override(Some(dir.to_path_buf()));
    let mut config = default_config();
    let descriptor = ensure_mobile_relay_endpoint_descriptor(
        &mut config,
        &mut test_runtime_secret_material(stringify!(&mut config)),
        "desktop_sidecar",
    )
    .unwrap();

    let mut missing_mlkem = descriptor.clone();
    missing_mlkem["preKeyBundle"]
        .as_object_mut()
        .unwrap()
        .remove("oneTimeMlKem1024Prekey");
    let error = pairwise_prekey_bundle_from_descriptor(&missing_mlkem)
        .unwrap_err()
        .to_string();
    assert!(error.contains("prekey bundle shape is invalid"));

    let mut unsupported_protocol = descriptor;
    unsupported_protocol["protocolVersion"] = json!("unsupported.secure-mesh.protocol");
    let error = pairwise_prekey_bundle_from_descriptor(&unsupported_protocol)
        .unwrap_err()
        .to_string();
    assert!(error.contains("protocol is unsupported"));

    set_portable_data_dir_override(previous);
}

#[test]
fn mobile_relay_runtime_rejects_incompatible_local_protocol_without_rekey_or_reset() {
    let dir = temp_dir("mobile-relay-pqxdh-incompatible-local-protocol");
    let previous = set_portable_data_dir_override(Some(dir.to_path_buf()));
    let mut config = default_config();
    ensure_mobile_relay_endpoint_descriptor(
        &mut config,
        &mut test_runtime_secret_material(stringify!(&mut config)),
        "desktop_sidecar",
    )
    .unwrap();
    let prior_identity = descriptor_text(&config["mobileRelayE2ee"], "publicKeyBase64url").unwrap();
    config["mobileRelayE2ee"]["protocolVersion"] = json!("unsupported.secure-mesh.protocol");
    config["paired"] = json!(true);
    config["relayEnabled"] = json!(true);
    config["pcToken"] = json!("local-token-canary");
    let before = config.clone();

    let error = ensure_mobile_relay_endpoint_descriptor(
        &mut config,
        &mut test_runtime_secret_material(stringify!(&mut config)),
        "desktop_sidecar",
    )
    .unwrap_err();

    assert!(
        error
            .to_string()
            .contains("requires explicit startup migration")
    );
    assert_eq!(
        descriptor_text(&config["mobileRelayE2ee"], "publicKeyBase64url").unwrap(),
        prior_identity
    );
    assert_eq!(config, before);

    set_portable_data_dir_override(previous);
}

#[test]
fn mobile_relay_rotates_curve_and_mlkem_one_time_prekeys_together() {
    let dir = temp_dir("mobile-relay-pqxdh-prekey-rotation");
    let previous = set_portable_data_dir_override(Some(dir.to_path_buf()));
    let mut config = default_config();
    ensure_mobile_relay_endpoint_descriptor(
        &mut config,
        &mut test_runtime_secret_material(stringify!(&mut config)),
        "desktop_sidecar",
    )
    .unwrap();
    let before_version = config["mobileRelayE2ee"]["prekeyPublicationVersion"]
        .as_u64()
        .unwrap();
    let before_curve_id = descriptor_text(&config["mobileRelayE2ee"], "oneTimePrekeyId").unwrap();
    let before_mlkem_id =
        descriptor_text(&config["mobileRelayE2ee"], "oneTimeMlKem1024PrekeyId").unwrap();
    let before_mlkem_public = descriptor_text(
        &config["mobileRelayE2ee"],
        "oneTimeMlKem1024PrekeyPublicKeyBase64url",
    )
    .unwrap();

    rotate_mobile_relay_one_time_prekeys(
        &mut config,
        &mut test_runtime_secret_material(stringify!(&mut config)),
    )
    .unwrap();

    assert_eq!(
        config["mobileRelayE2ee"]["prekeyPublicationVersion"],
        before_version + 1
    );
    assert_ne!(
        descriptor_text(&config["mobileRelayE2ee"], "oneTimePrekeyId").unwrap(),
        before_curve_id
    );
    assert_ne!(
        descriptor_text(&config["mobileRelayE2ee"], "oneTimeMlKem1024PrekeyId").unwrap(),
        before_mlkem_id
    );
    assert_ne!(
        descriptor_text(
            &config["mobileRelayE2ee"],
            "oneTimeMlKem1024PrekeyPublicKeyBase64url",
        )
        .unwrap(),
        before_mlkem_public
    );
    assert!(
        config["mobileRelayE2ee"]
            .get("keyTransparencyResponse")
            .is_none()
    );

    set_portable_data_dir_override(previous);
}

#[test]
fn mobile_relay_pqxdh_intro_requires_mlkem_prekey_id_and_ciphertext() {
    let dir = temp_dir("mobile-relay-pqxdh-strict-intro");
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
    let descriptor = ensure_mobile_relay_endpoint_descriptor(
        &mut mobile_config,
        &mut test_runtime_secret_material(stringify!(&mut mobile_config)),
        "mobile",
    )
    .unwrap();

    for field in [
        "responderOneTimeMlKem1024PrekeyId",
        "mlkem1024CiphertextBase64url",
    ] {
        let mut malformed = descriptor.clone();
        malformed["pairwiseIntro"]
            .as_object_mut()
            .unwrap()
            .remove(field);
        let error = pairwise_intro_from_descriptor(&malformed)
            .unwrap_err()
            .to_string();
        assert!(error.contains("pairwise intro shape is invalid"));
    }

    set_portable_data_dir_override(previous);
}

#[test]
fn mobile_relay_pairwise_rejects_intro_signed_prekey_mismatch() {
    let dir = temp_dir("mobile-relay-pqxdh-intro-signed-prekey-mismatch");
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
    let mut mobile_descriptor = ensure_mobile_relay_endpoint_descriptor(
        &mut mobile_config,
        &mut test_runtime_secret_material(stringify!(&mut mobile_config)),
        "mobile",
    )
    .unwrap();
    mobile_descriptor["pairwiseIntro"]["responderSignedPrekeyId"] = json!("spk-attacker");

    let error = apply_peer_secure_mesh_descriptor(
        &mut pc_config,
        &mut test_runtime_secret_material(stringify!(&mut pc_config)),
        &mobile_descriptor,
        true,
    )
    .unwrap_err()
    .to_string();
    assert!(error.contains("signed prekey id"));
    assert!(
        pc_config["mobileRelayE2ee"]
            .get("pairwiseAccepted")
            .is_none()
    );

    set_portable_data_dir_override(previous);
}

#[test]
fn mobile_relay_pairwise_rejects_intro_initiator_identity_mismatch() {
    let dir = temp_dir("mobile-relay-pqxdh-intro-initiator-identity-mismatch");
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
    let mut mobile_descriptor = ensure_mobile_relay_endpoint_descriptor(
        &mut mobile_config,
        &mut test_runtime_secret_material(stringify!(&mut mobile_config)),
        "mobile",
    )
    .unwrap();
    mobile_descriptor["pairwiseIntro"]["initiatorIdentityPublicKeyBase64url"] =
        json!(random_base64url(32));

    let error = apply_peer_secure_mesh_descriptor(
        &mut pc_config,
        &mut test_runtime_secret_material(stringify!(&mut pc_config)),
        &mobile_descriptor,
        true,
    )
    .unwrap_err()
    .to_string();
    assert!(error.contains("initiator identity"));
    assert!(
        pc_config["mobileRelayE2ee"]
            .get("pairwiseAccepted")
            .is_none()
    );

    set_portable_data_dir_override(previous);
}

#[test]
fn mobile_relay_pairwise_rejects_intro_missing_one_time_prekey() {
    let dir = temp_dir("mobile-relay-pqxdh-intro-missing-curve-otpk");
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
    let mut mobile_descriptor = ensure_mobile_relay_endpoint_descriptor(
        &mut mobile_config,
        &mut test_runtime_secret_material(stringify!(&mut mobile_config)),
        "mobile",
    )
    .unwrap();
    mobile_descriptor["pairwiseIntro"]
        .as_object_mut()
        .unwrap()
        .remove("responderOneTimePrekeyId");

    let error = apply_peer_secure_mesh_descriptor(
        &mut pc_config,
        &mut test_runtime_secret_material(stringify!(&mut pc_config)),
        &mobile_descriptor,
        true,
    )
    .unwrap_err()
    .to_string();
    assert!(error.contains("pairwise intro shape is invalid"));
    assert!(
        pc_config["mobileRelayE2ee"]
            .get("pairwiseAccepted")
            .is_none()
    );

    set_portable_data_dir_override(previous);
}

#[test]
fn mobile_relay_pairwise_rejects_reused_remote_one_time_prekey() {
    let dir = temp_dir("mobile-relay-pqxdh-reused-remote-otpk");
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

    mobile_config["mobileRelayE2ee"]["sessionId"] =
        json!(format!("mrelay_session_{}", Uuid::new_v4()));
    if let Some(e2ee) = mobile_config
        .get_mut("mobileRelayE2ee")
        .and_then(Value::as_object_mut)
    {
        e2ee.remove("pendingPairwiseIntro");
        e2ee.remove("pairwiseAccepted");
    }

    let error = apply_peer_secure_mesh_descriptor(
        &mut mobile_config,
        &mut test_runtime_secret_material(stringify!(&mut mobile_config)),
        &pc_descriptor,
        true,
    )
    .unwrap_err()
    .to_string();
    assert!(error.contains("remote one-time prekey was already used"));

    set_portable_data_dir_override(previous);
}

#[test]
fn mobile_relay_pairwise_does_not_reinitialize_from_peer_descriptor_session_id() {
    let dir = temp_dir("mobile-relay-pqxdh-stale-peer-session-id");
    let previous = set_portable_data_dir_override(Some(dir.to_path_buf()));
    let mut pc_config = default_config();
    let mut pc_descriptor = ensure_mobile_relay_endpoint_descriptor(
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
    let first_session_id = session_id(&mobile_config).unwrap();
    pc_descriptor["sessionId"] = json!("mrelay_session_stale_server_descriptor");

    apply_peer_secure_mesh_descriptor(
        &mut mobile_config,
        &mut test_runtime_secret_material(stringify!(&mut mobile_config)),
        &pc_descriptor,
        true,
    )
    .unwrap();

    assert_eq!(session_id(&mobile_config).unwrap(), first_session_id);
    assert!(mobile_config["mobileRelayE2ee"]["pendingPairwiseIntro"].is_object());

    set_portable_data_dir_override(previous);
}

#[test]
fn mobile_relay_pairwise_store_missing_requires_repair() {
    let dir = temp_dir("mobile-relay-pairwise-store-missing");
    let previous = set_portable_data_dir_override(Some(dir.to_path_buf()));
    let mut pc_config = default_config();
    let mut mobile_config = default_config();
    pair_mobile_relay_configs(&mut pc_config, &mut mobile_config);

    let store_path = mobile_relay_pairwise_store_path().unwrap();
    assert!(store_path.exists());
    fs::remove_file(&store_path).unwrap();
    let error = seal_mobile_relay_payload(
        &mobile_config,
        &mut test_runtime_secret_material(stringify!(&mobile_config)),
        crate::core::secure_mesh_crypto::SecureMeshPayloadKind::Command,
        &json!({"body": "must-not-bootstrap"}),
    )
    .unwrap_err()
    .to_string();
    assert!(error.contains("re-pairing is required"));

    set_portable_data_dir_override(previous);
}

#[test]
fn pairwise_product_blocks_withheld_peer_map_proof_and_expired_receipt_after_restart() {
    let dir = temp_dir("mobile-relay-kt-continuous-freshness");
    let previous = set_portable_data_dir_override(Some(dir.to_path_buf()));
    let store = Arc::new(EphemeralSecretStore::new());
    let mobile_store: Arc<dyn SecureMeshSecretStore> = store.clone();
    let pairwise_store: Arc<dyn SecureMeshSecretStore> = store.clone();

    with_mobile_relay_secret_store_override(mobile_store, || {
        with_pairwise_secret_store_override(pairwise_store, || {
            let mut pc_config = default_config();
            let mut mobile_config = default_config();
            pair_mobile_relay_configs(&mut pc_config, &mut mobile_config);
            let pending_for_mobile = seal_mobile_relay_payload(
                &pc_config,
                &mut test_runtime_secret_material(stringify!(&pc_config)),
                crate::core::secure_mesh_crypto::SecureMeshPayloadKind::ServiceAction,
                &json!({"action": "pre-withholding-envelope"}),
            )?;
            let local_endpoint_id = descriptor_text(
                mobile_config
                    .get("mobileRelayE2ee")
                    .ok_or_else(|| anyhow!("mobile test endpoint state is missing"))?,
                "endpointId",
            )?;
            let mut authority =
                open_mobile_relay_directory_authority(&mobile_config, &local_endpoint_id)?;
            let previous_tree_size = authority
                .latest_checkpoint()?
                .ok_or_else(|| anyhow!("mobile test KT checkpoint is missing"))?
                .tree_size;
            let now = mobile_relay_trust_record_now_epoch()?;
            let mut unrelated: SecureMeshDirectoryLeafClaim = serde_json::from_value(
                mobile_config["mobileRelayE2ee"]["keyTransparencyResponse"]["claim"].clone(),
            )?;
            unrelated.endpoint.endpoint_id = format!("unrelated-{}", Uuid::new_v4());
            unrelated.endpoint.identity_public_key = hex_encode_bytes(&[0x41; 32]);
            unrelated.endpoint.signing_public_key = hex_encode_bytes(&[0x42; 32]);
            unrelated.endpoint.fingerprint = sha256_hex(b"unrelated-directory-identity");
            unrelated.endpoint.rotation_epoch = 1;
            unrelated.endpoint.updated_at = now_iso();
            unrelated.key_material.signed_prekey_bundle_digest =
                sha256_hex(b"unrelated-signed-prekey");
            unrelated.key_material.one_time_prekey_batch_digest =
                sha256_hex(b"unrelated-one-time-prekey");
            unrelated.key_material.pairwise_prekey_version = 1;
            unrelated.key_material.mls_key_package_digest =
                sha256_hex(b"unrelated-mls-key-package");
            unrelated.key_material.mls_key_package_version = 1;
            unrelated.directory_version = 1;
            let gossip = with_mobile_relay_test_kt_log(|log| {
                let index = log.append_hashed_directory_leaf(
                    &unrelated.stable_label(),
                    unrelated.version(),
                    unrelated.revoked(),
                    unrelated.leaf_hash()?,
                )?;
                let inclusion = log.inclusion_proof_at(index, now)?;
                Ok(
                    crate::core::secure_mesh_transparency::SecureMeshKtGossipPayload::from_sth(
                        inclusion.signed_tree_head,
                        Some(log.consistency_proof_at(previous_tree_size, now)?),
                    ),
                )
            })?;
            authority.observe_gossip(&gossip, now)?;
            drop(authority);

            let withheld_seal = seal_mobile_relay_payload(
                &mobile_config,
                &mut test_runtime_secret_material(stringify!(&mobile_config)),
                crate::core::secure_mesh_crypto::SecureMeshPayloadKind::ServiceAction,
                &json!({"action": "must-refresh-peer-map"}),
            )
            .unwrap_err()
            .to_string();
            assert!(withheld_seal.contains("current accepted checkpoint"));
            let withheld_open = open_mobile_relay_payload(
                &mobile_config,
                &mut test_runtime_secret_material(stringify!(&mobile_config)),
                &pending_for_mobile,
                crate::core::secure_mesh_crypto::SecureMeshPayloadKind::ServiceAction,
            )
            .unwrap_err()
            .to_string();
            assert!(withheld_open.contains("current accepted checkpoint"));

            let pc_descriptor = ensure_mobile_relay_endpoint_descriptor(
                &mut pc_config,
                &mut test_runtime_secret_material(stringify!(&mut pc_config)),
                "desktop_sidecar",
            )?;
            apply_peer_secure_mesh_descriptor(
                &mut mobile_config,
                &mut test_runtime_secret_material(stringify!(&mut mobile_config)),
                &pc_descriptor,
                true,
            )?;
            ensure_mobile_relay_key_transparency(&mut mobile_config)?;
            let refreshed = require_current_pairwise_directory_authority(&mobile_config, now)?;
            assert!(refreshed.tree_size > previous_tree_size);
            let opened = open_mobile_relay_payload(
                &mobile_config,
                &mut test_runtime_secret_material(stringify!(&mobile_config)),
                &pending_for_mobile,
                crate::core::secure_mesh_crypto::SecureMeshPayloadKind::ServiceAction,
            )?;
            assert_eq!(
                serde_json::from_slice::<Value>(&opened)?["action"],
                "pre-withholding-envelope"
            );

            persist_config_secret_material_to_secret_store(
                &mut mobile_config,
                store.as_ref(),
                MOBILE_RELAY_PLATFORM_SECRET_STORE_NAMESPACE,
            )?;
            save_config(&mut mobile_config)?;
            let future = refreshed.expires_at_epoch_seconds.saturating_add(1);
            let future_clock = set_kt_freshness_now_override(future);
            let (restarted_config, _) = load_config_with_runtime_secret_overrides(&json!({}))?;
            let expired = seal_mobile_relay_payload(
                &restarted_config,
                &mut test_runtime_secret_material(stringify!(&restarted_config)),
                crate::core::secure_mesh_crypto::SecureMeshPayloadKind::ServiceAction,
                &json!({"action": "must-refresh-after-expiry"}),
            )
            .unwrap_err()
            .to_string();
            assert!(!expired.is_empty());
            let status = e2ee_status(&json!({}))?;
            assert_eq!(status["keyTransparencyFresh"], false);
            assert!(status["blockers"].as_array().is_some_and(|values| {
                values
                    .iter()
                    .any(|value| value == "key_transparency_label_refresh_required")
            }));
            drop(future_clock);
            Ok(())
        })
    })
    .unwrap();

    set_portable_data_dir_override(previous);
}

#[test]
fn mobile_relay_pairwise_payload_roundtrip_reuses_single_authorization_batch_per_operation() {
    let dir = temp_dir("mobile-relay-pairwise-payload-single-auth-batch");
    let previous = set_portable_data_dir_override(Some(dir.to_path_buf()));
    let secret_store = Arc::new(EphemeralSecretStore::new());
    let store_override: Arc<dyn SecureMeshSecretStore> = secret_store.clone();

    with_pairwise_secret_store_override(store_override, || {
        let mut pc_config = default_config();
        let mut mobile_config = default_config();
        pair_mobile_relay_configs(&mut pc_config, &mut mobile_config);
        let baseline_session_count = secret_store.authorization_session_count();
        let payload = json!({
            "schema": crate::core::secure_mesh::SECURE_MESH_COMMAND_PROTOCOL_VERSION,
            "commandId": "cmd_pairwise_single_auth_batch",
            "commandKind": "agent.message.send",
            "body": {
                "limit": 1
            }
        });

        let envelope = seal_mobile_relay_payload(
            &mobile_config,
            &mut test_runtime_secret_material(stringify!(&mobile_config)),
            crate::core::secure_mesh_crypto::SecureMeshPayloadKind::Command,
            &payload,
        )?;

        assert_eq!(
            secret_store.authorization_session_count(),
            baseline_session_count + 1
        );
        assert_eq!(
            secret_store.authorization_session_reasons()[baseline_session_count],
            "Mobile Relay pairwise payload authorization batch"
        );
        assert_eq!(
            secret_store.authorization_session_operation_counts()[baseline_session_count],
            3
        );

        let opened = open_mobile_relay_payload(
            &pc_config,
            &mut test_runtime_secret_material(stringify!(&pc_config)),
            &envelope,
            crate::core::secure_mesh_crypto::SecureMeshPayloadKind::Command,
        )?;
        let opened_payload = serde_json::from_slice::<Value>(&opened).unwrap();

        assert_eq!(
            opened_payload["commandId"],
            "cmd_pairwise_single_auth_batch"
        );
        assert_eq!(
            secret_store.authorization_session_count(),
            baseline_session_count + 2
        );
        assert_eq!(
            secret_store.authorization_session_reasons()[baseline_session_count + 1],
            "Mobile Relay pairwise payload authorization batch"
        );
        assert_eq!(
            secret_store.authorization_session_operation_counts()[baseline_session_count + 1],
            3
        );
        Ok(())
    })
    .unwrap();

    set_portable_data_dir_override(previous);
}
