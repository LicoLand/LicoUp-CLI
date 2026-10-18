use super::super::json_codec::hex_encode;
use super::super::sparse_map::sparse_map_default_hashes;
use super::super::*;
use super::support::leaf;
use ed25519_dalek::SigningKey;
use rand_core::OsRng;

#[test]
fn rfc9162_inclusion_paths_are_logarithmic_and_exact() {
    let mut log = SecureMeshKtLog::new(SigningKey::generate(&mut OsRng));
    for index in 0..31 {
        log.append_leaf(&leaf(&format!("device-{index}"), 1, "active"))
            .unwrap();
    }
    let pin = log.pin();
    let policy = KtFreshnessPolicy::strict(60, 2).unwrap();
    for index in 0..31 {
        let proof = log.inclusion_proof_at(index, 100).unwrap();
        assert!(proof.siblings.len() <= 5);
        verify_kt_inclusion(&proof, &pin, policy, 100).unwrap();
    }
}

#[test]
fn rfc9162_consistency_paths_are_logarithmic_and_exact() {
    let mut log = SecureMeshKtLog::new(SigningKey::generate(&mut OsRng));
    for index in 0..31 {
        log.append_leaf(&leaf(&format!("device-{index}"), 1, "active"))
            .unwrap();
    }
    let pin = log.pin();
    let policy = KtFreshnessPolicy::strict(60, 2).unwrap();
    for first_size in [1u64, 2, 3, 7, 8, 15, 16, 30] {
        let proof = log.consistency_proof_at(first_size, 100).unwrap();
        assert!(proof.path.len() <= 6);
        let cached = SecureMeshKtCachedCheckpoint {
            tree_size: first_size,
            root_hash: proof.first_root_hash.clone(),
            map_root_hash: hex_encode(&[0u8; 32]),
            issued_at_epoch_seconds: 99,
        };
        verify_kt_consistency(&proof, &pin, policy, 100, &cached).unwrap();
    }
}

#[test]
fn sparse_map_default_hashes_are_cached_once_and_depth_bounded() {
    let first = sparse_map_default_hashes();
    let second = sparse_map_default_hashes();

    assert!(std::ptr::eq(first, second));
    assert_eq!(first.len(), 257);
}

#[test]
fn sparse_map_authenticates_inclusion_absence_and_rejects_label_substitution() {
    let mut log = SecureMeshKtLog::new(SigningKey::generate(&mut OsRng));
    let body = leaf("device-a", 3, "active");
    log.append_leaf(&body).unwrap();
    let pin = log.pin();
    let policy = KtFreshnessPolicy::strict(60, 2).unwrap();
    let present = log.map_proof_at(&body.directory_key(), 100).unwrap();
    verify_kt_map_inclusion(
        &present,
        &body.directory_key(),
        &body.leaf_hash_hex().unwrap(),
        3,
        false,
        &pin,
        policy,
        100,
    )
    .unwrap();

    let mut other_workspace = body.clone();
    other_workspace.directory_scope_commitment =
        directory_scope_commitment("tenant-a", "account-a", "workspace-b");
    assert_ne!(body.directory_key(), other_workspace.directory_key());
    let workspace_absence = log
        .map_proof_at(&other_workspace.directory_key(), 100)
        .unwrap();
    verify_kt_non_inclusion(
        &workspace_absence,
        &other_workspace.directory_key(),
        &pin,
        policy,
        100,
    )
    .unwrap();

    let scope = directory_scope_commitment("tenant-a", "account-a", "workspace-a");
    let missing = stable_directory_label(&scope, "missing");
    let absence = log.map_proof_at(&missing, 100).unwrap();
    assert!(absence.entry.is_none());
    verify_kt_non_inclusion(&absence, &missing, &pin, policy, 100).unwrap();

    let substituted = stable_directory_label(&scope, "other-missing");
    let mut forged = absence.clone();
    forged.stable_label = substituted.clone();
    let error = verify_kt_non_inclusion(&forged, &missing, &pin, policy, 100).unwrap_err();
    assert!(error.to_string().contains("label substitution"));
}

#[test]
fn directory_leaf_serialization_exposes_only_an_opaque_scope_commitment() {
    let body = leaf("device-a", 1, "active");
    let serialized = serde_json::to_string(&body).unwrap();

    assert!(serialized.contains("directoryScopeCommitment"));
    for forbidden in [
        "tenantId",
        "accountId",
        "workspaceId",
        "tenant-a",
        "account-a",
        "workspace-a",
    ] {
        assert!(!serialized.contains(forbidden));
    }
}
