use super::super::test_support::*;
#[test]
fn mobile_relay_native_secret_store_boundary_invariant_persists_and_hydrates_redacted_config() {
    // store_secret_boundary_invariant: persisted config keeps redacted markers while E2EE
    // key material moves through SecureMeshSecretStore handles.
    let store = EphemeralSecretStore::new();
    let namespace = "native-secret-store-boundary-invariant";
    let secret_values = [
        "native-private-key-canary",
        "native-signing-key-canary",
        "native-signed-prekey-canary",
        "native-one-time-prekey-canary",
        "native-mlkem1024-prekey-seed-canary",
        "native-pairing-secret-canary",
    ];
    let mut config = json!({
        "mobileRelayE2ee": {}
    });
    for ((field, _), secret) in MOBILE_RELAY_E2EE_NATIVE_SECRET_FIELDS
        .iter()
        .copied()
        .zip(secret_values.iter().copied())
    {
        config["mobileRelayE2ee"][field] = json!(secret);
    }

    assert_eq!(store.authorization_session_count(), 0);
    persist_config_secret_material_to_secret_store(&mut config, &store, namespace).unwrap();
    assert_eq!(store.authorization_session_count(), 1);
    assert_eq!(
        store.authorization_session_reasons()[0],
        "Mobile Relay E2EE secret bundle persistence"
    );
    assert_eq!(
        store.authorization_session_operation_counts()[0],
        mobile_relay_e2ee_secret_store_authorization_batch_operation_count()
    );

    let serialized = serde_json::to_string(&config).unwrap();
    let bundle_handle = native_e2ee_secret_bundle_handle_for_namespace(namespace).unwrap();
    let bundle_raw = store
        .get_secret(&bundle_handle)
        .unwrap()
        .expect("native E2EE secret bundle should be persisted");
    let bundle = decode_mobile_relay_e2ee_secret_bundle(bundle_raw).unwrap();
    for (((field, material_field), secret_field), secret) in MOBILE_RELAY_E2EE_NATIVE_SECRET_FIELDS
        .iter()
        .copied()
        .zip(MobileRelayE2eeSecretField::ALL)
        .zip(secret_values.iter().copied())
    {
        assert!(config["mobileRelayE2ee"].get(field).is_none());
        assert_eq!(config["mobileRelayE2ee"][material_field], "redacted");
        assert!(!serialized.contains(field));
        assert!(!serialized.contains(secret));
        assert_eq!(
            bundle
                .secret(secret_field)
                .and_then(|value| value.expose_utf8().ok()),
            Some(secret)
        );
        let handle = native_secret_store_handle_for_namespace(namespace, field).unwrap();
        assert!(store.get_secret(&handle).unwrap().is_none());
    }
    assert_eq!(
        config["mobileRelayE2ee"]["secretStorageStatus"],
        "memory-only-ephemeral"
    );
    assert_eq!(
        config["secretStorageStatus"]["selectedBackend"],
        "memory-only-ephemeral"
    );

    let mut overrides = RuntimeSecretOverrides::default();
    let mut material = RuntimeSecretMaterial::new();
    hydrate_runtime_secret_material_from_secret_store(
        &config,
        &mut material,
        &mut overrides,
        &store,
        namespace,
    )
    .unwrap();
    assert_eq!(store.authorization_session_count(), 2);
    assert_eq!(
        store.authorization_session_reasons()[1],
        "Mobile Relay E2EE secret bundle hydration"
    );
    assert_eq!(
        store.authorization_session_operation_counts()[1],
        mobile_relay_e2ee_secret_store_authorization_batch_operation_count()
    );

    for (field, secret) in MobileRelayE2eeSecretField::ALL
        .into_iter()
        .zip(secret_values)
    {
        assert_eq!(
            material.e2ee_secret(field).unwrap().expose_utf8().unwrap(),
            secret,
        );
    }
    assert!(has_runtime_secret_overrides(&overrides));
    assert_eq!(
        secret_storage_backend_for_overrides(&overrides),
        "memory-only-ephemeral"
    );
}

fn persist_fixture_bundle(
    store: &EphemeralSecretStore,
    namespace: &str,
    material: &mut RuntimeSecretMaterial,
) {
    let bundle = material.take_e2ee_bundle().unwrap();
    let session = store
        .begin_authorized_session(&SecretStoreAuthorizationRequest::new(
            "Fixture custody bundle persistence",
            mobile_relay_e2ee_secret_store_authorization_batch_operation_count(),
        ))
        .unwrap();
    store
        .set_secret_with_session(
            &session,
            &native_e2ee_secret_bundle_handle_for_namespace(namespace).unwrap(),
            encode_mobile_relay_e2ee_secret_bundle(bundle).unwrap(),
        )
        .unwrap();
}

#[test]
fn relay_custody_namespace_is_derived_from_the_data_home_config_path() {
    let home_a = temp_dir("mobile-relay-custody-namespace-a");
    let home_b = temp_dir("mobile-relay-custody-namespace-b");
    let previous = set_portable_data_dir_override(Some(home_a.to_path_buf()));
    let namespace_a = native_secret_store_namespace().unwrap();
    let shared_namespace_a = native_secret_store_shared_secret_classes_namespace().unwrap();
    let config_path_a = config_path().unwrap();

    set_portable_data_dir_override(Some(home_b.to_path_buf()));
    let namespace_b = native_secret_store_namespace().unwrap();
    let config_path_b = config_path().unwrap();

    assert_ne!(config_path_a, config_path_b);
    assert_ne!(namespace_a, namespace_b);
    assert_eq!(
        namespace_a,
        sha256_hex(config_path_a.to_string_lossy().as_bytes())
    );
    assert_eq!(
        namespace_b,
        sha256_hex(config_path_b.to_string_lossy().as_bytes())
    );
    assert_eq!(namespace_b, native_secret_store_namespace().unwrap());
    assert_ne!(
        shared_namespace_a,
        native_secret_store_shared_secret_classes_namespace().unwrap()
    );
    set_portable_data_dir_override(previous);
}

#[test]
fn same_home_hydration_preserves_identity_and_fingerprint() {
    let home = temp_dir("mobile-relay-custody-same-home");
    let previous = set_portable_data_dir_override(Some(home.to_path_buf()));
    let store = EphemeralSecretStore::new();
    let mut config = default_config();
    let mut material = RuntimeSecretMaterial::new();
    ensure_mobile_relay_endpoint_material(&mut config, &mut material, "desktop_sidecar").unwrap();
    let namespace = native_secret_store_namespace().unwrap();
    let identity_before = local_public_device_identity(&config).unwrap();
    let endpoint_fingerprint_before = local_endpoint_state(&config, &material)
        .unwrap()
        .fingerprint;
    assert!(
        material
            .e2ee_secret(MobileRelayE2eeSecretField::PrivateKey)
            .is_some()
    );
    persist_fixture_bundle(&store, &namespace, &mut material);

    let mut hydrated = RuntimeSecretMaterial::new();
    let mut overrides = RuntimeSecretOverrides::default();
    hydrate_runtime_secret_material_from_secret_store(
        &config,
        &mut hydrated,
        &mut overrides,
        &store,
        &namespace,
    )
    .unwrap();
    assert!(
        hydrated
            .e2ee_secret(MobileRelayE2eeSecretField::PrivateKey)
            .is_some()
    );
    assert_eq!(
        local_endpoint_state(&config, &hydrated)
            .unwrap()
            .fingerprint,
        endpoint_fingerprint_before
    );
    assert_eq!(
        local_public_device_identity(&config).unwrap(),
        identity_before
    );

    ensure_mobile_relay_endpoint_material(&mut config, &mut hydrated, "desktop_sidecar").unwrap();
    assert_eq!(
        local_public_device_identity(&config).unwrap(),
        identity_before
    );
    assert_eq!(
        local_endpoint_state(&config, &hydrated)
            .unwrap()
            .fingerprint,
        endpoint_fingerprint_before
    );
    set_portable_data_dir_override(previous);
}

#[test]
fn status_reports_each_authorized_token_without_promoting_missing_siblings() {
    let home = temp_dir("custody-token-observations");
    let previous = set_portable_data_dir_override(Some(home.to_path_buf()));
    struct RestoreHome(Option<std::path::PathBuf>);
    impl Drop for RestoreHome {
        fn drop(&mut self) {
            set_portable_data_dir_override(self.0.take());
        }
    }
    let _restore = RestoreHome(previous);
    let store = EphemeralSecretStore::new();
    let namespace = native_secret_store_namespace().unwrap();
    let config = json!({
        "pcTokenPresent": true, "mobileTokenPresent": true,
        "pairedDevices": [
            {"id":"pc-a", "pairingId":"pair-a", "credentialPresent":true},
            {"id":"pc-b", "pairingId":"pair-b", "credentialPresent":true}
        ]
    });
    let a = paired_device_token_secret_store_key(&config["pairedDevices"][0]).unwrap();
    for key in ["pcToken", &a] {
        store
            .set_secret(
                &native_secret_store_handle_for_namespace(&namespace, key).unwrap(),
                SecretBytes::try_from_string("synthetic-token-canary".to_string()).unwrap(),
            )
            .unwrap();
    }
    let initial =
        mobile_relay_e2ee_secret_store_status(&config, &RuntimeSecretOverrides::default());
    assert!(
        initial["credentialCustody"]["inventory"]["entries"]
            .as_array()
            .unwrap()
            .iter()
            .all(|entry| entry["recovery"] == "observeLocalCustody")
    );
    let mut material = RuntimeSecretMaterial::new();
    let mut observed = RuntimeSecretOverrides::default();
    hydrate_runtime_secret_material_from_secret_store(
        &config,
        &mut material,
        &mut observed,
        &store,
        &namespace,
    )
    .unwrap();
    let authorization_count = store.authorization_session_count();
    let status = mobile_relay_e2ee_secret_store_status(&config, &observed);
    let entries = status["credentialCustody"]["inventory"]["entries"]
        .as_array()
        .unwrap();
    assert_eq!(entries.len(), 4);
    for entry in entries {
        let present = entry["credentialRef"] == "relay-token:pcToken"
            || entry["credentialRef"] == format!("relay-token:{a}");
        assert_eq!(
            entry["recovery"],
            if present {
                "retainAuthorizedLocalCustody"
            } else {
                "rePairDevice"
            }
        );
    }
    assert_eq!(store.authorization_session_count(), authorization_count);
    assert!(!status.to_string().contains("synthetic-token-canary"));
}

#[test]
fn same_home_custody_loss_preserves_metadata_and_refuses_silent_rekey() {
    let home = temp_dir("custody-loss-no-rekey");
    let previous = set_portable_data_dir_override(Some(home.to_path_buf()));
    let mut config = default_config();
    let mut original = RuntimeSecretMaterial::new();
    ensure_mobile_relay_endpoint_material(&mut config, &mut original, "desktop_sidecar").unwrap();
    save_config_raw(&mut config).unwrap();
    let before = config.clone();
    let mut missing = RuntimeSecretMaterial::new();
    let error = ensure_mobile_relay_endpoint_material(&mut config, &mut missing, "desktop_sidecar")
        .unwrap_err()
        .to_string();
    assert!(error.contains("custody is unavailable"), "{error}");
    assert_eq!(config, before);
    assert!(missing.is_empty());
    set_portable_data_dir_override(previous);
}

#[test]
fn authorized_same_device_origin_is_retained_without_key_copy_or_normalization_rebinding() {
    let home_a = temp_dir("custody-origin-a");
    let home_b = temp_dir("custody-origin-b");
    let previous = set_portable_data_dir_override(Some(home_a.to_path_buf()));
    let store = EphemeralSecretStore::new();
    let mut config = default_config();
    let mut original = RuntimeSecretMaterial::new();
    ensure_mobile_relay_endpoint_material(&mut config, &mut original, "desktop_sidecar").unwrap();
    let identity = local_public_device_identity(&config).unwrap();
    let namespace_a = native_secret_store_namespace().unwrap();
    persist_fixture_bundle(&store, &namespace_a, &mut original);
    save_config_raw(&mut config).unwrap();
    let source_bytes = std::fs::read(config_path().unwrap()).unwrap();
    set_portable_data_dir_override(Some(home_b.to_path_buf()));
    let namespace_b = native_secret_store_namespace().unwrap();
    assert_ne!(namespace_a, namespace_b);
    let session = store
        .begin_authorized_session(&SecretStoreAuthorizationRequest::new(
            "Synthetic same-device custody recovery",
            mobile_relay_e2ee_secret_store_authorization_batch_operation_count() * 2,
        ))
        .unwrap();
    let mut recovered = RuntimeSecretMaterial::new();
    let mut observed = RuntimeSecretOverrides::default();
    let chosen = hydrate_runtime_secret_material_with_local_owner(
        &config,
        &mut recovered,
        &mut observed,
        &store,
        &session,
        &namespace_b,
    )
    .unwrap();
    assert_eq!(chosen.as_deref(), Some(namespace_a.as_str()));
    ensure_mobile_relay_endpoint_material(&mut config, &mut recovered, "desktop_sidecar").unwrap();
    assert_eq!(local_public_device_identity(&config).unwrap(), identity);
    assert!(
        store
            .get_secret(&native_e2ee_secret_bundle_handle_for_namespace(&namespace_b).unwrap())
            .unwrap()
            .is_none()
    );
    let target_config = config_path().unwrap();
    let mut copied: Value = serde_json::from_slice(&source_bytes).unwrap();
    copied
        .as_object_mut()
        .unwrap()
        .remove(CONFIG_GENERATION_FIELD);
    copied
        .as_object_mut()
        .unwrap()
        .remove(AUTHORITY_GENERATION_FIELD);
    licoup_foundation::platform::file_security::atomic_write_private_text_bounded(
        &target_config,
        &serde_json::to_string(&copied).unwrap(),
        CONFIG_MAX_BYTES,
    )
    .unwrap();
    let normalized = load_config().unwrap();
    assert_eq!(
        normalized["mobileRelayE2ee"][CUSTODY_NAMESPACE_FIELD],
        namespace_a
    );
    assert_eq!(local_public_device_identity(&normalized).unwrap(), identity);
    set_portable_data_dir_override(Some(home_a.to_path_buf()));
    assert_eq!(std::fs::read(config_path().unwrap()).unwrap(), source_bytes);
    set_portable_data_dir_override(previous);
}

#[test]
fn copied_origin_cannot_override_a_different_active_target_custody() {
    let home_a = temp_dir("custody-protected-source");
    let home_b = temp_dir("custody-protected-target");
    let previous = set_portable_data_dir_override(Some(home_a.to_path_buf()));
    let store = EphemeralSecretStore::new();
    let mut source = default_config();
    let mut source_material = RuntimeSecretMaterial::new();
    ensure_mobile_relay_endpoint_material(&mut source, &mut source_material, "desktop_sidecar")
        .unwrap();
    let namespace_a = native_secret_store_namespace().unwrap();
    persist_fixture_bundle(&store, &namespace_a, &mut source_material);
    save_config_raw(&mut source).unwrap();
    set_portable_data_dir_override(Some(home_b.to_path_buf()));
    let mut target = default_config();
    let mut target_material = RuntimeSecretMaterial::new();
    ensure_mobile_relay_endpoint_material(&mut target, &mut target_material, "desktop_sidecar")
        .unwrap();
    let identity = local_public_device_identity(&target).unwrap();
    let namespace_b = native_secret_store_namespace().unwrap();
    persist_fixture_bundle(&store, &namespace_b, &mut target_material);
    let before = source.clone();
    let session = store
        .begin_authorized_session(&SecretStoreAuthorizationRequest::new(
            "Synthetic active target refusal",
            mobile_relay_e2ee_secret_store_authorization_batch_operation_count() * 2,
        ))
        .unwrap();
    let mut recovered = RuntimeSecretMaterial::new();
    let mut observed = RuntimeSecretOverrides::default();
    assert!(
        hydrate_runtime_secret_material_with_local_owner(
            &source,
            &mut recovered,
            &mut observed,
            &store,
            &session,
            &namespace_b,
        )
        .is_err()
    );
    assert!(recovered.is_empty());
    assert_eq!(source, before);
    let mut intact = RuntimeSecretMaterial::new();
    hydrate_runtime_secret_material_from_secret_store(
        &target,
        &mut intact,
        &mut RuntimeSecretOverrides::default(),
        &store,
        &namespace_b,
    )
    .unwrap();
    ensure_mobile_relay_endpoint_material(&mut target, &mut intact, "desktop_sidecar").unwrap();
    assert_eq!(local_public_device_identity(&target).unwrap(), identity);
    set_portable_data_dir_override(previous);
}

#[test]
fn owning_context_reuses_one_authorization_and_original_namespace_after_home_restore() {
    let home_a = temp_dir("custody-owner-context-a");
    let home_b = temp_dir("custody-owner-context-b");
    let previous = set_portable_data_dir_override(Some(home_a.to_path_buf()));
    let store = Arc::new(EphemeralSecretStore::new());
    let io_port: Arc<dyn SecureMeshSecretStore> = store.clone();
    with_native_namespace_store_test_port(io_port, || {
        let params = json!({"allowInteraction": true});
        let (mut source, mut context) = load_config_with_runtime_secret_context(&params)?;
        ensure_mobile_relay_endpoint_material(
            &mut source,
            &mut context.material,
            "desktop_sidecar",
        )?;
        save_config_with_runtime_secret_context(&mut source, &mut context)?;
        let source_identity = local_public_device_identity(&source)?;
        let source_namespace = native_secret_store_namespace()?;
        let source_path = config_path()?;
        let source_bytes = fs::read(&source_path)?;
        set_portable_data_dir_override(Some(home_b.to_path_buf()));
        let target_namespace = native_secret_store_namespace()?;
        let target_path = config_path()?;
        licoup_foundation::platform::file_security::atomic_write_private_text_bounded(
            &target_path,
            std::str::from_utf8(&source_bytes)?,
            CONFIG_MAX_BYTES,
        )?;
        let before = store.authorization_session_count();
        let (mut restored, mut restored_context) =
            load_config_with_runtime_secret_context(&params)?;
        ensure_mobile_relay_endpoint_material(
            &mut restored,
            &mut restored_context.material,
            "desktop_sidecar",
        )?;
        save_config_with_runtime_secret_context(&mut restored, &mut restored_context)?;
        assert_eq!(store.authorization_session_count(), before + 1);
        assert_eq!(
            restored_context.secret_store_batch.verified_namespace(),
            Some(source_namespace.as_str())
        );
        assert_eq!(
            restored["mobileRelayE2ee"][CUSTODY_NAMESPACE_FIELD],
            source_namespace
        );
        assert_eq!(local_public_device_identity(&restored)?, source_identity);
        let before_public_read = store.authorization_session_count();
        let public = config_get(&json!({}))?;
        assert_eq!(store.authorization_session_count(), before_public_read);
        assert!(
            public["config"]["mobileRelayE2ee"]
                .get(CUSTODY_NAMESPACE_FIELD)
                .is_none()
        );
        let saved =
            config_set(&json!({"pcClientName": "Restored local name", "allowInteraction": true}))?;
        assert!(
            saved["config"]["mobileRelayE2ee"]
                .get(CUSTODY_NAMESPACE_FIELD)
                .is_none()
        );
        let durable = load_config_without_persistence()?;
        assert_eq!(
            durable["mobileRelayE2ee"][CUSTODY_NAMESPACE_FIELD],
            source_namespace
        );
        assert_eq!(local_public_device_identity(&durable)?, source_identity);
        assert!(
            store
                .get_secret(&native_e2ee_secret_bundle_handle_for_namespace(
                    &target_namespace
                )?)?
                .is_none()
        );
        assert_eq!(fs::read(source_path)?, source_bytes);
        Ok(())
    })
    .unwrap();
    set_portable_data_dir_override(previous);
}

#[test]
fn a_transport_bundle_does_not_carry_local_custody_authority() {
    let source = temp_dir("custody-transport-source");
    let target = temp_dir("custody-transport-target");
    let previous = set_portable_data_dir_override(Some(source.to_path_buf()));
    let mut config = default_config();
    let mut material = RuntimeSecretMaterial::new();
    ensure_mobile_relay_endpoint_material(&mut config, &mut material, "desktop_sidecar").unwrap();
    save_config_raw(&mut config).unwrap();
    let bytes =
        encode_mobile_relay_e2ee_secret_bundle(material.take_e2ee_bundle().unwrap()).unwrap();
    let mut transported = RuntimeSecretMaterial::new();
    transported.merge_e2ee_bundle(decode_mobile_relay_e2ee_secret_bundle(bytes).unwrap());
    assert!(transported.local_custody_namespace().is_none());
    set_portable_data_dir_override(Some(target.to_path_buf()));
    let before = config.clone();
    assert!(
        ensure_mobile_relay_endpoint_material(&mut config, &mut transported, "desktop_sidecar")
            .unwrap_err()
            .to_string()
            .contains("authorized local custody")
    );
    assert_eq!(config, before);
    set_portable_data_dir_override(previous);
}

#[test]
fn insufficient_authorization_does_not_promote_copied_metadata_or_material() {
    let source = temp_dir("custody-budget-source");
    let target = temp_dir("custody-budget-target");
    let previous = set_portable_data_dir_override(Some(source.to_path_buf()));
    let store = EphemeralSecretStore::new();
    let mut config = default_config();
    let mut material = RuntimeSecretMaterial::new();
    ensure_mobile_relay_endpoint_material(&mut config, &mut material, "desktop_sidecar").unwrap();
    persist_fixture_bundle(
        &store,
        &native_secret_store_namespace().unwrap(),
        &mut material,
    );
    save_config_raw(&mut config).unwrap();
    set_portable_data_dir_override(Some(target.to_path_buf()));
    let before = config.clone();
    let session = store
        .begin_authorized_session(&SecretStoreAuthorizationRequest::new(
            "Synthetic insufficient authorization",
            1,
        ))
        .unwrap();
    let mut observed = RuntimeSecretOverrides::default();
    let mut recovered = RuntimeSecretMaterial::new();
    assert!(
        hydrate_runtime_secret_material_with_local_owner(
            &config,
            &mut recovered,
            &mut observed,
            &store,
            &session,
            &native_secret_store_namespace().unwrap()
        )
        .is_err()
    );
    assert!(recovered.is_empty());
    assert!(recovered.local_custody_namespace().is_none());
    assert_eq!(config, before);
    set_portable_data_dir_override(previous);
}

#[test]
fn relocated_home_custody_miss_is_refused_without_identity_regeneration() {
    let home_a = temp_dir("mobile-relay-custody-relocation-a");
    let home_b = temp_dir("mobile-relay-custody-relocation-b");
    let previous = set_portable_data_dir_override(Some(home_a.to_path_buf()));
    let store = EphemeralSecretStore::new();
    let mut config = default_config();
    let mut material = RuntimeSecretMaterial::new();
    ensure_mobile_relay_endpoint_material(&mut config, &mut material, "desktop_sidecar").unwrap();
    let namespace_a = native_secret_store_namespace().unwrap();
    let identity_before = local_public_device_identity(&config).unwrap();
    persist_fixture_bundle(&store, &namespace_a, &mut material);
    save_config_raw(&mut config).unwrap();
    assert_eq!(
        config["mobileRelayE2ee"][CUSTODY_NAMESPACE_FIELD],
        namespace_a
    );

    set_portable_data_dir_override(Some(home_b.to_path_buf()));
    let namespace_b = native_secret_store_namespace().unwrap();
    assert_ne!(namespace_a, namespace_b);

    let mut relocated = config.clone();
    let before = relocated.clone();
    let mut relocated_material = RuntimeSecretMaterial::new();
    let mut relocated_overrides = RuntimeSecretOverrides::default();
    hydrate_runtime_secret_material_from_secret_store(
        &relocated,
        &mut relocated_material,
        &mut relocated_overrides,
        &store,
        &namespace_b,
    )
    .unwrap();
    assert!(
        relocated_material
            .e2ee_secret(MobileRelayE2eeSecretField::PrivateKey)
            .is_none()
    );

    let error = ensure_mobile_relay_endpoint_material(
        &mut relocated,
        &mut relocated_material,
        "desktop_sidecar",
    )
    .unwrap_err()
    .to_string();

    assert!(error.contains("custody is unavailable"), "{error}");
    assert_eq!(relocated, before);
    assert_eq!(
        local_public_device_identity(&relocated).unwrap(),
        identity_before
    );
    assert!(
        relocated_material
            .e2ee_secret(MobileRelayE2eeSecretField::PrivateKey)
            .is_none()
    );
    set_portable_data_dir_override(previous);
}

#[test]
fn fresh_home_without_identity_can_initialize_new_custody() {
    let home = temp_dir("mobile-relay-custody-fresh-home");
    let previous = set_portable_data_dir_override(Some(home.to_path_buf()));
    let mut config = default_config();
    save_config_raw(&mut config).unwrap();

    let mut created = RuntimeSecretMaterial::new();
    ensure_mobile_relay_endpoint_material(&mut config, &mut created, "desktop_sidecar").unwrap();
    save_config_raw(&mut config).unwrap();
    assert!(
        created
            .e2ee_secret(MobileRelayE2eeSecretField::PrivateKey)
            .is_some()
    );
    assert_eq!(
        config["mobileRelayE2ee"][CUSTODY_NAMESPACE_FIELD],
        native_secret_store_namespace().unwrap()
    );
    set_portable_data_dir_override(previous);
}
