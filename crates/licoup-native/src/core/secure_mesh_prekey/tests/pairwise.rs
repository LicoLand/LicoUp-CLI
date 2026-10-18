use super::super::pairwise::{prekey_signature_payload, validate_pairwise_prekey_bundle_crypto};
use super::super::validation::hex_sha256;
use super::super::{
    SecureMeshPairwisePreKeyBundle, SecureMeshPreKeyKind, SecureMeshPreKeyValidationPolicy,
    sign_prekey_record, validate_pairwise_prekey_bundle,
};
use super::support::{
    CREATED_AT, EXPIRES_AT, authorize_test_pairwise_prekey_bundle_with_purpose,
    deterministic_identity_fixture, identity_fixture, mlkem_prekey_fixture, now,
    one_time_prekey_fixture, signed_prekey_fixture,
};
use crate::core::secure_mesh_directory::DirectoryAuthorizationPurpose;
use crate::core::secure_mesh_trust::DeviceTrustState;

#[test]
fn bundle_verifies_signed_curve_and_one_time_pq_prekeys() {
    let (signing_key, identity) = identity_fixture("desktop:alice");
    let bundle = SecureMeshPairwisePreKeyBundle {
        endpoint_identity: identity.clone(),
        trust_state: DeviceTrustState::Verified,
        signed_prekey: signed_prekey_fixture(&signing_key, &identity, "spk-1"),
        one_time_prekey: Some(one_time_prekey_fixture(&signing_key, &identity, "otpk-1")),
        one_time_mlkem1024_prekey: mlkem_prekey_fixture(&signing_key, &identity, "pqotpk-1", 1),
        prekey_publication_version: 1,
    };
    let validation = validate_pairwise_prekey_bundle_crypto(
        &bundle,
        &SecureMeshPreKeyValidationPolicy::default(),
        now(),
    )
    .unwrap();
    assert_eq!(validation.endpoint_id, "desktop:alice");
    assert_eq!(validation.one_time_prekey_id.as_deref(), Some("otpk-1"));
    assert_eq!(validation.one_time_mlkem1024_prekey_id, "pqotpk-1");
}

#[test]
fn session_rejects_cross_purpose_directory_authorization() {
    let (signing_key, identity) = identity_fixture("desktop:alice-purpose");
    let bundle = SecureMeshPairwisePreKeyBundle {
        endpoint_identity: identity.clone(),
        trust_state: DeviceTrustState::Verified,
        signed_prekey: signed_prekey_fixture(&signing_key, &identity, "spk-purpose"),
        one_time_prekey: Some(one_time_prekey_fixture(
            &signing_key,
            &identity,
            "otpk-purpose",
        )),
        one_time_mlkem1024_prekey: mlkem_prekey_fixture(
            &signing_key,
            &identity,
            "pqotpk-purpose",
            2,
        ),
        prekey_publication_version: 1,
    };
    let wrong_purpose = authorize_test_pairwise_prekey_bundle_with_purpose(
        &bundle,
        DirectoryAuthorizationPurpose::Pairing,
    );
    let error = validate_pairwise_prekey_bundle(
        &bundle,
        &wrong_purpose,
        &SecureMeshPreKeyValidationPolicy::default(),
        now(),
    )
    .unwrap_err();
    assert!(
        error
            .to_string()
            .contains("exact directory authorization purpose")
    );
}

#[test]
fn bundle_rejects_tampered_signed_prekey_signature() {
    let (signing_key, identity) = identity_fixture("desktop:alice");
    let mut signed_prekey = signed_prekey_fixture(&signing_key, &identity, "spk-1");
    signed_prekey.public_key[0] ^= 0x01;
    let bundle = SecureMeshPairwisePreKeyBundle {
        endpoint_identity: identity.clone(),
        trust_state: DeviceTrustState::Verified,
        signed_prekey,
        one_time_prekey: None,
        one_time_mlkem1024_prekey: mlkem_prekey_fixture(&signing_key, &identity, "pqotpk-1", 3),
        prekey_publication_version: 1,
    };
    let error = validate_pairwise_prekey_bundle_crypto(
        &bundle,
        &SecureMeshPreKeyValidationPolicy {
            require_verified_device: true,
            require_one_time_prekey: false,
        },
        now(),
    )
    .unwrap_err();
    assert!(error.to_string().contains("signature verification failed"));
}

#[test]
fn bundle_requires_active_trust_and_rejects_cross_signed_without_revocation_state() {
    let (signing_key, identity) = identity_fixture("desktop:alice");
    for (trust_state, expected) in [
        (DeviceTrustState::KeyChanged, "identity changed"),
        (
            DeviceTrustState::CrossSigned,
            "durable epoch and revocation",
        ),
    ] {
        let bundle = SecureMeshPairwisePreKeyBundle {
            endpoint_identity: identity.clone(),
            trust_state,
            signed_prekey: signed_prekey_fixture(&signing_key, &identity, "spk-trust"),
            one_time_prekey: Some(one_time_prekey_fixture(
                &signing_key,
                &identity,
                "otpk-trust",
            )),
            one_time_mlkem1024_prekey: mlkem_prekey_fixture(
                &signing_key,
                &identity,
                "pqotpk-trust",
                4,
            ),
            prekey_publication_version: 1,
        };
        let error = validate_pairwise_prekey_bundle_crypto(
            &bundle,
            &SecureMeshPreKeyValidationPolicy::default(),
            now(),
        )
        .unwrap_err();
        assert!(error.to_string().contains(expected));
    }
}

#[test]
fn bundle_rejects_missing_required_one_time_prekey() {
    let (signing_key, identity) = identity_fixture("desktop:alice-required");
    let bundle = SecureMeshPairwisePreKeyBundle {
        endpoint_identity: identity.clone(),
        trust_state: DeviceTrustState::Verified,
        signed_prekey: signed_prekey_fixture(&signing_key, &identity, "spk-required"),
        one_time_prekey: None,
        one_time_mlkem1024_prekey: mlkem_prekey_fixture(
            &signing_key,
            &identity,
            "pqotpk-required",
            5,
        ),
        prekey_publication_version: 1,
    };
    let error = validate_pairwise_prekey_bundle_crypto(
        &bundle,
        &SecureMeshPreKeyValidationPolicy::default(),
        now(),
    )
    .unwrap_err();
    assert!(error.to_string().contains("one-time prekey is required"));
}

#[test]
fn bundle_rejects_expired_and_future_signed_prekeys() {
    let (signing_key, identity) = identity_fixture("desktop:alice-time");
    for (created_at, expires_at, expected, seed) in [
        (
            "2025-12-01T00:00:00Z",
            "2025-12-02T00:00:00Z",
            "prekey is expired",
            6,
        ),
        (
            "2026-01-01T01:00:01Z",
            "2026-01-02T00:00:00Z",
            "createdAt is too far in the future",
            7,
        ),
    ] {
        let signed_prekey = sign_prekey_record(
            &signing_key,
            &identity,
            SecureMeshPreKeyKind::SignedPreKey,
            "spk-time",
            vec![7; 32],
            created_at,
            expires_at,
        )
        .unwrap();
        let bundle = SecureMeshPairwisePreKeyBundle {
            endpoint_identity: identity.clone(),
            trust_state: DeviceTrustState::Verified,
            signed_prekey,
            one_time_prekey: None,
            one_time_mlkem1024_prekey: mlkem_prekey_fixture(
                &signing_key,
                &identity,
                "pqotpk-time",
                seed,
            ),
            prekey_publication_version: 1,
        };
        let error = validate_pairwise_prekey_bundle_crypto(
            &bundle,
            &SecureMeshPreKeyValidationPolicy {
                require_verified_device: true,
                require_one_time_prekey: false,
            },
            now(),
        )
        .unwrap_err();
        assert!(error.to_string().contains(expected));
    }
}

#[test]
fn pairwise_signature_payload_and_signature_match_the_stable_vector() {
    let (signing_key, identity) = deterministic_identity_fixture("desktop:vector");
    let record = sign_prekey_record(
        &signing_key,
        &identity,
        SecureMeshPreKeyKind::SignedPreKey,
        "spk-vector-v1",
        vec![0x5a; 32],
        CREATED_AT,
        EXPIRES_AT,
    )
    .unwrap();
    let payload =
        prekey_signature_payload(&identity, SecureMeshPreKeyKind::SignedPreKey, &record).unwrap();
    assert_eq!(
        hex_sha256(&payload),
        "ba74fb2e1dc52166cad3b43010b84ef762ba3f4ae2684430b3fc10698da4f021"
    );
    assert_eq!(
        record.signature,
        "g5TtKPbnPBDHGJGVTzN2LxUIKA0nX7NTL1fvtBeOnk5OKGzbJBuyA1gTOvbJS1HOqvq4Dsohd40aOH1HZMUWAA"
    );
}
