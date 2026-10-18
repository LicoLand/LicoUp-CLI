use super::super::test_support::*;
use crate::core::secure_mesh_secret_store::SecretBytes;
#[test]
fn mobile_relay_user_level_config_mutation_reuses_single_secret_store_authorization_batch() {
    let dir = temp_dir("mobile-relay-user-level-secret-store-batch");
    let previous = set_portable_data_dir_override(Some(dir.to_path_buf()));
    let store = Arc::new(EphemeralSecretStore::new());
    let mut config = default_config();
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
    save_config(&mut config).unwrap();
    let baseline_session_count = store.authorization_session_count();

    let store_override: Arc<dyn SecureMeshSecretStore> = store.clone();
    let output = with_mobile_relay_secret_store_override(store_override, || {
        config_set(&json!({
            "relayEnabled": false
        }))
    })
    .unwrap();

    assert_eq!(output["ok"], true);
    assert_eq!(
        store.authorization_session_count(),
        baseline_session_count + 1
    );
    assert_eq!(
        store.authorization_session_reasons()[baseline_session_count],
        "Mobile Relay E2EE secret store authorization batch"
    );
    assert_eq!(
        store.authorization_session_operation_counts()[baseline_session_count],
        mobile_relay_e2ee_secret_store_authorization_batch_operation_count()
    );
    let persisted = fs::read_to_string(config_path().unwrap()).unwrap();
    for (field, _) in MOBILE_RELAY_E2EE_NATIVE_SECRET_FIELDS {
        assert!(!persisted.contains(field));
    }

    set_portable_data_dir_override(previous);
}

#[test]
fn mobile_relay_native_secret_store_cleanup_uses_single_authorization_batch() {
    let store = EphemeralSecretStore::new();
    let namespace = "native-secret-store-cleanup-batch";
    let mut config = json!({
        "pcToken": "cleanup-pc-token-canary",
        "mobileToken": "cleanup-mobile-token-canary",
        "mobileRelayE2ee": {},
        "pairedDevices": [
            {
                "id": "cleanup-device",
                "pairingId": "cleanup-pairing",
                "mobileToken": "cleanup-paired-token-canary"
            }
        ]
    });
    for ((field, _), secret) in MOBILE_RELAY_E2EE_NATIVE_SECRET_FIELDS.iter().copied().zip(
        [
            "cleanup-private-key-canary",
            "cleanup-signing-key-canary",
            "cleanup-signed-prekey-canary",
            "cleanup-one-time-prekey-canary",
            "cleanup-pairing-secret-canary",
        ]
        .into_iter(),
    ) {
        config["mobileRelayE2ee"][field] = json!(secret);
    }
    persist_config_secret_material_to_secret_store(&mut config, &store, namespace).unwrap();
    let handles = custody_cleanup_root_secret_handles(&config, namespace).unwrap();
    assert!(!handles.is_empty());
    // Root cleanup deletes the full handle set (bundle + token + field + paired-device
    // keys). Seed any missing handles so the single-batch delete budget is observable.
    for handle in &handles {
        if store.get_secret(handle).unwrap().is_none() {
            store
                .set_secret(
                    handle,
                    SecretBytes::try_from_bytes(b"cleanup-batch-seed-canary".to_vec()).unwrap(),
                )
                .unwrap();
        }
        assert!(store.get_secret(handle).unwrap().is_some());
    }
    let baseline_session_count = store.authorization_session_count();

    cleanup_native_secret_store_fields_for_store(&config, &store, namespace).unwrap();

    assert_eq!(
        store.authorization_session_count(),
        baseline_session_count + 1
    );
    assert_eq!(
        store.authorization_session_reasons()[baseline_session_count],
        "Mobile Relay E2EE secret store cleanup authorization batch"
    );
    assert_eq!(
        store.authorization_session_operation_counts()[baseline_session_count],
        handles.len()
    );
    assert_eq!(
        store.authorization_session_consumed_operation_counts()[baseline_session_count],
        handles.len()
    );
    for handle in &handles {
        assert!(store.get_secret(handle).unwrap().is_none());
    }
}
