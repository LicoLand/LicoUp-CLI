use ed25519_dalek::{SigningKey, VerifyingKey};
use rand_core::OsRng;
use time::{OffsetDateTime, format_description::well_known::Rfc3339};

use super::super::{
    SecureMeshPairwisePreKeyBundle, SecureMeshPreKeyKind, SecureMeshPreKeyRecord,
    one_time_prekey_batch_digest, sign_prekey_record, signed_prekey_bundle_digest,
};
use crate::core::secure_mesh_directory::{
    AuthorizedDirectoryLeaf, DirectoryAuthorizationPurpose, SecureMeshDirectoryAuthority,
    SecureMeshDirectoryKeyMaterialCommitment, SecureMeshDirectoryLeafClaim,
    UntrustedDirectoryResponse,
};
use crate::core::secure_mesh_pqxdh::SecureMeshMlKem1024PreKeySeed;
use crate::core::secure_mesh_transparency::{
    KtFreshnessPolicy, SecureMeshKtLog, SecureMeshTransparencyLeafBody, directory_scope_commitment,
};
use crate::core::secure_mesh_trust::DeviceTrustPublicIdentity;

pub(super) const NOW: &str = "2026-01-01T00:00:00Z";
pub(super) const CREATED_AT: &str = "2026-01-01T00:00:00Z";
pub(super) const EXPIRES_AT: &str = "2026-01-02T00:00:00Z";

pub(crate) fn authorize_test_pairwise_prekey_bundle(
    bundle: &SecureMeshPairwisePreKeyBundle,
) -> AuthorizedDirectoryLeaf {
    authorize_test_pairwise_prekey_bundle_with_purpose(
        bundle,
        DirectoryAuthorizationPurpose::PairwiseSessionBootstrap,
    )
}

pub(crate) fn authorize_test_pairwise_prekey_bundle_with_purpose(
    bundle: &SecureMeshPairwisePreKeyBundle,
    purpose: DirectoryAuthorizationPurpose,
) -> AuthorizedDirectoryLeaf {
    let claim = SecureMeshDirectoryLeafClaim {
        endpoint: SecureMeshTransparencyLeafBody {
            directory_scope_commitment: directory_scope_commitment(
                "test-tenant",
                "test-account",
                "test-workspace",
            ),
            endpoint_id: bundle.endpoint_identity.endpoint_id.clone(),
            endpoint_kind: "test".to_string(),
            identity_public_key: hex_bytes(&bundle.endpoint_identity.identity_public_key),
            signing_public_key: hex_bytes(&bundle.endpoint_identity.signing_public_key),
            fingerprint: bundle.endpoint_identity.fingerprint().unwrap(),
            rotation_epoch: bundle.endpoint_identity.rotation_epoch,
            directory_state: "active".to_string(),
            updated_at: "2026-07-12T00:00:00Z".to_string(),
        },
        key_material: SecureMeshDirectoryKeyMaterialCommitment {
            signed_prekey_bundle_digest: signed_prekey_bundle_digest(bundle).unwrap(),
            one_time_prekey_batch_digest: one_time_prekey_batch_digest(bundle).unwrap(),
            pairwise_prekey_version: bundle.prekey_publication_version,
            mls_key_package_digest: hex_bytes(&[0u8; 32]),
            mls_key_package_version: 0,
        },
        directory_version: bundle.prekey_publication_version,
    };
    let now = 1_800_000_000;
    let mut log = SecureMeshKtLog::new(SigningKey::generate(&mut OsRng));
    let index = log
        .append_hashed_directory_leaf(
            &claim.stable_label(),
            claim.version(),
            claim.revoked(),
            claim.leaf_hash().unwrap(),
        )
        .unwrap();
    let response = UntrustedDirectoryResponse {
        claim,
        inclusion: log.inclusion_proof_at(index, now).unwrap(),
        latest_map: log
            .map_proof_at(&bundle_directory_label(bundle), now)
            .unwrap(),
        consistency: None,
    };
    let mut authority = SecureMeshDirectoryAuthority::open_in_memory(
        log.pin(),
        KtFreshnessPolicy::strict(60, 2).unwrap(),
    )
    .unwrap();
    authority.authorize(response, purpose, now).unwrap()
}

pub(super) fn signed_prekey_fixture(
    signing_key: &SigningKey,
    identity: &DeviceTrustPublicIdentity,
    prekey_id: &str,
) -> SecureMeshPreKeyRecord {
    sign_prekey_record(
        signing_key,
        identity,
        SecureMeshPreKeyKind::SignedPreKey,
        prekey_id,
        vec![1; 32],
        CREATED_AT,
        EXPIRES_AT,
    )
    .unwrap()
}

pub(super) fn one_time_prekey_fixture(
    signing_key: &SigningKey,
    identity: &DeviceTrustPublicIdentity,
    prekey_id: &str,
) -> SecureMeshPreKeyRecord {
    sign_prekey_record(
        signing_key,
        identity,
        SecureMeshPreKeyKind::OneTimePreKey,
        prekey_id,
        vec![2; 32],
        CREATED_AT,
        EXPIRES_AT,
    )
    .unwrap()
}

pub(super) fn mlkem_prekey_fixture(
    signing_key: &SigningKey,
    identity: &DeviceTrustPublicIdentity,
    prekey_id: &str,
    seed_byte: u8,
) -> SecureMeshPreKeyRecord {
    let seed = SecureMeshMlKem1024PreKeySeed::from_bytes([seed_byte; 64]);
    sign_prekey_record(
        signing_key,
        identity,
        SecureMeshPreKeyKind::OneTimeMlKem1024PreKey,
        prekey_id,
        seed.public_key(),
        CREATED_AT,
        EXPIRES_AT,
    )
    .unwrap()
}

pub(super) fn identity_fixture(endpoint_id: &str) -> (SigningKey, DeviceTrustPublicIdentity) {
    let identity_key = SigningKey::generate(&mut OsRng);
    let signing_key = SigningKey::generate(&mut OsRng);
    let identity = DeviceTrustPublicIdentity::new(
        endpoint_id,
        VerifyingKey::from(&identity_key).to_bytes(),
        VerifyingKey::from(&signing_key).to_bytes(),
        1,
    )
    .unwrap();
    (signing_key, identity)
}

pub(super) fn deterministic_identity_fixture(
    endpoint_id: &str,
) -> (SigningKey, DeviceTrustPublicIdentity) {
    let identity_key = SigningKey::from_bytes(&[0x11; 32]);
    let signing_key = SigningKey::from_bytes(&[0x22; 32]);
    let identity = DeviceTrustPublicIdentity::new(
        endpoint_id,
        VerifyingKey::from(&identity_key).to_bytes(),
        VerifyingKey::from(&signing_key).to_bytes(),
        7,
    )
    .unwrap();
    (signing_key, identity)
}

pub(super) fn now() -> OffsetDateTime {
    OffsetDateTime::parse(NOW, &Rfc3339).unwrap()
}

fn bundle_directory_label(bundle: &SecureMeshPairwisePreKeyBundle) -> String {
    let scope = directory_scope_commitment("test-tenant", "test-account", "test-workspace");
    crate::core::secure_mesh_transparency::stable_directory_label(
        &scope,
        &bundle.endpoint_identity.endpoint_id,
    )
}

fn hex_bytes(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}
