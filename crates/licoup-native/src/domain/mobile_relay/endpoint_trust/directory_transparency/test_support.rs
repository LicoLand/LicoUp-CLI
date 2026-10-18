use super::claim::local_pairwise_prekey_bundle_from_config;
use crate::core::secure_mesh_directory::{
    SecureMeshDirectoryKeyMaterialCommitment, SecureMeshDirectoryLeafClaim,
    UntrustedDirectoryResponse,
};
use crate::core::secure_mesh_prekey::{one_time_prekey_batch_digest, signed_prekey_bundle_digest};
use crate::core::secure_mesh_transparency::{
    SecureMeshKtLog, SecureMeshTransparencyLeafBody, directory_scope_commitment,
};
use crate::domain::mobile_relay::endpoint_trust::{
    descriptor_text, hex_encode_bytes, mobile_relay_trust_record_now_epoch, now_iso,
};
use crate::platform::client_state::ClientStateStore;
use anyhow::{Result, anyhow, ensure};
use ed25519_dalek::SigningKey;
use rand_core::OsRng;
use serde_json::{Value, json};
use std::cell::RefCell;
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::{Mutex, OnceLock};

#[derive(Clone, Eq, Ord, PartialEq, PartialOrd)]
enum MobileRelayTestKtAuthority {
    PortableRoot(PathBuf),
    Scenario(String),
}

thread_local! {
    static MOBILE_RELAY_TEST_KT_AUTHORITY_OVERRIDE: RefCell<Option<String>> =
        const { RefCell::new(None) };
}

static MOBILE_RELAY_TEST_KT_LOGS: OnceLock<
    Mutex<BTreeMap<MobileRelayTestKtAuthority, SecureMeshKtLog>>,
> = OnceLock::new();

struct MobileRelayTestKtAuthorityGuard {
    previous: Option<String>,
}

impl Drop for MobileRelayTestKtAuthorityGuard {
    fn drop(&mut self) {
        MOBILE_RELAY_TEST_KT_AUTHORITY_OVERRIDE.with(|slot| {
            slot.replace(self.previous.take());
        });
    }
}

pub(in crate::domain::mobile_relay) fn with_mobile_relay_test_kt_authority_scope<T>(
    scope: &str,
    operation: impl FnOnce() -> Result<T>,
) -> Result<T> {
    ensure!(
        !scope.is_empty()
            && scope.len() <= 128
            && scope
                .as_bytes()
                .iter()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(*byte, b'-' | b'_')),
        "mobile relay test KT authority scope is invalid"
    );
    let previous =
        MOBILE_RELAY_TEST_KT_AUTHORITY_OVERRIDE.with(|slot| slot.replace(Some(scope.to_string())));
    let guard = MobileRelayTestKtAuthorityGuard { previous };
    ensure!(
        guard.previous.is_none(),
        "mobile relay test KT authority scope is already active"
    );
    operation()
}

pub(in crate::domain::mobile_relay) fn with_mobile_relay_test_kt_log<T>(
    operation: impl FnOnce(&mut SecureMeshKtLog) -> Result<T>,
) -> Result<T> {
    let authority = MOBILE_RELAY_TEST_KT_AUTHORITY_OVERRIDE
        .with(|slot| slot.borrow().clone())
        .map(MobileRelayTestKtAuthority::Scenario)
        .map_or_else(
            || {
                ClientStateStore::portable().map(|store| {
                    MobileRelayTestKtAuthority::PortableRoot(
                        store.root().join("mobile-relay").join("secure-mesh-kt"),
                    )
                })
            },
            Ok,
        )?;
    let logs = MOBILE_RELAY_TEST_KT_LOGS.get_or_init(|| Mutex::new(BTreeMap::new()));
    let mut logs = logs
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let log = logs.entry(authority).or_insert_with(|| {
        SecureMeshKtLog::with_identity(
            SigningKey::generate(&mut OsRng),
            "local-mock-kt-log",
            "local-mock-kt-key",
        )
    });
    operation(log)
}

pub(super) fn uses_local_acceptance_mock(config: &Value) -> bool {
    config
        .get("secureMeshKeyTransparency")
        .and_then(|settings| settings.get("pin"))
        .and_then(|pin| pin.get("provenance"))
        .and_then(Value::as_str)
        == Some("local-acceptance-mock")
}

pub(super) fn provision_mobile_relay_test_key_transparency(config: &mut Value) -> Result<()> {
    let desired_mls_key_package_digest = config
        .get("mobileRelayE2ee")
        .and_then(|state| state.get("mlsKeyPackageDigest"))
        .and_then(Value::as_str)
        .map(str::to_string)
        .unwrap_or_else(|| hex_encode_bytes(&[0u8; 32]));
    let desired_mls_key_package_version = config
        .get("mobileRelayE2ee")
        .and_then(|state| state.get("mlsKeyPackageVersion"))
        .and_then(Value::as_u64)
        .unwrap_or(0);
    let existing_response = config
        .get("mobileRelayE2ee")
        .and_then(|state| state.get("keyTransparencyResponse"))
        .filter(|value| value.is_object())
        .cloned()
        .map(serde_json::from_value::<UntrustedDirectoryResponse>)
        .transpose()
        .map_err(|_| anyhow!("mobile relay test KT response is invalid"))?;
    if existing_response.as_ref().is_some_and(|response| {
        response.claim.key_material.mls_key_package_digest == desired_mls_key_package_digest
            && response.claim.key_material.mls_key_package_version
                == desired_mls_key_package_version
    }) && config
        .get("secureMeshKeyTransparency")
        .and_then(|settings| settings.get("pin"))
        .is_some_and(Value::is_object)
    {
        return Ok(());
    }
    let bundle = local_pairwise_prekey_bundle_from_config(config)?;
    let state = config
        .get("mobileRelayE2ee")
        .ok_or_else(|| anyhow!("mobile relay E2EE endpoint state is missing"))?;
    let endpoint_kind = descriptor_text(state, "endpointKind")?;
    let previous_tree_size = state
        .get("keyTransparencyLastTreeSize")
        .and_then(Value::as_u64);
    let directory_version = existing_response
        .as_ref()
        .map(|response| response.claim.directory_version.saturating_add(1))
        .unwrap_or(bundle.prekey_publication_version);
    let claim = SecureMeshDirectoryLeafClaim {
        endpoint: SecureMeshTransparencyLeafBody {
            directory_scope_commitment: directory_scope_commitment(
                "local-test-tenant",
                "local-test-account",
                "local-test-workspace",
            ),
            endpoint_id: bundle.endpoint_identity.endpoint_id.clone(),
            endpoint_kind,
            identity_public_key: hex_encode_bytes(&bundle.endpoint_identity.identity_public_key),
            signing_public_key: hex_encode_bytes(&bundle.endpoint_identity.signing_public_key),
            fingerprint: bundle.endpoint_identity.fingerprint()?,
            rotation_epoch: bundle.endpoint_identity.rotation_epoch,
            directory_state: "active".to_string(),
            updated_at: now_iso(),
        },
        key_material: SecureMeshDirectoryKeyMaterialCommitment {
            signed_prekey_bundle_digest: signed_prekey_bundle_digest(&bundle)?,
            one_time_prekey_batch_digest: one_time_prekey_batch_digest(&bundle)?,
            pairwise_prekey_version: bundle.prekey_publication_version,
            mls_key_package_digest: desired_mls_key_package_digest,
            mls_key_package_version: desired_mls_key_package_version,
        },
        directory_version,
    };
    let now_epoch_seconds = mobile_relay_trust_record_now_epoch()?;
    let (response, pin, tree_size) = with_mobile_relay_test_kt_log(|log| {
        let index = log.append_hashed_directory_leaf(
            &claim.stable_label(),
            claim.version(),
            claim.revoked(),
            claim.leaf_hash()?,
        )?;
        let response = UntrustedDirectoryResponse {
            claim: claim.clone(),
            inclusion: log.inclusion_proof_at(index, now_epoch_seconds)?,
            latest_map: log.map_proof_at(&claim.stable_label(), now_epoch_seconds)?,
            consistency: previous_tree_size
                .filter(|first| *first < log.tree_size())
                .map(|first| log.consistency_proof_at(first, now_epoch_seconds))
                .transpose()?,
        };
        Ok((response, log.pin(), log.tree_size()))
    })?;
    config["secureMeshKeyTransparency"] = json!({
        "pin": {
            "logId": pin.log_id(),
            "keyId": pin.key_id(),
            "publicKeyHex": pin.public_key_hex(),
            "provenance": pin.provenance().stable_code()
        },
        "maxSthAgeSeconds": 3600,
        "maxFutureSkewSeconds": 300
    });
    config["secureMeshDirectoryScopeCommitment"] =
        json!(&claim.endpoint.directory_scope_commitment);
    config["mobileRelayE2ee"]["keyTransparencyResponse"] = serde_json::to_value(response)?;
    config["mobileRelayE2ee"]["keyTransparencyLastTreeSize"] = json!(tree_size);
    Ok(())
}

pub(super) fn refresh_mobile_relay_test_directory_response(
    response_value: Value,
    previous_tree_size: Option<u64>,
    now_epoch_seconds: u64,
) -> Result<Value> {
    let stale_response: UntrustedDirectoryResponse = serde_json::from_value(response_value)
        .map_err(|_| anyhow!("mobile relay test key transparency response is invalid"))?;
    let stable_label = stale_response.claim.stable_label();
    let response = with_mobile_relay_test_kt_log(|log| {
        if let Some(previous_tree_size) = previous_tree_size {
            ensure!(
                previous_tree_size <= log.tree_size(),
                "mobile relay test KT checkpoint is ahead of the local mock authority"
            );
        }
        let current_tree_size = log.tree_size();
        ensure!(
            current_tree_size > 0,
            "mobile relay test KT authority has no authenticated map checkpoint"
        );
        let index = current_tree_size - 1;
        Ok(UntrustedDirectoryResponse {
            claim: stale_response.claim,
            inclusion: log.inclusion_proof_at(index, now_epoch_seconds)?,
            latest_map: log.map_proof_at(&stable_label, now_epoch_seconds)?,
            consistency: previous_tree_size
                .filter(|size| *size < current_tree_size)
                .map(|size| log.consistency_proof_at(size, now_epoch_seconds))
                .transpose()?,
        })
    })?;
    serde_json::to_value(response).map_err(Into::into)
}
