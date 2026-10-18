use super::*;
use crate::core::secure_mesh_transparency::{
    KtAuthorityProvenance, SecureMeshKtLog, directory_scope_commitment, stable_directory_label,
};
use ed25519_dalek::SigningKey;
use rand_core::OsRng;

#[test]
fn typed_authority_carries_identity_pairwise_mls_and_test_provenance() {
    let mut log = SecureMeshKtLog::new(SigningKey::generate(&mut OsRng));
    let claim = claim("device-a", 1, "active", 1);
    let response = append_response(&mut log, &claim, 100, None);
    let mut authority = SecureMeshDirectoryAuthority::open_in_memory(
        log.pin(),
        KtFreshnessPolicy::strict(60, 2).unwrap(),
    )
    .unwrap();
    let authorized = authority
        .authorize(response, DirectoryAuthorizationPurpose::Pairing, 100)
        .unwrap();

    assert_eq!(authorized.claim(), &claim);
    assert_eq!(authorized.purpose(), DirectoryAuthorizationPurpose::Pairing);
    assert_eq!(
        authorized.provenance(),
        &KtAuthorityProvenance::LocalAcceptanceMock
    );
    assert_eq!(authorized.inclusion().siblings.len(), 0);
    assert_eq!(authorized.latest_map().siblings.len(), 256);
    assert_eq!(authorized.authorization_digest().len(), 64);
    authorized
        .require_signed_prekey_digest(
            &claim.key_material.signed_prekey_bundle_digest,
            claim.key_material.pairwise_prekey_version,
        )
        .unwrap();
    authorized
        .require_one_time_prekey_batch_digest(
            &claim.key_material.one_time_prekey_batch_digest,
            claim.key_material.pairwise_prekey_version,
        )
        .unwrap();
    authorized
        .require_mls_key_package_digest(
            &claim.key_material.mls_key_package_digest,
            claim.key_material.mls_key_package_version,
        )
        .unwrap();
}

#[test]
fn transcript_binding_is_stable_across_independently_verified_fresh_log_views() {
    let mut log = SecureMeshKtLog::new(SigningKey::generate(&mut OsRng));
    let claim = claim("device-a", 1, "active", 1);
    let first_response = append_response(&mut log, &claim, 100, None);
    let mut authority = SecureMeshDirectoryAuthority::open_in_memory(
        log.pin(),
        KtFreshnessPolicy::strict(60, 2).unwrap(),
    )
    .unwrap();
    let first = authority
        .authorize(
            first_response,
            DirectoryAuthorizationPurpose::PairwiseSessionBootstrap,
            100,
        )
        .unwrap();

    let index = log.append_current_map_checkpoint_for_test().unwrap();
    let second_response = current_response(&log, &claim, index, 101, Some(1));
    let second = authority
        .authorize(
            second_response,
            DirectoryAuthorizationPurpose::PairwiseSessionBootstrap,
            101,
        )
        .unwrap();

    assert_eq!(
        first.transcript_binding_digest(),
        second.transcript_binding_digest()
    );
    assert_ne!(first.authorization_digest(), second.authorization_digest());
}

#[test]
fn purpose_receipt_requires_target_label_at_current_sth_and_freshness_after_restart() {
    let path = state_path("purpose-receipt-current-label");
    let mut log = SecureMeshKtLog::new(SigningKey::generate(&mut OsRng));
    let pin = log.pin();
    let policy = KtFreshnessPolicy::strict(60, 2).unwrap();
    let target = claim("device-a", 1, "active", 1);
    let target_response = append_response(&mut log, &target, 100, None);
    let mut authority = SecureMeshDirectoryAuthority::open(&path, pin.clone(), policy).unwrap();
    authority
        .authorize(
            target_response,
            DirectoryAuthorizationPurpose::SelfMonitor,
            100,
        )
        .unwrap();
    let receipt = authority
        .require_current_authorization(
            &target.stable_label(),
            DirectoryAuthorizationPurpose::SelfMonitor,
            101,
        )
        .unwrap();
    assert_eq!(receipt.tree_size, 1);
    assert_eq!(receipt.purpose, "self-monitor");

    let other = claim("device-b", 1, "active", 2);
    let other_response = append_response(&mut log, &other, 102, Some(1));
    authority
        .authorize(other_response, DirectoryAuthorizationPurpose::Pairing, 102)
        .unwrap();
    let target_lag = authority
        .require_current_authorization(
            &target.stable_label(),
            DirectoryAuthorizationPurpose::SelfMonitor,
            102,
        )
        .unwrap_err();
    assert!(
        target_lag
            .to_string()
            .contains("purpose-bound authorization is missing")
    );
    drop(authority);

    let mut restored = SecureMeshDirectoryAuthority::open(&path, pin.clone(), policy).unwrap();
    assert!(
        restored
            .require_current_authorization(
                &target.stable_label(),
                DirectoryAuthorizationPurpose::SelfMonitor,
                102,
            )
            .unwrap_err()
            .to_string()
            .contains("purpose-bound authorization is missing")
    );
    let refreshed_target = current_response(&log, &target, 1, 103, None);
    restored
        .authorize(
            refreshed_target,
            DirectoryAuthorizationPurpose::SelfMonitor,
            103,
        )
        .unwrap();
    assert!(
        restored
            .require_current_authorization(
                &target.stable_label(),
                DirectoryAuthorizationPurpose::Pairing,
                103,
            )
            .unwrap_err()
            .to_string()
            .contains("purpose-bound authorization is missing")
    );
    let latest_receipt = restored
        .require_current_authorization(
            &target.stable_label(),
            DirectoryAuthorizationPurpose::SelfMonitor,
            160,
        )
        .unwrap();
    assert_eq!(latest_receipt.validated_at_epoch_seconds, 160);
    assert!(
        restored
            .require_current_authorization(
                &target.stable_label(),
                DirectoryAuthorizationPurpose::SelfMonitor,
                164,
            )
            .unwrap_err()
            .to_string()
            .contains("authenticated_sth_expired")
    );
    drop(restored);
    let mut clock_rolled_back = SecureMeshDirectoryAuthority::open(&path, pin, policy).unwrap();
    let blocked = clock_rolled_back
        .require_current_authorization(
            &target.stable_label(),
            DirectoryAuthorizationPurpose::SelfMonitor,
            101,
        )
        .unwrap_err()
        .to_string();
    assert!(blocked.contains("previously persisted"));
    assert!(clock_rolled_back.security_blocked().unwrap());
    let _ = std::fs::remove_file(path);
}

#[test]
fn typed_authority_rejects_commitment_tamper_and_map_log_mismatch() {
    let mut log = SecureMeshKtLog::new(SigningKey::generate(&mut OsRng));
    let original = claim("device-a", 1, "active", 1);
    let original_response = append_response(&mut log, &original, 100, None);

    let mut tampered_response = original_response.clone();
    tampered_response.claim.key_material.mls_key_package_digest = digest(99);
    let mut tamper_authority = SecureMeshDirectoryAuthority::open_in_memory(
        log.pin(),
        KtFreshnessPolicy::strict(60, 2).unwrap(),
    )
    .unwrap();
    let tamper = tamper_authority
        .authorize(
            tampered_response,
            DirectoryAuthorizationPurpose::MlsMemberAdd,
            100,
        )
        .unwrap_err();
    assert!(tamper.to_string().contains("sparse-map leaf is not latest"));

    let old_map = original_response.latest_map;
    let other = claim("device-b", 1, "active", 2);
    log.append_hashed_directory_leaf(
        &other.stable_label(),
        other.version(),
        other.revoked(),
        other.leaf_hash().unwrap(),
    )
    .unwrap();
    let mixed = UntrustedDirectoryResponse {
        claim: original.clone(),
        inclusion: log.inclusion_proof_at(1, 101).unwrap(),
        latest_map: old_map,
        consistency: None,
    };
    let mut mixed_authority = SecureMeshDirectoryAuthority::open_in_memory(
        log.pin(),
        KtFreshnessPolicy::strict(60, 2).unwrap(),
    )
    .unwrap();
    let mismatch = mixed_authority
        .authorize(mixed, DirectoryAuthorizationPurpose::Pairing, 101)
        .unwrap_err();
    assert!(
        mismatch
            .to_string()
            .contains("do not share one signed tree head")
    );
}

#[test]
fn request_mismatch_does_not_advance_or_poison_persistent_authority_state() {
    let path = state_path("request-mismatch-atomic");
    let mut log = SecureMeshKtLog::new(SigningKey::generate(&mut OsRng));
    let pin = log.pin();
    let policy = KtFreshnessPolicy::strict(120, 2).unwrap();
    let malicious = claim("device-a", 1, "active", 9);
    let expected = claim("device-a", 1, "active", 1);
    let malicious_response = append_response(&mut log, &malicious, 100, None);
    let mut authority = SecureMeshDirectoryAuthority::open(&path, pin.clone(), policy).unwrap();
    let error = authority
        .authorize_request(
            malicious_response,
            DirectoryAuthorizationRequest::for_exact_claim(
                DirectoryAuthorizationPurpose::SelfMonitor,
                &expected.endpoint.directory_scope_commitment,
                &expected,
            ),
            100,
        )
        .unwrap_err();
    assert!(error.to_string().contains("exact locally prepared claim"));
    assert!(authority.latest_checkpoint().unwrap().is_none());
    assert!(!authority.security_blocked().unwrap());
    drop(authority);

    let mut corrected = expected;
    corrected.directory_version = 2;
    corrected.endpoint.rotation_epoch = 2;
    let corrected_response = append_response(&mut log, &corrected, 101, None);
    let mut restored = SecureMeshDirectoryAuthority::open(&path, pin, policy).unwrap();
    restored
        .observe_gossip(
            &SecureMeshKtGossipPayload::from_sth(
                corrected_response.inclusion.signed_tree_head.clone(),
                corrected_response.consistency.clone(),
            ),
            101,
        )
        .unwrap();
    restored
        .authorize_request(
            corrected_response,
            DirectoryAuthorizationRequest::for_exact_claim(
                DirectoryAuthorizationPurpose::SelfMonitor,
                &corrected.endpoint.directory_scope_commitment,
                &corrected,
            ),
            101,
        )
        .unwrap();
    assert_eq!(restored.latest_checkpoint().unwrap().unwrap().tree_size, 2);
    assert!(!restored.security_blocked().unwrap());
    let _ = std::fs::remove_file(path);
}

#[test]
fn typed_request_rejects_foreign_scope_under_the_same_pinned_log() {
    let mut log = SecureMeshKtLog::new(SigningKey::generate(&mut OsRng));
    let published = claim("device-scope", 1, "active", 11);
    let response = append_response(&mut log, &published, 100, None);
    let foreign_scope = directory_scope_commitment("tenant-a", "account-a", "workspace-foreign");
    let request = DirectoryAuthorizationRequest {
        purpose: DirectoryAuthorizationPurpose::Pairing,
        expected_directory_scope_commitment: &foreign_scope,
        expected_identity: None,
        expected_directory_version: Some(published.directory_version),
        expected_signed_prekey: None,
        expected_one_time_prekey: None,
        expected_mls_key_package: None,
        expected_exact_claim: None,
    };
    let mut authority = SecureMeshDirectoryAuthority::open_in_memory(
        log.pin(),
        KtFreshnessPolicy::strict(60, 2).unwrap(),
    )
    .unwrap();

    let error = authority
        .authorize_request(response, request, 100)
        .unwrap_err()
        .to_string();
    assert!(error.contains("scope commitment mismatch"));
    assert!(authority.latest_checkpoint().unwrap().is_none());
    assert!(!authority.security_blocked().unwrap());
}

#[test]
fn full_subject_request_rejects_mls_key_package_substitution_before_state_advance() {
    let mut log = SecureMeshKtLog::new(SigningKey::generate(&mut OsRng));
    let published = claim("device-mls-package", 1, "active", 12);
    let response = append_response(&mut log, &published, 100, None);
    let substituted_digest = digest(99);
    let request = DirectoryAuthorizationRequest {
        purpose: DirectoryAuthorizationPurpose::SelfMonitor,
        expected_directory_scope_commitment: &published.endpoint.directory_scope_commitment,
        expected_identity: None,
        expected_directory_version: Some(published.directory_version),
        expected_signed_prekey: None,
        expected_one_time_prekey: None,
        expected_mls_key_package: Some((
            &substituted_digest,
            published.key_material.mls_key_package_version,
        )),
        expected_exact_claim: None,
    };
    let mut authority = SecureMeshDirectoryAuthority::open_in_memory(
        log.pin(),
        KtFreshnessPolicy::strict(60, 2).unwrap(),
    )
    .unwrap();

    let error = authority
        .authorize_request(response, request, 100)
        .unwrap_err()
        .to_string();
    assert!(error.contains("MLS KeyPackage commitment mismatch"));
    assert!(authority.latest_checkpoint().unwrap().is_none());
    assert!(!authority.security_blocked().unwrap());
}

#[test]
fn latest_version_requires_consistency_and_revoked_resurrection_blocks_after_restart() {
    let path = state_path("latest-revoke");
    let mut log = SecureMeshKtLog::new(SigningKey::generate(&mut OsRng));
    let pin = log.pin();
    let policy = KtFreshnessPolicy::strict(120, 2).unwrap();
    let first = claim("device-a", 1, "active", 1);
    let first_response = append_response(&mut log, &first, 100, None);
    let mut authority = SecureMeshDirectoryAuthority::open(&path, pin.clone(), policy).unwrap();
    authority
        .authorize(
            first_response,
            DirectoryAuthorizationPurpose::PairwiseSignedPrekey,
            100,
        )
        .unwrap();

    let second = claim("device-a", 2, "active", 2);
    let missing_consistency = append_response(&mut log, &second, 101, None);
    let error = authority
        .authorize(
            missing_consistency,
            DirectoryAuthorizationPurpose::MlsKeyPackage,
            101,
        )
        .unwrap_err();
    assert!(error.to_string().contains("consistency proof is required"));
    let second_response = current_response(&log, &second, 1, 101, Some(1));
    authority
        .authorize(
            second_response,
            DirectoryAuthorizationPurpose::MlsKeyPackage,
            101,
        )
        .unwrap();

    let revoked = claim("device-a", 3, "revoked", 3);
    let revoked_response = append_response(&mut log, &revoked, 102, Some(2));
    authority
        .authorize(
            revoked_response,
            DirectoryAuthorizationPurpose::Revocation,
            102,
        )
        .unwrap();

    let resurrected = claim("device-a", 4, "active", 4);
    let index = log
        .force_append_hashed_directory_leaf_for_adversarial_test(
            &resurrected.stable_label(),
            resurrected.version(),
            resurrected.revoked(),
            resurrected.leaf_hash().unwrap(),
        )
        .unwrap();
    let resurrection_response = current_response(&log, &resurrected, index, 103, Some(3));
    let error = authority
        .authorize(
            resurrection_response,
            DirectoryAuthorizationPurpose::Pairing,
            103,
        )
        .unwrap_err();
    assert!(error.to_string().contains("revoked identity resurrection"));
    drop(authority);

    let restored = SecureMeshDirectoryAuthority::open(&path, pin, policy).unwrap();
    assert!(restored.security_blocked().unwrap());
    let _ = std::fs::remove_file(path);
}

#[test]
fn authenticated_absence_is_typed_and_bound_to_requested_label() {
    let mut log = SecureMeshKtLog::new(SigningKey::generate(&mut OsRng));
    let scope = directory_scope_commitment("tenant-a", "account-a", "workspace-a");
    let missing = stable_directory_label(&scope, "missing");
    let map_root_index = log.append_empty_map_checkpoint_for_test().unwrap();
    let response = UntrustedDirectoryAbsenceResponse {
        stable_label: missing.clone(),
        map_root_inclusion: log.inclusion_proof_at(map_root_index, 100).unwrap(),
        absence_map: log.map_proof_at(&missing, 100).unwrap(),
        consistency: None,
    };
    let mut authority = SecureMeshDirectoryAuthority::open_in_memory(
        log.pin(),
        KtFreshnessPolicy::strict(60, 2).unwrap(),
    )
    .unwrap();
    authority
        .observe_gossip(
            &SecureMeshKtGossipPayload::from_sth(
                response.map_root_inclusion.signed_tree_head.clone(),
                response.consistency.clone(),
            ),
            100,
        )
        .unwrap();
    let authorized = authority.authorize_absence(response, 100).unwrap();
    assert_eq!(authorized.stable_label(), missing);
    assert_eq!(authorized.absence_map().siblings.len(), 256);
    assert_eq!(authorized.authorization_digest().len(), 64);
    assert_eq!(
        authorized.provenance(),
        &KtAuthorityProvenance::LocalAcceptanceMock
    );
}

#[test]
fn directory_authorization_requires_persisted_fresh_peer_gossip() {
    let path = state_path("fresh-peer-gossip-required");
    let mut log = SecureMeshKtLog::new(SigningKey::generate(&mut OsRng));
    let pin = log.pin();
    let policy = KtFreshnessPolicy::strict(3_600, 2).unwrap();
    let published = claim("device-gossip", 1, "active", 7);
    let response = append_response(&mut log, &published, 100, None);
    let request = DirectoryAuthorizationRequest::for_exact_claim(
        DirectoryAuthorizationPurpose::SelfMonitor,
        &published.endpoint.directory_scope_commitment,
        &published,
    );
    let mut authority = SecureMeshDirectoryAuthority::open(&path, pin.clone(), policy).unwrap();
    let missing = authority
        .authorize_request(response.clone(), request.clone(), 100)
        .unwrap_err();
    assert!(missing.to_string().contains("peer-gossip or witness"));
    authority
        .observe_gossip(
            &SecureMeshKtGossipPayload::from_sth(
                response.inclusion.signed_tree_head.clone(),
                response.consistency.clone(),
            ),
            100,
        )
        .unwrap();
    authority.authorize_request(response, request, 100).unwrap();
    drop(authority);

    let mut restored = SecureMeshDirectoryAuthority::open(&path, pin, policy).unwrap();
    restored
        .require_current_authorization(
            &published.stable_label(),
            DirectoryAuthorizationPurpose::SelfMonitor,
            101,
        )
        .unwrap();
    let stale = restored
        .require_current_authorization(
            &published.stable_label(),
            DirectoryAuthorizationPurpose::SelfMonitor,
            1_001,
        )
        .unwrap_err();
    assert!(
        stale
            .to_string()
            .contains("peer-gossip or witness observation is stale")
    );
    let _ = std::fs::remove_file(path);
}

#[test]
fn independently_consistent_clients_persist_split_view_on_cross_gossip() {
    let signing_key = SigningKey::generate(&mut OsRng);
    let mut first_log =
        SecureMeshKtLog::with_identity(signing_key.clone(), "split-gossip-log", "split-gossip-key");
    let mut second_log =
        SecureMeshKtLog::with_identity(signing_key, "split-gossip-log", "split-gossip-key");
    let first_claim = claim("device-split-a", 1, "active", 1);
    let second_claim = claim("device-split-b", 1, "active", 2);
    let first_response = append_response(&mut first_log, &first_claim, 100, None);
    let second_response = append_response(&mut second_log, &second_claim, 100, None);
    let policy = KtFreshnessPolicy::strict(3_600, 2).unwrap();
    let mut first_client =
        SecureMeshDirectoryAuthority::open_in_memory(first_log.pin(), policy).unwrap();
    let mut second_client =
        SecureMeshDirectoryAuthority::open_in_memory(second_log.pin(), policy).unwrap();
    first_client
        .authorize(
            first_response,
            DirectoryAuthorizationPurpose::SelfMonitor,
            100,
        )
        .unwrap();
    second_client
        .authorize(
            second_response.clone(),
            DirectoryAuthorizationPurpose::SelfMonitor,
            100,
        )
        .unwrap();

    let split = first_client
        .observe_gossip(
            &SecureMeshKtGossipPayload::from_sth(second_response.inclusion.signed_tree_head, None),
            101,
        )
        .unwrap_err();
    assert!(split.to_string().contains("same-size split view"));
    assert!(first_client.security_blocked().unwrap());
}

#[test]
fn directory_active_state_cannot_substitute_for_local_device_trust() {
    let mut log = SecureMeshKtLog::new(SigningKey::generate(&mut OsRng));
    let attacker_claim = claim("device-a", 1, "active", 9);
    let response = append_response(&mut log, &attacker_claim, 100, None);
    let mut authority = SecureMeshDirectoryAuthority::open_in_memory(
        log.pin(),
        KtFreshnessPolicy::strict(60, 2).unwrap(),
    )
    .unwrap();
    let directory_leaf = authority
        .authorize(response, DirectoryAuthorizationPurpose::Pairing, 100)
        .unwrap();
    let locally_trusted_identity =
        DeviceTrustPublicIdentity::new("device-a", [1u8; 32], [2u8; 32], 1).unwrap();
    let error = directory_leaf
        .require_device_identity(&locally_trusted_identity)
        .unwrap_err();
    assert!(error.to_string().contains("identity commitment mismatch"));
}

#[test]
fn directory_versions_reject_cross_language_unsafe_json_integers() {
    let mut unsafe_claim = claim("device-a", 1, "active", 1);
    unsafe_claim.directory_version = KT_JSON_SAFE_INTEGER_MAX + 1;
    assert!(
        unsafe_claim
            .leaf_hash()
            .unwrap_err()
            .to_string()
            .contains("cross-language safe range")
    );
    unsafe_claim.directory_version = 1;
    unsafe_claim.key_material.mls_key_package_version = KT_JSON_SAFE_INTEGER_MAX + 1;
    assert!(
        unsafe_claim
            .leaf_hash()
            .unwrap_err()
            .to_string()
            .contains("cross-language safe range")
    );
}

#[test]
fn component_versions_and_same_version_digests_are_monotonic_across_restart() {
    let first = claim("device-a", 1, "active", 1);

    let mut pairwise_rollback = claim("device-a", 2, "active", 2);
    pairwise_rollback.key_material.pairwise_prekey_version = 0;
    assert_component_violation(
        first.clone(),
        pairwise_rollback,
        "Pairwise prekey version rollback",
        "pairwise-rollback",
    );

    let mut pairwise_split = claim("device-a", 2, "active", 2);
    pairwise_split.key_material.pairwise_prekey_version = 1;
    assert_component_violation(
        first.clone(),
        pairwise_split,
        "Pairwise prekey same-version split view",
        "pairwise-split",
    );

    let mut mls_rollback = claim("device-a", 2, "active", 2);
    mls_rollback.key_material.mls_key_package_version = 0;
    assert_component_violation(
        first.clone(),
        mls_rollback,
        "MLS KeyPackage version rollback",
        "mls-rollback",
    );

    let mut mls_split = claim("device-a", 2, "active", 2);
    mls_split.key_material.mls_key_package_version = 1;
    assert_component_violation(
        first,
        mls_split,
        "MLS KeyPackage same-version split view",
        "mls-split",
    );
}

#[test]
fn identity_rotation_is_monotonic_and_prekey_only_updates_keep_the_same_epoch() {
    let first = claim("device-a", 1, "active", 1);

    let mut same_epoch_key_change = claim("device-a", 2, "active", 2);
    same_epoch_key_change.endpoint.rotation_epoch = 1;
    assert_component_violation(
        first.clone(),
        same_epoch_key_change,
        "identity key changed without strict rotation epoch advance",
        "identity-same-epoch-key-change",
    );

    let mut unchanged_key_epoch_change = first.clone();
    unchanged_key_epoch_change.directory_version = 2;
    unchanged_key_epoch_change.endpoint.rotation_epoch = 2;
    unchanged_key_epoch_change
        .key_material
        .pairwise_prekey_version = 2;
    unchanged_key_epoch_change
        .key_material
        .signed_prekey_bundle_digest = digest(7);
    unchanged_key_epoch_change
        .key_material
        .one_time_prekey_batch_digest = digest(8);
    assert_component_violation(
        first.clone(),
        unchanged_key_epoch_change,
        "identity epoch changed without identity material change",
        "identity-epoch-without-key-change",
    );

    let mut prekey_only = first.clone();
    prekey_only.directory_version = 2;
    prekey_only.key_material.pairwise_prekey_version = 2;
    prekey_only.key_material.signed_prekey_bundle_digest = digest(7);
    prekey_only.key_material.one_time_prekey_batch_digest = digest(8);
    let path = state_path("identity-prekey-only");
    let mut log = SecureMeshKtLog::new(SigningKey::generate(&mut OsRng));
    let pin = log.pin();
    let policy = KtFreshnessPolicy::strict(120, 2).unwrap();
    let first_response = append_response(&mut log, &first, 100, None);
    let mut authority = SecureMeshDirectoryAuthority::open(&path, pin.clone(), policy).unwrap();
    authority
        .authorize(
            first_response,
            DirectoryAuthorizationPurpose::SelfMonitor,
            100,
        )
        .unwrap();
    let next_response = append_response(&mut log, &prekey_only, 101, Some(1));
    authority
        .authorize(
            next_response,
            DirectoryAuthorizationPurpose::PairwiseSignedPrekey,
            101,
        )
        .unwrap();
    drop(authority);
    let restored = SecureMeshDirectoryAuthority::open(&path, pin, policy).unwrap();
    assert!(!restored.security_blocked().unwrap());
    let _ = std::fs::remove_file(path);
}

#[test]
fn previously_present_label_cannot_become_absent_across_restart() {
    let path = state_path("present-to-absent");
    let mut log = SecureMeshKtLog::new(SigningKey::generate(&mut OsRng));
    let pin = log.pin();
    let policy = KtFreshnessPolicy::strict(120, 2).unwrap();
    let present = claim("device-a", 1, "active", 1);
    let present_response = append_response(&mut log, &present, 100, None);
    let mut authority = SecureMeshDirectoryAuthority::open(&path, pin.clone(), policy).unwrap();
    authority
        .authorize(
            present_response,
            DirectoryAuthorizationPurpose::SelfMonitor,
            100,
        )
        .unwrap();

    let map_root_index = log
        .force_remove_map_label_for_adversarial_test(&present.stable_label())
        .unwrap();
    let absence = UntrustedDirectoryAbsenceResponse {
        stable_label: present.stable_label(),
        map_root_inclusion: log.inclusion_proof_at(map_root_index, 101).unwrap(),
        absence_map: log.map_proof_at(&present.stable_label(), 101).unwrap(),
        consistency: Some(log.consistency_proof_at(1, 101).unwrap()),
    };
    authority
        .observe_gossip(
            &SecureMeshKtGossipPayload::from_sth(
                absence.map_root_inclusion.signed_tree_head.clone(),
                absence.consistency.clone(),
            ),
            101,
        )
        .unwrap();
    let error = authority.authorize_absence(absence, 101).unwrap_err();
    assert!(error.to_string().contains("previously present"));
    drop(authority);
    let restored = SecureMeshDirectoryAuthority::open(&path, pin, policy).unwrap();
    assert!(restored.security_blocked().unwrap());
    let _ = std::fs::remove_file(path);
}

fn append_response(
    log: &mut SecureMeshKtLog,
    claim: &SecureMeshDirectoryLeafClaim,
    issued_at: u64,
    first_size: Option<u64>,
) -> UntrustedDirectoryResponse {
    let index = log
        .append_hashed_directory_leaf(
            &claim.stable_label(),
            claim.version(),
            claim.revoked(),
            claim.leaf_hash().unwrap(),
        )
        .unwrap();
    current_response(log, claim, index, issued_at, first_size)
}

fn assert_component_violation(
    first: SecureMeshDirectoryLeafClaim,
    second: SecureMeshDirectoryLeafClaim,
    expected: &str,
    label: &str,
) {
    let path = state_path(label);
    let mut log = SecureMeshKtLog::new(SigningKey::generate(&mut OsRng));
    let pin = log.pin();
    let policy = KtFreshnessPolicy::strict(120, 2).unwrap();
    let first_response = append_response(&mut log, &first, 100, None);
    let mut authority = SecureMeshDirectoryAuthority::open(&path, pin.clone(), policy).unwrap();
    authority
        .authorize(
            first_response,
            DirectoryAuthorizationPurpose::SelfMonitor,
            100,
        )
        .unwrap();
    let second_response = append_response(&mut log, &second, 101, Some(1));
    let error = authority
        .authorize(
            second_response,
            DirectoryAuthorizationPurpose::SelfMonitor,
            101,
        )
        .unwrap_err();
    assert!(error.to_string().contains(expected), "{error}");
    drop(authority);
    let restored = SecureMeshDirectoryAuthority::open(&path, pin, policy).unwrap();
    assert!(restored.security_blocked().unwrap());
    let _ = std::fs::remove_file(path);
}

fn current_response(
    log: &SecureMeshKtLog,
    claim: &SecureMeshDirectoryLeafClaim,
    index: u64,
    issued_at: u64,
    first_size: Option<u64>,
) -> UntrustedDirectoryResponse {
    UntrustedDirectoryResponse {
        claim: claim.clone(),
        inclusion: log.inclusion_proof_at(index, issued_at).unwrap(),
        latest_map: log.map_proof_at(&claim.stable_label(), issued_at).unwrap(),
        consistency: first_size.map(|first| log.consistency_proof_at(first, issued_at).unwrap()),
    }
}

fn claim(
    endpoint_id: &str,
    version: u64,
    directory_state: &str,
    seed: u8,
) -> SecureMeshDirectoryLeafClaim {
    SecureMeshDirectoryLeafClaim {
        endpoint: SecureMeshTransparencyLeafBody {
            directory_scope_commitment: directory_scope_commitment(
                "tenant-a",
                "account-a",
                "workspace-a",
            ),
            endpoint_id: endpoint_id.to_string(),
            endpoint_kind: "test".to_string(),
            identity_public_key: format!("{endpoint_id}-identity-{seed}"),
            signing_public_key: format!("{endpoint_id}-signing-{seed}"),
            fingerprint: format!("{endpoint_id}-fingerprint-{seed}"),
            rotation_epoch: version,
            directory_state: directory_state.to_string(),
            updated_at: "2026-07-12T00:00:00Z".to_string(),
        },
        key_material: SecureMeshDirectoryKeyMaterialCommitment {
            signed_prekey_bundle_digest: digest(seed),
            one_time_prekey_batch_digest: digest(seed.wrapping_add(1)),
            pairwise_prekey_version: version,
            mls_key_package_digest: digest(seed.wrapping_add(2)),
            mls_key_package_version: version,
        },
        directory_version: version,
    }
}

fn digest(seed: u8) -> String {
    hex_encode(&[seed; 32])
}

fn state_path(label: &str) -> std::path::PathBuf {
    std::env::temp_dir().join(format!(
        "lico-directory-{label}-{}-{}.sqlite3",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ))
}
