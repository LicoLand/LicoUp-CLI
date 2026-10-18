use super::super::test_support::*;
#[test]
fn e2ee_status_accepts_memory_only_custody_but_does_not_overclaim_missing_session() {
    let dir = temp_dir("mobile-relay-e2ee-status-platform-secret-store");
    let previous = set_portable_data_dir_override(Some(dir.to_path_buf()));
    let store = Arc::new(EphemeralSecretStore::new());
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
    let runtime_secrets = MobileRelayE2eeSecretField::ALL
        .map(|field| test_runtime_e2ee_secret(stringify!(&mobile_config), field));
    let setup_store_override: Arc<dyn SecureMeshSecretStore> = store.clone();
    with_mobile_relay_secret_store_override(setup_store_override, || {
        save_test_config_with_runtime_secret_context(&mut mobile_config, stringify!(&mobile_config))
    })
    .unwrap();
    assert_eq!(store.authorization_session_count(), 1);
    assert_eq!(
        store.authorization_session_operation_counts()[0],
        mobile_relay_e2ee_secret_store_authorization_batch_operation_count()
    );

    let store_override: Arc<dyn SecureMeshSecretStore> = store.clone();
    let status = with_mobile_relay_secret_store_override(store_override, || {
        e2ee_status(&json!({
            "authorize": true
        }))
    })
    .unwrap();

    assert_eq!(status["peerVerified"], true);
    assert_eq!(status["secureSessionEstablished"], false);
    assert_eq!(status["capabilityProjection"], Value::Null);
    assert!(
        status["blockers"]
            .as_array()
            .unwrap()
            .iter()
            .any(|blocker| blocker == "pairwise_session_missing")
    );
    assert_eq!(
        status["secretStore"]["selectedBackend"],
        "memory-only-ephemeral"
    );
    assert_eq!(
        status["secretStore"]["capabilityReport"]["custody"]["strategy"],
        "memory_only_ephemeral"
    );
    assert_eq!(
        status["secretStore"]["capabilityReport"]["custody"]["restartSemantics"],
        "re_pair_rekey_after_restart"
    );
    assert_eq!(status["secretStore"]["privateKeyInSelectedCustody"], true);
    assert_eq!(
        status["secretStore"]["oneTimeMlKem1024PrekeySeedInSelectedCustody"],
        true
    );
    assert_eq!(
        status["secretStore"]["allPrivateKeysInSelectedCustody"],
        true
    );
    assert_eq!(
        status["secretStore"]["pairingSecretInSelectedCustody"],
        true
    );
    assert_eq!(
        status["secretStore"]["portableConfigPrivateKeyPresent"],
        false
    );
    assert_eq!(status["secretStore"]["unsafePersistenceDetected"], false);
    assert_eq!(
        status["secretStore"]["authorization"]["appPasswordPromptUsed"],
        false
    );
    assert_eq!(
        status["secretStore"]["custodyReason"],
        "custody_operational"
    );
    assert_eq!(store.authorization_session_count(), 2);
    assert_eq!(
        store.authorization_session_operation_counts()[1],
        mobile_relay_e2ee_secret_store_authorization_batch_operation_count().saturating_add(2)
    );
    let serialized = serde_json::to_string(&status).unwrap();
    for secret in runtime_secrets {
        assert!(!serialized.contains(&secret));
    }

    set_portable_data_dir_override(previous);
}

#[test]
fn e2ee_status_reports_only_confirmed_negotiated_durable_session() {
    let dir = temp_dir("mobile-relay-e2ee-status-confirmed-session");
    let previous = set_portable_data_dir_override(Some(dir.to_path_buf()));
    let store = Arc::new(EphemeralSecretStore::new());
    let store_override: Arc<dyn SecureMeshSecretStore> = store.clone();
    with_mobile_relay_secret_store_override(store_override, || {
        let mut pc_config = default_config();
        let mut mobile_config = default_config();
        pair_mobile_relay_configs(&mut pc_config, &mut mobile_config);
        save_test_config_with_runtime_secret_context(
            &mut mobile_config,
            stringify!(&mobile_config),
        )?;

        let status = e2ee_status(&json!({"authorize": true}))?;
        assert_eq!(status["secureSessionEstablished"], true);
        assert!(status["capabilityProjection"].is_object());
        assert!(status["capabilityProjection"]["local"].is_object());
        assert!(status["capabilityProjection"]["peer"].is_object());
        assert!(
            status["capabilityProjection"]["negotiatedProtocolCapabilities"]
                .as_array()
                .is_some_and(|values| !values.is_empty())
        );
        assert!(
            !status["blockers"]
                .as_array()
                .unwrap()
                .iter()
                .any(|blocker| blocker
                    .as_str()
                    .unwrap_or_default()
                    .starts_with("pairwise_"))
        );
        Ok(())
    })
    .unwrap();
    set_portable_data_dir_override(previous);
}

#[test]
fn public_config_get_does_not_begin_secret_store_authorization_session() {
    let dir = temp_dir("mobile-relay-public-config-no-authorization");
    let previous = set_portable_data_dir_override(Some(dir.to_path_buf()));
    let store = Arc::new(EphemeralSecretStore::new());
    let mut config = default_config();
    config["pairingId"] = json!("pair-public-no-auth");
    config["pcToken"] = json!("pc-token-public-no-auth-canary");
    config["mobileToken"] = json!("mobile-token-public-no-auth-canary");
    ensure_mobile_relay_endpoint_descriptor(
        &mut config,
        &mut test_runtime_secret_material(stringify!(&mut config)),
        "mobile",
    )
    .unwrap();
    persist_config_secret_material_to_secret_store(
        &mut config,
        store.as_ref(),
        MOBILE_RELAY_PLATFORM_SECRET_STORE_NAMESPACE,
    )
    .unwrap();
    config["lastPairingCode"] = json!("NOAUTH-CODE");
    save_config_raw(&mut config).unwrap();
    let before_read = fs::read_to_string(config_path().unwrap()).unwrap();
    let baseline_session_count = store.authorization_session_count();

    let store_override: Arc<dyn SecureMeshSecretStore> = store.clone();
    let output = with_mobile_relay_secret_store_override(store_override, || {
        config_get(&json!({
            "authorize": false,
            "hydrateSecrets": false
        }))
    })
    .unwrap();

    assert_eq!(store.authorization_session_count(), baseline_session_count);
    assert_eq!(output["config"]["pcTokenPresent"], true);
    assert_eq!(output["config"]["mobileTokenPresent"], true);
    assert_eq!(output["config"]["lastPairingCode"], "");
    let after_read = fs::read_to_string(config_path().unwrap()).unwrap();
    assert_eq!(after_read, before_read);
    let serialized = serde_json::to_string(&output).unwrap();
    assert!(!serialized.contains("pc-token-public-no-auth-canary"));
    assert!(!serialized.contains("mobile-token-public-no-auth-canary"));

    set_portable_data_dir_override(previous);
}

#[test]
fn e2ee_status_without_authorization_does_not_begin_secret_store_session() {
    let dir = temp_dir("mobile-relay-e2ee-status-no-authorization");
    let previous = set_portable_data_dir_override(Some(dir.to_path_buf()));
    let store = Arc::new(EphemeralSecretStore::new());
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
    let setup_store_override: Arc<dyn SecureMeshSecretStore> = store.clone();
    with_mobile_relay_secret_store_override(setup_store_override, || {
        save_test_config_with_runtime_secret_context(&mut mobile_config, stringify!(&mobile_config))
    })
    .unwrap();
    let baseline_session_count = store.authorization_session_count();

    let store_override: Arc<dyn SecureMeshSecretStore> = store.clone();
    let status = with_mobile_relay_secret_store_override(store_override, || {
        e2ee_status(&json!({
            "authorize": false,
            "hydrateSecrets": false
        }))
    })
    .unwrap();

    assert_eq!(store.authorization_session_count(), baseline_session_count);
    assert_eq!(status["fullStatusAuthorized"], false);
    assert_eq!(status["authorizationRequiredForFullStatus"], true);
    assert_eq!(status["secureSessionEstablished"], false);
    assert!(
        status["blockers"]
            .as_array()
            .unwrap()
            .iter()
            .any(|blocker| { blocker == "pairwise_session_verification_requires_authorization" })
    );
    assert_eq!(
        status["secretStore"]["authorizationRequiredForFullStatus"],
        true
    );
    assert_eq!(
        status["secretStore"]["authorization"]["systemAuthorizationAttemptCount"],
        0
    );
    assert_eq!(
        status["secretStore"]["capabilityReport"]["custody"]["strategy"],
        "memory_only_ephemeral"
    );
    assert!(
        !status["secretStore"]["capabilityReport"]["enabled"]
            .as_array()
            .unwrap()
            .iter()
            .any(|capability| capability == "custody.os_secure_store")
    );
    let serialized = serde_json::to_string(&status).unwrap();
    assert!(!serialized.contains(&private_key));

    set_portable_data_dir_override(previous);
}

#[test]
fn e2ee_status_requires_single_system_authorization_prompt_budget() {
    let config = json!({});
    let mut capability_facts =
        mandatory_protocol_facts(CapabilityEvidenceKind::TestFixture).unwrap();
    capability_facts.extend([
        CapabilityFact::supported(
            SecurityCapability::OsSecureStore,
            CapabilityEvidenceKind::TestFixture,
        ),
        CapabilityFact::supported(
            SecurityCapability::UnlockedDeviceRequired,
            CapabilityEvidenceKind::TestFixture,
        ),
        CapabilityFact::supported(
            SecurityCapability::OsUserPresence,
            CapabilityEvidenceKind::TestFixture,
        ),
    ]);
    let user_presence_report = capability_catalog()
        .unwrap()
        .evaluate(&capability_facts)
        .unwrap()
        .report();
    let mut overrides = RuntimeSecretOverrides {
        pc_token: false,
        mobile_token: false,
        e2ee_private_key: true,
        e2ee_pairing_secret: true,
        e2ee_signing_key: true,
        identity_custody_verified: Some(true),
        e2ee_signed_prekey_private_key: true,
        e2ee_one_time_prekey_private_key: true,
        e2ee_one_time_mlkem1024_prekey_seed: true,
        secret_storage_backend: Some("macos-keychain"),
        secret_store_authorization: Some(RuntimeSecretStoreAuthorizationProof {
            backend: "macos-keychain",
            operation_count: 7,
            consumed_operation_count: 5,
            remaining_operation_count: 2,
            authorization_batch_within_budget: true,
            allow_interaction: true,
            shared_system_context_required: true,
            shared_system_context_available: true,
            system_authorization_attempt_count: 1,
            system_authorization_completed: true,
            single_system_authorization_context_verified: true,
            app_password_prompt_used: false,
            app_credential_prompt_used: false,
            capability_report: Some(user_presence_report),
        }),
        paired_device_tokens: Vec::new(),
    };

    let ready = mobile_relay_e2ee_secret_store_status(&config, &overrides);
    assert_eq!(
        ready["authorization"]["singleSystemAuthorizationContextVerified"],
        true
    );
    assert_eq!(ready["authorization"]["withinPromptBudget"], true);
    assert_eq!(ready["authorization"]["consumedOperationCount"], 5);
    assert_eq!(ready["authorization"]["remainingOperationCount"], 2);
    assert_eq!(ready["authorization"]["withinOperationBudget"], true);
    assert_eq!(ready["authorization"]["claimConsistent"], true);
    assert_eq!(ready["authorization"]["appPasswordPromptUsed"], false);
    assert_eq!(ready["authorization"]["appCredentialPromptUsed"], false);

    let authorization = overrides.secret_store_authorization.as_mut().unwrap();
    authorization.system_authorization_attempt_count = 2;
    authorization.single_system_authorization_context_verified = false;
    let repeated = mobile_relay_e2ee_secret_store_status(&config, &overrides);
    assert_eq!(
        repeated["authorization"]["singleSystemAuthorizationContextVerified"],
        false
    );
    assert_eq!(repeated["authorization"]["withinPromptBudget"], false);
    assert_eq!(repeated["authorization"]["claimConsistent"], false);

    let authorization = overrides.secret_store_authorization.as_mut().unwrap();
    authorization.system_authorization_attempt_count = 1;
    authorization.single_system_authorization_context_verified = false;
    authorization.app_password_prompt_used = true;
    let app_prompt = mobile_relay_e2ee_secret_store_status(&config, &overrides);
    assert_eq!(app_prompt["authorization"]["withinPromptBudget"], false);
    assert_eq!(app_prompt["authorization"]["claimConsistent"], false);
    assert_eq!(app_prompt["authorization"]["appPasswordPromptUsed"], true);

    let authorization = overrides.secret_store_authorization.as_mut().unwrap();
    authorization.app_password_prompt_used = false;
    authorization.single_system_authorization_context_verified = true;
    authorization.consumed_operation_count = 8;
    authorization.remaining_operation_count = 0;
    authorization.authorization_batch_within_budget = false;
    let over_budget = mobile_relay_e2ee_secret_store_status(&config, &overrides);
    assert_eq!(over_budget["authorization"]["withinOperationBudget"], false);
    assert_eq!(over_budget["authorization"]["claimConsistent"], false);
}

#[test]
fn adaptive_secret_store_self_test_accepts_memory_only_without_persistence() {
    let report = e2ee_secret_store_self_test(&json!({})).unwrap();
    assert_eq!(report["ok"], true);
    assert_eq!(report["selfTestPassed"], true);
    assert_eq!(report["selectedBackend"], "memory-only-ephemeral");
    assert_eq!(
        report["capabilityReport"]["custody"]["strategy"],
        "memory_only_ephemeral"
    );
    assert_eq!(
        report["capabilityReport"]["custody"]["restartSemantics"],
        "re_pair_rekey_after_restart"
    );
    assert_eq!(report["ordinaryFileSecretArtifactCount"], 0);
    assert_eq!(
        report["restartProof"]["staleSessionRestorationRejected"],
        true
    );
    assert_eq!(report["restartProof"]["rePairRekeyRequired"], true);
    assert_eq!(report["sharedSecretClassRoundTripPassed"], true);
    assert_eq!(report["sharedSecretClassPersistenceReady"], false);
}
