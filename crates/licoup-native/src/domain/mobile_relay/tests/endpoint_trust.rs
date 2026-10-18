use super::test_support::*;

#[test]
fn kt_authority_configuration_requires_bound_two_phase_foreground_confirmation() {
    let dir = temp_dir("mobile-relay-kt-two-phase-confirmation");
    let previous = set_portable_data_dir_override(Some(dir.to_path_buf()));
    let store = Arc::new(EphemeralSecretStore::new());
    let selected: Arc<dyn SecureMeshSecretStore> = store.clone();
    let signing_key = SigningKey::generate(&mut OsRng);
    let proposal = json!({
        "operation": "prepare",
        "directoryScopeCommitment": sha256_hex(b"two-phase-directory-scope"),
        "pin": {
            "logId": "two-phase-log",
            "keyId": "two-phase-key",
            "publicKeyHex": hex_encode_bytes(signing_key.verifying_key().as_bytes()),
            "provenance": "user-configured-external"
        },
        "maxSthAgeSeconds": 3600,
        "maxFutureSkewSeconds": 300
    });

    with_mobile_relay_secret_store_override(selected, || {
        let mut forbidden_one_step = proposal.clone();
        forbidden_one_step["confirmAuthorityConfiguration"] = json!(true);
        assert!(
            key_transparency_configure_authority(&forbidden_one_step)
                .unwrap_err()
                .to_string()
                .contains("cannot confirm its own challenge")
        );
        assert_eq!(store.authorization_session_count(), 0);

        let prepared = key_transparency_configure_authority(&proposal)?;
        assert_eq!(prepared["status"], "confirmation_required");
        assert_eq!(prepared["requiresUserPresence"], true);
        assert_eq!(store.authorization_session_count(), 0);
        let repeated = key_transparency_configure_authority(&proposal)?;
        assert_eq!(
            repeated["authorityChallengeId"],
            prepared["authorityChallengeId"]
        );

        let mut background_confirmation = proposal.clone();
        background_confirmation["operation"] = json!("confirm");
        background_confirmation["authorityChallengeId"] = prepared["authorityChallengeId"].clone();
        background_confirmation["confirmAuthorityConfiguration"] = json!(true);
        background_confirmation["allowInteraction"] = json!(false);
        assert!(
            key_transparency_configure_authority(&background_confirmation)
                .unwrap_err()
                .to_string()
                .contains("foreground user interaction")
        );
        assert_eq!(store.authorization_session_count(), 0);

        let mut confirmation = background_confirmation;
        confirmation["allowInteraction"] = json!(true);
        let confirmed = key_transparency_configure_authority(&confirmation)?;
        assert_eq!(confirmed["scopeCommitted"], true);
        assert_eq!(store.authorization_session_count(), 1);
        assert!(read_kt_authority_challenge()?.is_none());
        assert!(authority_configuration_matches(
            &load_config_without_persistence()?,
            &parse_kt_authority_proposal(&proposal)?,
        ));
        Ok(())
    })
    .unwrap();

    set_portable_data_dir_override(previous);
}

#[test]
fn kt_authority_challenge_rejects_stale_config_generation() {
    let dir = temp_dir("mobile-relay-kt-stale-challenge-generation");
    let previous = set_portable_data_dir_override(Some(dir.to_path_buf()));
    let store = Arc::new(EphemeralSecretStore::new());
    let selected: Arc<dyn SecureMeshSecretStore> = store.clone();
    let signing_key = SigningKey::generate(&mut OsRng);
    let proposal = json!({
        "operation": "prepare",
        "directoryScopeCommitment": sha256_hex(b"stale-challenge-directory-scope"),
        "pin": {
            "logId": "stale-challenge-log",
            "keyId": "stale-challenge-key",
            "publicKeyHex": hex_encode_bytes(signing_key.verifying_key().as_bytes()),
            "provenance": "user-configured-external"
        },
        "maxSthAgeSeconds": 3600,
        "maxFutureSkewSeconds": 300
    });
    with_mobile_relay_secret_store_override(selected, || {
        let prepared = key_transparency_configure_authority(&proposal)?;
        let mut unrelated = load_config()?;
        unrelated["pcClientName"] = json!("concurrent-config-update");
        save_config(&mut unrelated)?;
        let mut confirmation = proposal.clone();
        confirmation["operation"] = json!("confirm");
        confirmation["authorityChallengeId"] = prepared["authorityChallengeId"].clone();
        confirmation["confirmAuthorityConfiguration"] = json!(true);
        confirmation["allowInteraction"] = json!(true);
        let error = key_transparency_configure_authority(&confirmation)
            .unwrap_err()
            .to_string();
        assert!(error.contains("generation is stale"));
        assert!(
            load_config_without_persistence()?
                .get("secureMeshKeyTransparency")
                .is_none()
        );
        Ok(())
    })
    .unwrap();
    set_portable_data_dir_override(previous);
}

#[test]
fn mobile_relay_public_config_exposes_verified_trust_presentation_without_keys() {
    let dir = temp_dir("mobile-relay-public-trust-presentation");
    let previous = set_portable_data_dir_override(Some(dir.to_path_buf()));
    let mut desktop_config = default_config();
    let mut mobile_config = default_config();
    pair_mobile_relay_configs(&mut desktop_config, &mut mobile_config);

    let public = public_config(&desktop_config);
    let presentation = &public["deviceTrustPresentation"];
    assert_eq!(
        presentation["schemaVersion"],
        "licomesh.secure-mesh.device-trust-presentation.v1"
    );
    assert_eq!(presentation["verified"], true);
    assert_eq!(presentation["trustState"], "verified");
    assert_eq!(
        presentation["safetyNumberGroups"].as_array().map(Vec::len),
        Some(12)
    );
    assert!(
        presentation["safetyNumberGroups"]
            .as_array()
            .unwrap()
            .iter()
            .all(|group| group.as_str().is_some_and(
                |value| value.len() == 5 && value.bytes().all(|byte| byte.is_ascii_digit())
            ))
    );
    assert!(
        presentation["qrPayload"]
            .as_str()
            .is_some_and(|value| !value.is_empty())
    );
    let serialized = serde_json::to_string(presentation).unwrap();
    for forbidden in [
        "privateKeyBase64url",
        "signingKeyBase64url",
        "publicKeyBase64url",
        "signingPublicKeyBase64url",
    ] {
        assert!(!serialized.contains(forbidden));
    }

    set_portable_data_dir_override(previous);
}

#[test]
fn mobile_relay_pairwise_rejects_relay_asserted_prekey_trust_state() {
    let dir = temp_dir("mobile-relay-pqxdh-prekey-trust-state-required");
    let previous = set_portable_data_dir_override(Some(dir.to_path_buf()));
    let mut pc_config = default_config();
    let mut pc_descriptor = ensure_mobile_relay_endpoint_descriptor(
        &mut pc_config,
        &mut test_runtime_secret_material(stringify!(&mut pc_config)),
        "desktop_sidecar",
    )
    .unwrap();
    pc_descriptor["preKeyBundle"]["trustState"] = json!("verified");
    let error = pairwise_prekey_bundle_from_descriptor(&pc_descriptor)
        .unwrap_err()
        .to_string();
    assert!(error.contains("prekey bundle shape is invalid"));

    let mut mobile_config = default_config();
    ensure_mobile_relay_endpoint_descriptor(
        &mut mobile_config,
        &mut test_runtime_secret_material(stringify!(&mut mobile_config)),
        "mobile",
    )
    .unwrap();
    assert!(
        apply_peer_secure_mesh_descriptor(
            &mut mobile_config,
            &mut test_runtime_secret_material(stringify!(&mut mobile_config)),
            &pc_descriptor,
            true
        )
        .is_err()
    );
    assert_ne!(mobile_config["mobileRelayE2ee"]["peerVerified"], true);

    set_portable_data_dir_override(previous);
}

#[test]
fn mobile_relay_pairwise_rejects_intro_directory_authorization_mismatch() {
    let dir = temp_dir("mobile-relay-pqxdh-intro-tree-head-mismatch");
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
    mobile_descriptor["pairwiseIntro"]["directoryAuthorizationDigest"] = json!("ab".repeat(32));

    let error = apply_peer_secure_mesh_descriptor(
        &mut pc_config,
        &mut test_runtime_secret_material(stringify!(&mut pc_config)),
        &mobile_descriptor,
        true,
    )
    .unwrap_err()
    .to_string();
    assert!(error.contains("directory authorization"));
    assert!(
        pc_config["mobileRelayE2ee"]
            .get("pairwiseAccepted")
            .is_none()
    );

    set_portable_data_dir_override(previous);
}

#[test]
fn mobile_relay_pairwise_rejects_tampered_prekey_signature_via_directory_commitment() {
    let dir = temp_dir("mobile-relay-pqxdh-tampered-prekey");
    let previous = set_portable_data_dir_override(Some(dir.to_path_buf()));
    let mut pc_config = default_config();
    let mut pc_descriptor = ensure_mobile_relay_endpoint_descriptor(
        &mut pc_config,
        &mut test_runtime_secret_material(stringify!(&mut pc_config)),
        "desktop_sidecar",
    )
    .unwrap();
    pc_descriptor["preKeyBundle"]["signedPrekey"]["signatureBase64url"] =
        json!(random_base64url(64));

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
    assert!(error.contains("signed prekey commitment mismatch"));
    assert!(peer_secure_mesh_descriptor(&mobile_config).is_none());

    set_portable_data_dir_override(previous);
}

#[test]
fn mobile_relay_secure_command_requires_signed_peer_trust_record() {
    let dir = temp_dir("mobile-relay-signed-trust-record-required");
    let previous = set_portable_data_dir_override(Some(dir.to_path_buf()));
    let (mut pc_config, _mobile_config, envelope) = paired_command_envelope_fixture();
    pc_config["mobileRelayE2ee"]["peerVerified"] = json!(true);
    pc_config["mobileRelayE2ee"]
        .as_object_mut()
        .unwrap()
        .remove("peerTrustRecord");
    save_config(&mut pc_config).unwrap();

    let error = execute_secure_envelope_command(
        &json!({
            "type": SECURE_MESH_ENVELOPE_COMMAND,
            "envelope": envelope
        }),
        &json!({}),
    )
    .unwrap_err()
    .to_string();
    assert!(error.contains("peer trust record is missing"));

    set_portable_data_dir_override(previous);
}

#[test]
fn mobile_relay_secure_command_rejects_tampered_peer_trust_record() {
    let dir = temp_dir("mobile-relay-signed-trust-record-tamper");
    let previous = set_portable_data_dir_override(Some(dir.to_path_buf()));
    let (mut pc_config, _mobile_config, envelope) = paired_command_envelope_fixture();
    pc_config["mobileRelayE2ee"]["peerVerified"] = json!(true);
    pc_config["mobileRelayE2ee"]["peerTrustRecord"]["verificationMethod"] =
        json!("server_injected_trust");
    save_config(&mut pc_config).unwrap();

    let error = execute_secure_envelope_command(
        &json!({
            "type": SECURE_MESH_ENVELOPE_COMMAND,
            "envelope": envelope
        }),
        &json!({}),
    )
    .unwrap_err()
    .to_string();
    assert!(error.contains("peer trust record is invalid"));

    set_portable_data_dir_override(previous);
}

#[test]
fn mobile_relay_protected_send_blocks_unverified_key_changed_and_revoked_peers() {
    let dir = temp_dir("mobile-relay-protected-send-trust-blocks");
    let previous = set_portable_data_dir_override(Some(dir.to_path_buf()));
    let mut pc_config = default_config();
    let mut mobile_config = default_config();
    pair_mobile_relay_configs(&mut pc_config, &mut mobile_config);

    let payload_kinds = [
        crate::core::secure_mesh_crypto::SecureMeshPayloadKind::Command,
        crate::core::secure_mesh_crypto::SecureMeshPayloadKind::ResultPayload,
        crate::core::secure_mesh_crypto::SecureMeshPayloadKind::FileManifest,
        crate::core::secure_mesh_crypto::SecureMeshPayloadKind::ServiceAction,
    ];

    pc_config["mobileRelayE2ee"]["peerVerified"] = json!(false);
    for kind in payload_kinds {
        let error = seal_mobile_relay_payload(
            &pc_config,
            &mut test_runtime_secret_material(stringify!(&pc_config)),
            kind,
            &json!({"body": "blocked"}),
        )
        .unwrap_err()
        .to_string();
        assert!(
            error.contains("peer is not verified"),
            "unverified seal should fail closed for {kind:?}: {error}"
        );
    }

    pc_config["mobileRelayE2ee"]["peerVerified"] = json!(true);
    pc_config["mobileRelayE2ee"]["peerTrustRecord"]["trustState"] = json!("key_changed");
    for kind in payload_kinds {
        let error = seal_mobile_relay_payload(
            &pc_config,
            &mut test_runtime_secret_material(stringify!(&pc_config)),
            kind,
            &json!({"body": "blocked"}),
        )
        .unwrap_err()
        .to_string();
        assert!(
            error.contains("peer trust record is invalid")
                || error.contains("identity_key_changed")
                || error.contains("not trusted for sensitive use"),
            "key-changed seal should fail closed for {kind:?}: {error}"
        );
    }

    pc_config["mobileRelayE2ee"]["peerTrustRecord"]["trustState"] = json!("revoked");
    for kind in payload_kinds {
        let error = seal_mobile_relay_payload(
            &pc_config,
            &mut test_runtime_secret_material(stringify!(&pc_config)),
            kind,
            &json!({"body": "blocked"}),
        )
        .unwrap_err()
        .to_string();
        assert!(
            error.contains("peer trust record is invalid")
                || error.contains("device_revoked")
                || error.contains("not trusted for sensitive use"),
            "revoked seal should fail closed for {kind:?}: {error}"
        );
    }

    set_portable_data_dir_override(previous);
}

#[test]
fn kt_authority_reset_guard_survives_restart_and_blocks_all_old_session_paths() {
    let dir = temp_dir("mobile-relay-kt-reset-guard-crash");
    let previous = set_portable_data_dir_override(Some(dir.to_path_buf()));
    let store = Arc::new(EphemeralSecretStore::new());
    let mobile_store: Arc<dyn SecureMeshSecretStore> = store.clone();
    let pairwise_store: Arc<dyn SecureMeshSecretStore> = store.clone();

    with_mobile_relay_secret_store_override(mobile_store, || {
        with_pairwise_secret_store_override(pairwise_store, || {
            let mut pc_config = default_config();
            let mut mobile_config = default_config();
            pair_mobile_relay_configs(&mut pc_config, &mut mobile_config);
            let old_envelope = seal_mobile_relay_payload(
                &mobile_config,
                &mut test_runtime_secret_material(stringify!(&mobile_config)),
                crate::core::secure_mesh_crypto::SecureMeshPayloadKind::ServiceAction,
                &json!({"action": "old-session-before-authority-reset"}),
            )?;
            save_test_config_with_runtime_secret_context(
                &mut mobile_config,
                stringify!(&mobile_config),
            )?;

            let replacement_signing_key = SigningKey::generate(&mut OsRng);
            let replacement = json!({
                "operation": "prepare",
                "confirmSecurityReset": "RESET_KEY_TRANSPARENCY_AUTHORITY",
                "directoryScopeCommitment": sha256_hex(b"replacement-directory-scope"),
                "pin": {
                    "logId": "replacement-user-configured-log",
                    "keyId": "replacement-user-configured-key",
                    "publicKeyHex": hex_encode_bytes(
                        replacement_signing_key.verifying_key().as_bytes()
                    ),
                    "provenance": "user-configured-external"
                },
                "maxSthAgeSeconds": 3600,
                "maxFutureSkewSeconds": 300
            });
            let prepared = key_transparency_configure_authority(&replacement)?;
            assert_eq!(prepared["status"], "confirmation_required");
            let mut confirmation = replacement.clone();
            confirmation["operation"] = json!("confirm");
            confirmation["authorityChallengeId"] = prepared["authorityChallengeId"].clone();
            confirmation["confirmAuthorityConfiguration"] = json!(true);
            confirmation["allowInteraction"] = json!(true);
            let failpoint = set_kt_authority_reset_failpoint("after_guard_persisted");
            let failure = key_transparency_configure_authority(&confirmation)
                .expect_err("crash failpoint must interrupt authority replacement");
            assert!(failure.to_string().contains("reset failpoint"));
            drop(failpoint);

            // A new process observes the persisted guard before hydrating any secret or
            // opening any Pairwise/MLS state.
            assert!(kt_authority_reset_in_progress()?);
            let _restarted_public_config = load_config_without_persistence()?;
            assert!(kt_authority_reset_in_progress()?);
            let seal_error = seal_mobile_relay_payload(
                &mobile_config,
                &mut test_runtime_secret_material(stringify!(&mobile_config)),
                crate::core::secure_mesh_crypto::SecureMeshPayloadKind::ServiceAction,
                &json!({"action": "must-not-seal-after-crash"}),
            )
            .unwrap_err()
            .to_string();
            assert!(seal_error.contains("security operations remain blocked"));
            let open_error = open_mobile_relay_payload(
                &pc_config,
                &mut test_runtime_secret_material(stringify!(&pc_config)),
                &old_envelope,
                crate::core::secure_mesh_crypto::SecureMeshPayloadKind::ServiceAction,
            )
            .unwrap_err()
            .to_string();
            assert!(open_error.contains("security operations remain blocked"));
            let mls_error = crate::domain::secure_mesh_mls::dispatch(
                "secure_mesh.mls.payload.seal",
                &json!({}),
            )
            .unwrap_err()
            .to_string();
            assert!(mls_error.contains("security operations remain blocked"));
            let lifecycle_error = crate::ffi::secure_mesh_mobile_ffi::dispatch_json(
                &json!({
                    "action": "secure_mesh.lifecycle.serviceAction",
                    "params": {}
                }),
                "unsupported",
            )
            .unwrap_err()
            .to_string();
            assert_eq!(lifecycle_error, "native_operation_failed");
            let kt_route_error = crate::ffi::secure_mesh_mobile_ffi::dispatch_json(
                &json!({
                    "action": "secure_mesh.kt.publicationRequest",
                    "params": {}
                }),
                "unsupported",
            )
            .unwrap_err()
            .to_string();
            assert_eq!(kt_route_error, "native_operation_failed");
            let kt_status = crate::ffi::secure_mesh_mobile_ffi::dispatch_json(
                &json!({
                    "action": "secure_mesh.kt.status",
                    "params": {}
                }),
                "unsupported",
            )
            .map_err(|error| anyhow!("guarded KT status failed: {error}"))?;
            assert_eq!(kt_status["resetInProgress"], true);
            assert_eq!(kt_status["guardValid"], true);

            let resumed = key_transparency_configure_authority(&confirmation)
                .map_err(|error| anyhow!("authority reset resume failed: {error}"))?;
            assert_eq!(resumed["authorityChanged"], true);
            assert!(!kt_authority_reset_in_progress()?);
            let stale_session_error = seal_mobile_relay_payload(
                &mobile_config,
                &mut test_runtime_secret_material(stringify!(&mobile_config)),
                crate::core::secure_mesh_crypto::SecureMeshPayloadKind::ServiceAction,
                &json!({"action": "must-repair-after-reset"}),
            )
            .unwrap_err()
            .to_string();
            assert!(
                stale_session_error.contains("missing")
                    || stale_session_error.contains("re-pairing is required")
            );
            Ok(())
        })
    })
    .unwrap();

    set_portable_data_dir_override(previous);
}

#[test]
fn kt_authority_confirmation_recovers_idempotently_after_config_commit_crash() {
    let dir = temp_dir("mobile-relay-kt-confirmation-post-commit-crash");
    let previous = set_portable_data_dir_override(Some(dir.to_path_buf()));
    let store = Arc::new(EphemeralSecretStore::new());
    let mobile_store: Arc<dyn SecureMeshSecretStore> = store.clone();
    let pairwise_store: Arc<dyn SecureMeshSecretStore> = store.clone();

    with_mobile_relay_secret_store_override(mobile_store, || {
        with_pairwise_secret_store_override(pairwise_store, || {
            let mut config = default_config();
            ensure_mobile_relay_endpoint_descriptor(
                &mut config,
                &mut test_runtime_secret_material(stringify!(&mut config)),
                "desktop_sidecar",
            )?;
            persist_config_secret_material_to_secret_store(
                &mut config,
                store.as_ref(),
                MOBILE_RELAY_PLATFORM_SECRET_STORE_NAMESPACE,
            )?;
            save_config(&mut config)?;
            let replacement_signing_key = SigningKey::generate(&mut OsRng);
            let proposal = json!({
                "operation": "prepare",
                "confirmSecurityReset": "RESET_KEY_TRANSPARENCY_AUTHORITY",
                "directoryScopeCommitment": sha256_hex(b"post-commit-replacement-scope"),
                "pin": {
                    "logId": "post-commit-replacement-log",
                    "keyId": "post-commit-replacement-key",
                    "publicKeyHex": hex_encode_bytes(
                        replacement_signing_key.verifying_key().as_bytes()
                    ),
                    "provenance": "user-configured-external"
                },
                "maxSthAgeSeconds": 3600,
                "maxFutureSkewSeconds": 300
            });
            let prepared = key_transparency_configure_authority(&proposal)?;
            assert_eq!(prepared["requiresSecurityReset"], true);
            let mut confirmation = proposal;
            confirmation["operation"] = json!("confirm");
            confirmation["authorityChallengeId"] = prepared["authorityChallengeId"].clone();
            confirmation["confirmAuthorityConfiguration"] = json!(true);
            confirmation["allowInteraction"] = json!(true);

            let failpoint = set_kt_authority_reset_failpoint("after_replacement_config_persisted");
            let failure = key_transparency_configure_authority(&confirmation)
                .unwrap_err()
                .to_string();
            assert!(failure.contains("reset failpoint"));
            drop(failpoint);
            assert!(kt_authority_reset_in_progress()?);
            assert!(read_kt_authority_challenge()?.is_some());

            let recovered = key_transparency_configure_authority(&confirmation)?;
            assert_eq!(recovered["alreadyCommitted"], true);
            assert!(!kt_authority_reset_in_progress()?);
            assert!(read_kt_authority_challenge()?.is_none());
            Ok(())
        })
    })
    .unwrap();
    set_portable_data_dir_override(previous);
}

#[test]
fn kt_gossip_action_is_pairwise_encrypted_and_advances_both_endpoint_authorities() {
    let dir = temp_dir("mobile-relay-kt-encrypted-gossip");
    let previous = set_portable_data_dir_override(Some(dir.to_path_buf()));
    let store = Arc::new(EphemeralSecretStore::new());
    let mobile_store: Arc<dyn SecureMeshSecretStore> = store.clone();
    let pairwise_store: Arc<dyn SecureMeshSecretStore> = store.clone();

    with_mobile_relay_secret_store_override(mobile_store, || {
        with_pairwise_secret_store_override(pairwise_store, || {
            let mut pc_config = default_config();
            let mut mobile_config = default_config();
            pair_mobile_relay_configs(&mut pc_config, &mut mobile_config);
            let mobile_endpoint_id = descriptor_text(
                mobile_config
                    .get("mobileRelayE2ee")
                    .ok_or_else(|| anyhow!("mobile gossip endpoint state is missing"))?,
                "endpointId",
            )?;
            let pc_endpoint_id = descriptor_text(
                pc_config
                    .get("mobileRelayE2ee")
                    .ok_or_else(|| anyhow!("desktop gossip endpoint state is missing"))?,
                "endpointId",
            )?;
            let mobile_authority =
                open_mobile_relay_directory_authority(&mobile_config, &mobile_endpoint_id)?;
            let mobile_previous_tree_size = mobile_authority
                .latest_checkpoint()?
                .ok_or_else(|| anyhow!("mobile gossip checkpoint is missing"))?
                .tree_size;
            drop(mobile_authority);
            let pc_authority = open_mobile_relay_directory_authority(&pc_config, &pc_endpoint_id)?;
            let pc_previous_tree_size = pc_authority
                .latest_checkpoint()?
                .ok_or_else(|| anyhow!("desktop gossip checkpoint is missing"))?
                .tree_size;
            drop(pc_authority);
            let now = mobile_relay_trust_record_now_epoch()?;
            let mut unrelated: SecureMeshDirectoryLeafClaim = serde_json::from_value(
                pc_config["mobileRelayE2ee"]["keyTransparencyResponse"]["claim"].clone(),
            )?;
            unrelated.endpoint.endpoint_id = format!("gossip-{}", Uuid::new_v4());
            unrelated.endpoint.identity_public_key = hex_encode_bytes(&[0x51; 32]);
            unrelated.endpoint.signing_public_key = hex_encode_bytes(&[0x52; 32]);
            unrelated.endpoint.fingerprint = sha256_hex(b"encrypted-gossip-identity");
            unrelated.endpoint.updated_at = now_iso();
            unrelated.key_material.signed_prekey_bundle_digest =
                sha256_hex(b"encrypted-gossip-signed-prekey");
            unrelated.key_material.one_time_prekey_batch_digest =
                sha256_hex(b"encrypted-gossip-one-time-prekey");
            unrelated.key_material.mls_key_package_digest =
                sha256_hex(b"encrypted-gossip-mls-key-package");
            let signed_tree_head = with_mobile_relay_test_kt_log(|log| {
                let index = log.append_hashed_directory_leaf(
                    &unrelated.stable_label(),
                    unrelated.version(),
                    unrelated.revoked(),
                    unrelated.leaf_hash()?,
                )?;
                Ok(log.inclusion_proof_at(index, now)?.signed_tree_head)
            })?;

            // External directory/witness transport remains outside this client-owned
            // algorithm test. Model each endpoint independently accepting the same
            // authenticated transition, then gossip only the already accepted current
            // checkpoint. The current-checkpoint message deliberately carries no transition
            // proof and remains bound to the exact v7 issued-at value.
            for (config, endpoint_id, previous_tree_size) in [
                (
                    &mobile_config,
                    mobile_endpoint_id.as_str(),
                    mobile_previous_tree_size,
                ),
                (&pc_config, pc_endpoint_id.as_str(), pc_previous_tree_size),
            ] {
                let transition = with_mobile_relay_test_kt_log(|log| {
                    Ok(SecureMeshKtGossipPayload::from_sth(
                        signed_tree_head.clone(),
                        (previous_tree_size < signed_tree_head.tree_size)
                            .then(|| log.consistency_proof_at(previous_tree_size, now))
                            .transpose()?,
                    ))
                })?;
                let mut authority = open_mobile_relay_directory_authority(config, endpoint_id)?;
                let accepted = authority.observe_gossip(&transition, now)?;
                assert_eq!(accepted.tree_size, signed_tree_head.tree_size);
                assert_eq!(accepted.root_hash, signed_tree_head.root_hash);
                assert_eq!(accepted.map_root_hash, signed_tree_head.map_root_hash);
                assert_eq!(
                    accepted.issued_at_epoch_seconds,
                    signed_tree_head.issued_at_epoch_seconds
                );
            }
            let gossip = SecureMeshKtGossipPayload::from_sth(signed_tree_head, None);
            assert!(gossip.consistency_proof.is_none());

            save_test_config_with_runtime_secret_context(
                &mut mobile_config,
                stringify!(&mobile_config),
            )?;
            let sealed = dispatch_key_transparency_action(
                "secure_mesh.kt.gossip",
                &json!({
                    "operation": "seal",
                    "gossip": gossip,
                    "allowInteraction": true
                }),
            )?;
            let envelope = sealed["envelope"].clone();
            let wire = serde_json::to_string(&envelope)?;
            for forbidden in [
                SECURE_MESH_KT_GOSSIP_CONTROL_TYPE,
                gossip.signed_tree_head.root_hash.as_str(),
                gossip.signed_tree_head.map_root_hash.as_str(),
                gossip.signed_tree_head.signature.as_str(),
            ] {
                assert!(!wire.contains(forbidden));
            }

            let durable_generation = load_config_without_persistence()?;
            pc_config[CONFIG_GENERATION_FIELD] =
                durable_generation[CONFIG_GENERATION_FIELD].clone();
            pc_config[AUTHORITY_GENERATION_FIELD] =
                durable_generation[AUTHORITY_GENERATION_FIELD].clone();
            save_test_config_with_runtime_secret_context(&mut pc_config, stringify!(&pc_config))?;
            let opened = dispatch_key_transparency_action(
                "secure_mesh.kt.gossip",
                &json!({
                    "operation": "open",
                    "envelope": envelope,
                    "allowInteraction": true
                }),
            )?;
            assert_eq!(opened["treeSize"], sealed["treeSize"]);
            assert_eq!(opened["bodyRedacted"], true);
            assert!(opened.get("gossip").is_none());
            Ok(())
        })
    })
    .unwrap();

    set_portable_data_dir_override(previous);
}
