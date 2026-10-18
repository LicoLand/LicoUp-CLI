use super::super::test_support::*;
#[test]
fn mobile_ffi_dispatcher_callback_store_keeps_public_reads_no_auth_until_authorized() {
    let files_dir = temp_dir("mobile-ffi-dispatcher-secret-store");
    let portable_dir = files_dir.join("portable-data");
    let previous = set_portable_data_dir_override(Some(portable_dir.to_path_buf()));
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
    let secret_values = MobileRelayE2eeSecretField::ALL
        .iter()
        .map(|field| test_runtime_e2ee_secret(stringify!(&mobile_config), *field))
        .collect::<Vec<_>>();
    for ((field, _), secret) in MOBILE_RELAY_E2EE_NATIVE_SECRET_FIELDS
        .iter()
        .zip(&secret_values)
    {
        mobile_config["mobileRelayE2ee"][*field] = json!(secret);
    }
    // This is a pre-binding published document, not a new durable write by the
    // current owner. Its first authorized normalization adopts the real bridge
    // custody owner; a public read must not perform that migration.
    mobile_config["mobileRelayE2ee"]
        .as_object_mut()
        .unwrap()
        .remove(CUSTODY_NAMESPACE_FIELD);
    licoup_foundation::platform::file_security::atomic_write_private_text_bounded(
        &config_path().unwrap(),
        &serde_json::to_string(&mobile_config).unwrap(),
        CONFIG_MAX_BYTES,
    )
    .unwrap();
    set_portable_data_dir_override(previous);

    let store_override: Arc<dyn SecureMeshSecretStore> = store.clone();
    let public_config =
        crate::ffi::secure_mesh_mobile_ffi::dispatch_json_with_files_dir_and_pairwise_secret_store(
            &json!({
                "action": "mobile.relay.config.get",
                "params": {}
            })
            .to_string(),
            files_dir.to_string_lossy().as_ref(),
            "ios_secure_mesh_native_json_action_unsupported",
            store_override,
        )
        .unwrap();

    assert_eq!(public_config["ok"], true);
    assert_eq!(store.authorization_session_count(), 0);
    let public_config_text = serde_json::to_string(&public_config).unwrap();
    for secret in secret_values.iter() {
        assert!(!public_config_text.contains(secret));
    }

    let store_override: Arc<dyn SecureMeshSecretStore> = store.clone();
    let public_status =
        crate::ffi::secure_mesh_mobile_ffi::dispatch_json_with_files_dir_and_pairwise_secret_store(
            &json!({
                "action": "mobile.relay.e2ee.status",
                "params": {}
            })
            .to_string(),
            files_dir.to_string_lossy().as_ref(),
            "ios_secure_mesh_native_json_action_unsupported",
            store_override,
        )
        .unwrap();

    assert_eq!(public_status["ok"], true);
    assert_eq!(public_status["fullStatusAuthorized"], false);
    assert_eq!(public_status["authorizationRequiredForFullStatus"], true);
    assert!(public_status["local"].is_object());
    assert_eq!(store.authorization_session_count(), 0);
    let public_status_text = serde_json::to_string(&public_status).unwrap();
    for secret in secret_values.iter() {
        assert!(!public_status_text.contains(secret));
    }
    let bundle_handle = native_e2ee_secret_bundle_handle_for_namespace(
        MOBILE_RELAY_PLATFORM_SECRET_STORE_NAMESPACE,
    )
    .unwrap();
    assert!(store.get_secret(&bundle_handle).unwrap().is_none());

    let store_override: Arc<dyn SecureMeshSecretStore> = store.clone();
    let response =
        crate::ffi::secure_mesh_mobile_ffi::dispatch_json_with_files_dir_and_pairwise_secret_store(
            &json!({
                "action": "mobile.relay.e2ee.status",
                "params": {
                    "authorize": true
                }
            })
            .to_string(),
            files_dir.to_string_lossy().as_ref(),
            "ios_secure_mesh_native_json_action_unsupported",
            store_override,
        )
        .unwrap();

    assert_eq!(response["ok"], true);
    assert_eq!(response["fullStatusAuthorized"], true);
    assert_eq!(
        response["secretStore"]["selectedBackend"],
        "memory-only-ephemeral"
    );
    assert_eq!(
        response["secretStore"]["allPrivateKeysInSelectedCustody"],
        true
    );
    assert_eq!(
        response["secretStore"]["capabilityReport"]["custody"]["strategy"],
        "memory_only_ephemeral"
    );
    assert_eq!(store.authorization_session_count(), 2);
    assert_eq!(
        store.authorization_session_reasons(),
        vec![
            "Mobile Relay E2EE secret bundle persistence".to_string(),
            "Mobile Relay E2EE status authorization batch".to_string()
        ]
    );
    assert!(store.get_secret(&bundle_handle).unwrap().is_some());

    let previous = set_portable_data_dir_override(Some(files_dir.join("portable-data")));
    let persisted =
        serde_json::from_str::<Value>(&fs::read_to_string(config_path().unwrap()).unwrap())
            .unwrap();
    let persisted_text = serde_json::to_string(&persisted).unwrap();
    for ((field, material_field), secret) in MOBILE_RELAY_E2EE_NATIVE_SECRET_FIELDS
        .iter()
        .zip(secret_values.iter())
    {
        assert!(persisted["mobileRelayE2ee"].get(*field).is_none());
        assert_eq!(persisted["mobileRelayE2ee"][*material_field], "redacted");
        assert!(!persisted_text.contains(*field));
        assert!(!persisted_text.contains(secret));
    }
    set_portable_data_dir_override(previous);
}
