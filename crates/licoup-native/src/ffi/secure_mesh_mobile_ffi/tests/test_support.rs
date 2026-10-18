pub(super) use super::super::{
    action_catalog::MOBILE_RELAY_NATIVE_ACTIONS,
    dispatch_context::{
        dispatch_json_with_files_dir, dispatch_json_with_files_dir_and_pairwise_secret_store,
    },
    dispatch_router::dispatch_json,
    feature_status::{EXPECTED_FEATURES, runtime_feature_flags, runtime_self_test},
    fixture_trust::{hex_bytes, native_device_identity_fixture},
    protected_operation::secure_mesh_action_requires_protected_operation_gate,
    request_validation::{
        MAX_FFI_JSON_DEPTH, MAX_FFI_JSON_NODES, MAX_FFI_OBJECT_FIELDS, MAX_FFI_REQUEST_BYTES,
        MAX_FFI_STRING_BYTES,
    },
};
pub(super) use crate::core::secure_mesh_directory::{
    SecureMeshDirectoryLeafClaim, UntrustedDirectoryResponse,
};
pub(super) use crate::core::secure_mesh_transparency::{
    SecureMeshKtLog, directory_scope_commitment,
};
pub(super) use crate::platform::secure_mesh_secret_store::{
    EphemeralSecretStore, SecretBytes, SecureMeshSecretStore,
};
pub(super) use base64::{Engine as _, engine::general_purpose};
pub(super) use ed25519_dalek::SigningKey;
pub(super) use rand_core::OsRng;
pub(super) use serde_json::{Value, json};
pub(super) use std::sync::Arc;

pub(super) struct CapabilityOnlySecretStore;

impl SecureMeshSecretStore for CapabilityOnlySecretStore {
    fn backend(&self) -> &'static str {
        "capability-only-test-store"
    }

    fn supported(&self) -> bool {
        true
    }

    fn capability_facts(
        &self,
    ) -> anyhow::Result<Vec<crate::core::secure_mesh_capability::CapabilityFact>> {
        use crate::core::secure_mesh_capability::{
            CapabilityEvidenceKind, CapabilityFact, SecurityCapability,
        };
        Ok(vec![
            CapabilityFact::supported(
                SecurityCapability::OsSecureStore,
                CapabilityEvidenceKind::RuntimeOperation,
            ),
            CapabilityFact::supported(
                SecurityCapability::AppleKeychain,
                CapabilityEvidenceKind::RuntimeOperation,
            ),
        ])
    }

    fn begin_authorized_session(
        &self,
        _request: &crate::platform::secure_mesh_secret_store::SecretStoreAuthorizationRequest,
    ) -> anyhow::Result<crate::platform::secure_mesh_secret_store::SecretStoreAuthorizationSession>
    {
        panic!("status capability projection must not request authorization")
    }

    fn set_secret(
        &self,
        _handle: &crate::platform::secure_mesh_secret_store::SecretStoreHandle,
        _secret: SecretBytes,
    ) -> anyhow::Result<()> {
        panic!("status capability projection must not write secrets")
    }

    fn get_secret(
        &self,
        _handle: &crate::platform::secure_mesh_secret_store::SecretStoreHandle,
    ) -> anyhow::Result<Option<SecretBytes>> {
        panic!("status capability projection must not read secrets")
    }

    fn delete_secret(
        &self,
        _handle: &crate::platform::secure_mesh_secret_store::SecretStoreHandle,
    ) -> anyhow::Result<()> {
        panic!("status capability projection must not delete secrets")
    }
}

pub(super) fn initialize_mls_ffi_client(
    files_dir: &std::path::Path,
    store: Arc<EphemeralSecretStore>,
    endpoint_kind: &str,
) {
    let portable_dir = files_dir.join("portable-data");
    let previous =
        licoup_foundation::platform::paths::set_portable_data_dir_override(Some(portable_dir));
    let selected: Arc<dyn SecureMeshSecretStore> = store;
    crate::domain::mobile_relay::with_mobile_relay_secret_store_override(selected, || {
        crate::domain::mobile_relay::initialize_secure_mesh_mls_test_endpoint(endpoint_kind)
    })
    .unwrap();
    licoup_foundation::platform::paths::set_portable_data_dir_override(previous);
}

pub(super) fn initialize_mls_ffi_peer(
    files_dir: &std::path::Path,
    store: Arc<EphemeralSecretStore>,
    peer: &Value,
) {
    let peer_identity = mls_ffi_identity(peer);
    let portable_dir = files_dir.join("portable-data");
    let previous =
        licoup_foundation::platform::paths::set_portable_data_dir_override(Some(portable_dir));
    let selected: Arc<dyn SecureMeshSecretStore> = store;
    crate::domain::mobile_relay::with_mobile_relay_secret_store_override(selected, || {
        crate::domain::mobile_relay::initialize_secure_mesh_mls_test_peer(&peer_identity)
    })
    .unwrap();
    licoup_foundation::platform::paths::set_portable_data_dir_override(previous);
}

pub(super) fn mls_ffi_identity(
    value: &Value,
) -> crate::core::secure_mesh_trust::DeviceTrustPublicIdentity {
    let decode_key = |field: &str| -> [u8; 32] {
        general_purpose::URL_SAFE_NO_PAD
            .decode(value[field].as_str().unwrap())
            .unwrap()
            .try_into()
            .unwrap()
    };
    crate::core::secure_mesh_trust::DeviceTrustPublicIdentity::new(
        value["endpointId"].as_str().unwrap(),
        decode_key("identityPublicKeyBase64url"),
        decode_key("signingPublicKeyBase64url"),
        value["rotationEpoch"].as_u64().unwrap(),
    )
    .unwrap()
}

#[track_caller]
pub(super) fn call_mls_ffi(
    files_dir: &std::path::Path,
    store: Arc<EphemeralSecretStore>,
    action: &str,
    params: Value,
) -> Value {
    let selected: Arc<dyn SecureMeshSecretStore> = store;
    dispatch_json_with_files_dir_and_pairwise_secret_store(
        &json!({"action": action, "params": params}).to_string(),
        files_dir.to_string_lossy().as_ref(),
        "test_secure_mesh_action_unsupported",
        selected,
    )
    .unwrap_or_else(|error| panic!("{action} failed with redacted error: {error}"))
}
