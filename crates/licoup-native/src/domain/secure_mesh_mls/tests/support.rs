use ed25519_dalek::SigningKey;
use rand_core::OsRng;
use serde_json::{Value, json};

use crate::core::secure_mesh_directory::{
    SecureMeshDirectoryKeyMaterialCommitment, SecureMeshDirectoryLeafClaim,
    UntrustedDirectoryResponse,
};
use crate::core::secure_mesh_transparency::{
    SecureMeshKtLog, SecureMeshTransparencyLeafBody, directory_scope_commitment,
};
use crate::core::secure_mesh_trust::DeviceTrustPublicIdentity;

use super::super::input_codec::hex_sha256;

pub(super) fn test_identity(endpoint_id: &str) -> DeviceTrustPublicIdentity {
    let identity_key = SigningKey::generate(&mut OsRng);
    let signing_key = SigningKey::generate(&mut OsRng);
    DeviceTrustPublicIdentity::new(
        endpoint_id,
        identity_key.verifying_key().to_bytes(),
        signing_key.verifying_key().to_bytes(),
        1,
    )
    .unwrap()
}

pub(super) fn test_directory_claim(
    member: &DeviceTrustPublicIdentity,
    directory_version: u64,
    mls_key_package_version: u64,
    mls_key_package_digest: &str,
) -> SecureMeshDirectoryLeafClaim {
    SecureMeshDirectoryLeafClaim {
        endpoint: SecureMeshTransparencyLeafBody {
            directory_scope_commitment: directory_scope_commitment(
                "test-tenant",
                "test-account",
                "test-workspace",
            ),
            endpoint_id: member.endpoint_id.clone(),
            endpoint_kind: "test".to_string(),
            identity_public_key: member
                .identity_public_key
                .iter()
                .map(|byte| format!("{byte:02x}"))
                .collect(),
            signing_public_key: member
                .signing_public_key
                .iter()
                .map(|byte| format!("{byte:02x}"))
                .collect(),
            fingerprint: member.fingerprint().unwrap(),
            rotation_epoch: member.rotation_epoch,
            directory_state: "active".to_string(),
            updated_at: "2026-07-12T00:00:00Z".to_string(),
        },
        key_material: SecureMeshDirectoryKeyMaterialCommitment {
            signed_prekey_bundle_digest: hex_sha256(b"test-signed-prekey-bundle"),
            one_time_prekey_batch_digest: hex_sha256(b"test-one-time-prekey-batch"),
            pairwise_prekey_version: 1,
            mls_key_package_digest: mls_key_package_digest.to_string(),
            mls_key_package_version,
        },
        directory_version,
    }
}

pub(super) fn append_test_directory_response(
    log: &mut SecureMeshKtLog,
    claim: &SecureMeshDirectoryLeafClaim,
    issued_at: u64,
    previous_tree_size: Option<u64>,
) -> UntrustedDirectoryResponse {
    let index = log
        .append_hashed_directory_leaf(
            &claim.stable_label(),
            claim.version(),
            claim.revoked(),
            claim.leaf_hash().unwrap(),
        )
        .unwrap();
    UntrustedDirectoryResponse {
        claim: claim.clone(),
        inclusion: log.inclusion_proof_at(index, issued_at).unwrap(),
        latest_map: log.map_proof_at(&claim.stable_label(), issued_at).unwrap(),
        consistency: previous_tree_size
            .map(|size| log.consistency_proof_at(size, issued_at).unwrap()),
    }
}

pub(super) fn test_kt_config(log: &SecureMeshKtLog) -> Value {
    let pin = log.pin();
    json!({
        "secureMeshKeyTransparency": {
            "pin": {
                "logId": pin.log_id(),
                "keyId": pin.key_id(),
                "publicKeyHex": pin.public_key_hex(),
                "provenance": pin.provenance().stable_code()
            },
            "maxSthAgeSeconds": 60,
            "maxFutureSkewSeconds": 2
        }
    })
}
