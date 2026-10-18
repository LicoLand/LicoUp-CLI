use super::test_support::*;

#[test]
fn mobile_ffi_self_test_covers_native_secure_mesh_runtime() {
    let root = std::env::temp_dir().join(format!(
        "lico-mobile-ffi-pure-runtime-probe-{}",
        uuid::Uuid::new_v4()
    ));
    let previous =
        licoup_foundation::platform::paths::set_portable_data_dir_override(Some(root.clone()));
    assert_eq!(runtime_feature_flags(), EXPECTED_FEATURES);
    assert!(runtime_self_test());
    licoup_foundation::platform::paths::set_portable_data_dir_override(previous);
    assert!(!root.exists());
}

#[test]
fn mobile_ffi_status_projects_exact_client_capabilities() {
    let store: Arc<dyn SecureMeshSecretStore> = Arc::new(CapabilityOnlySecretStore);
    let response =
        crate::domain::mobile_relay::with_mobile_relay_secret_store_override(store, || {
            dispatch_json(
                &json!({"action": "secure_mesh.status", "params": {}}),
                "mobile_secure_mesh_native_json_action_unsupported",
            )
        })
        .unwrap();
    assert_eq!(
        response["capabilityProjection"]["schemaVersion"],
        crate::core::secure_mesh_capability_proof::CLIENT_CAPABILITY_PROJECTION_SCHEMA_VERSION
    );
    assert!(response["capabilityProjection"]["local"]["enabled"].is_array());
    assert!(
        response["capabilityProjection"]["local"]["enabled"]
            .as_array()
            .is_some_and(|enabled| enabled.contains(&json!("custody.os_secure_store")))
    );
    assert!(
        response["capabilityProjection"]["local"]["enabled"]
            .as_array()
            .is_some_and(|enabled| enabled.contains(&json!("custody.apple_keychain")))
    );
    assert!(response["capabilityProjection"]["peer"].is_null());
    assert_eq!(
        response["capabilityProjection"]["negotiatedProtocolCapabilities"],
        json!([])
    );
    assert_eq!(response["pairwiseKem"]["parameterSet"], "ML-KEM-1024");
    assert_eq!(response["pairwiseKem"]["standard"], "FIPS 203");
    assert_eq!(
        response["pairwiseKem"]["publicKeyBytes"],
        crate::core::secure_mesh_pqxdh::ML_KEM_1024_PUBLIC_KEY_BYTES
    );
    assert_eq!(
        response["pairwiseKem"]["ciphertextBytes"],
        crate::core::secure_mesh_pqxdh::ML_KEM_1024_CIPHERTEXT_BYTES
    );
}
