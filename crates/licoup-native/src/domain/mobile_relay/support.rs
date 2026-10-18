pub(super) use crate::platform::client_state::ClientStateStore;
pub(super) use anyhow::{Result, anyhow, ensure};
pub(super) use base64::{Engine, engine::general_purpose};
#[cfg(test)]
pub(super) use ed25519_dalek::SigningKey;
pub(super) use hmac::{Hmac, Mac};
pub(super) use rand_core::{OsRng, RngCore};
pub(super) use serde_json::{Value, json};
pub(super) use sha2::{Digest, Sha256};
#[cfg(test)]
pub(super) use std::cell::RefCell;
pub(super) use std::collections::BTreeSet;
pub(super) use std::fs;
pub(super) use std::path::PathBuf;
pub(super) use time::{Duration, OffsetDateTime, format_description::well_known::Rfc3339};
pub(super) use uuid::Uuid;

pub(super) use super::config::{
    effective_station_base_url, normalize_station_fields, validated_station_base_url,
};
pub(super) use super::secret_custody::{
    CONFIG_SCHEMA_VERSION, RuntimeSecretContext, ensure_secure_mesh_protected_operation_allowed,
    load_config_for_read, load_config_with_runtime_secret_context, load_config_without_persistence,
    public_secret_storage_backend, save_config_with_runtime_secret_context, secret_present,
};
#[cfg(test)]
pub(super) use super::secret_custody::{
    load_config, load_config_with_runtime_secret_context_for_operation,
    mobile_relay_e2ee_secret_store_authorization_batch_operation_count,
};
#[cfg(test)]
pub(super) use crate::core::secure_mesh_capability::SecurityCapability;
pub(super) use crate::core::secure_mesh_directory::{
    AuthorizedDirectoryLeaf, DirectoryAuthorizationPurpose, UntrustedDirectoryResponse,
};
#[cfg(test)]
pub(super) use crate::core::secure_mesh_directory::{
    SecureMeshDirectoryKeyMaterialCommitment, SecureMeshDirectoryLeafClaim,
};
pub(super) use crate::core::secure_mesh_pairwise::{
    SECURE_MESH_PAIRWISE_CIPHER_SUITE, SecureMeshPairwiseSessionAccepted,
    SecureMeshPairwiseSessionFinished, SecureMeshPairwiseSessionIntro,
};
#[cfg(test)]
pub(super) use crate::core::secure_mesh_pqxdh::ML_KEM_1024_KEY_GENERATION_SEED_BYTES;
pub(super) use crate::core::secure_mesh_pqxdh::{
    ML_KEM_1024_CIPHERTEXT_BYTES, ML_KEM_1024_PUBLIC_KEY_BYTES,
};
pub(super) use crate::core::secure_mesh_prekey::{
    SecureMeshPairwisePreKeyBundle, SecureMeshPreKeyRecord,
};
pub(super) use crate::core::secure_mesh_transparency::stable_directory_label;
#[cfg(test)]
pub(super) use crate::core::secure_mesh_transparency::{
    SecureMeshKtGossipPayload, SecureMeshTransparencyLeafBody, directory_scope_commitment,
};
pub(super) use crate::core::secure_mesh_trust::{
    DeviceTrustPublicIdentity, DeviceTrustState, ProtectedSendAuthorization,
    ProtectedSendPayloadKind, authorize_protected_send_from_trust_record,
    device_trust_record_to_json, qr_verification_payload, sas_decimal_chunks,
    sign_device_trust_record, verify_device_trust_record_json,
};
#[cfg(test)]
pub(super) use crate::platform::secure_mesh_secret_store::{
    SecretStoreAuthorizationRequest, SecretStoreAuthorizationSession,
};
#[cfg(test)]
pub(super) use crate::platform::secure_mesh_secret_store::{
    SecretStoreHandle, SecureMeshSecretStore,
};

pub(super) type MobileRelayClaimMac = Hmac<Sha256>;

pub(super) const MOBILE_RELAY_E2EE_PROTOCOL_VERSION: &str =
    "licomesh.mobile-relay.e2ee.pqxdh-mlkem1024.v1";
pub(super) const SECURE_MESH_ENVELOPE_COMMAND: &str = "secure_mesh.envelope";
pub(super) const MOBILE_RELAY_COMMAND_TTL_SECONDS: i64 = 10 * 60;
pub(super) const MOBILE_RELAY_RESULT_TTL_SECONDS: i64 = 10 * 60;
pub(super) const MOBILE_RELAY_KEY_BYTES: usize = 32;
pub(super) const MOBILE_RELAY_PREKEY_VALIDITY_DAYS: i64 = 30;
pub(super) const MOBILE_RELAY_TRUST_RECORD_VALIDITY_DAYS: i64 = 90;
pub(super) const SECURE_MESH_ENDPOINT_CRYPTO_RUNTIME_FAILED_CODE: &str =
    "secure_mesh_endpoint_crypto_runtime_failed";
pub(super) const SECURE_MESH_ENDPOINT_CRYPTO_RUNTIME_FAILED_DETAIL: &str =
    "secure mesh endpoint could not open or execute command; details are local-only";
pub(super) const SECURE_MESH_PEER_TRUST_AUTHORITY_SCHEMA: &str =
    "licomesh.secure-mesh.peer-trust-authority.v1";
pub(super) const MAX_SECURE_MESH_PEER_TRUST_ENTRIES: usize = 256;

thread_local! {
    #[cfg(test)]
    pub(super) static KT_FRESHNESS_NOW_OVERRIDE: RefCell<Option<u64>> = const { RefCell::new(None) };
}

pub(super) fn text_param(params: &Value, keys: &[&str]) -> Option<String> {
    keys.iter()
        .find_map(|key| params.get(*key).and_then(Value::as_str))
        .map(|value| value.trim().to_string())
}

pub(super) fn bool_param(params: &Value, keys: &[&str]) -> Option<bool> {
    for key in keys {
        if let Some(value) = params.get(*key) {
            if let Some(bool_value) = value.as_bool() {
                return Some(bool_value);
            }
            if let Some(text) = value.as_str() {
                return match text.trim().to_lowercase().as_str() {
                    "true" | "1" | "yes" | "on" => Some(true),
                    "false" | "0" | "no" | "off" => Some(false),
                    _ => None,
                };
            }
        }
    }
    None
}

pub(super) fn ensure_only_known_params(
    params: &Value,
    allowed: &[&str],
    label: &str,
) -> Result<()> {
    let object = params
        .as_object()
        .ok_or_else(|| anyhow!("{label} parameters must be an object"))?;
    ensure!(
        object.keys().all(|key| allowed.contains(&key.as_str())),
        "{label} contains an unsupported field"
    );
    Ok(())
}

pub(super) fn json_param(params: &Value, key: &str) -> Option<Value> {
    let value = params.get(key)?;
    if value.is_object() || value.is_array() {
        return Some(value.clone());
    }
    value.as_str().and_then(parse_json_value_param)
}

pub(super) fn json_file_param(params: &Value, keys: &[&str]) -> Result<Option<Value>> {
    let Some(path) = text_param(params, keys).filter(|value| !value.trim().is_empty()) else {
        return Ok(None);
    };
    let text = fs::read_to_string(&path)
        .map_err(|error| anyhow!("failed to read JSON parameter file {}: {}", path, error))?;
    let text = text.trim_start_matches('\u{feff}');
    let value = serde_json::from_str::<Value>(text)
        .map_err(|error| anyhow!("failed to parse JSON parameter file {}: {}", path, error))?;
    Ok(Some(value))
}

pub(super) fn parse_json_value_param(text: &str) -> Option<Value> {
    let parsed = serde_json::from_str::<Value>(text).ok()?;
    if let Some(inner) = parsed.as_str() {
        let trimmed = inner.trim();
        if trimmed.starts_with('{') || trimmed.starts_with('[') {
            return serde_json::from_str::<Value>(trimmed).ok().or(Some(parsed));
        }
    }
    Some(parsed)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn support_module_parses_nested_json_without_text_projection() {
        let parsed = parse_json_value_param(r#""{\"kind\":\"secure\",\"enabled\":true}""#)
            .expect("nested JSON string should parse");

        assert_eq!(parsed["kind"], "secure");
        assert_eq!(parsed["enabled"], true);
        assert!(ensure_only_known_params(&parsed, &["kind", "enabled"], "fixture").is_ok());
    }
}
