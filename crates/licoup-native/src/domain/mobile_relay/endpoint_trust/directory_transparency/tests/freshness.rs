use super::super::freshness::ensure_pairwise_authorization_receipt_current;
use crate::core::secure_mesh_transparency::SecureMeshKtAuthorizationReceipt;

fn receipt(tree_size: u64, revoked: bool) -> SecureMeshKtAuthorizationReceipt {
    SecureMeshKtAuthorizationReceipt {
        stable_label: "label".to_string(),
        purpose: "pairwise-signed-prekey".to_string(),
        directory_version: 1,
        leaf_hash: "a".repeat(64),
        revoked,
        tree_size,
        root_hash: "b".repeat(64),
        map_root_hash: "c".repeat(64),
        issued_at_epoch_seconds: 1,
        observed_at_epoch_seconds: 1,
        validated_at_epoch_seconds: 1,
        expires_at_epoch_seconds: 2,
        identity_fingerprint: "fingerprint".to_string(),
        identity_rotation_epoch: 1,
        identity_key_digest: "d".repeat(64),
        pairwise_prekey_version: 1,
        signed_prekey_digest: "e".repeat(64),
        one_time_prekey_digest: "f".repeat(64),
        mls_key_package_version: 0,
        mls_key_package_digest: "0".repeat(64),
    }
}

#[test]
fn freshness_receipt_requires_current_tree_and_active_claim() {
    assert!(ensure_pairwise_authorization_receipt_current(&receipt(7, false), 7).is_ok());
    assert!(ensure_pairwise_authorization_receipt_current(&receipt(6, false), 7).is_err());
    assert!(ensure_pairwise_authorization_receipt_current(&receipt(7, true), 7).is_err());
}
