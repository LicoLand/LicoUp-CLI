use base64::Engine as _;
use ed25519_dalek::SigningKey;
use rand_core::OsRng;
use serde_json::json;
use sha2::{Digest, Sha256};
use time::{Duration, OffsetDateTime, format_description::well_known::Rfc3339};

use super::super::{
    open_lifecycle_service_action_pairwise, reject_plaintext_lifecycle_service_action_transport,
    seal_lifecycle_service_action_pairwise,
};
use crate::core::secure_mesh_crypto::SecureMeshContentContext;
use crate::core::secure_mesh_pairwise::{SecureMeshPairwisePrivateKey, SecureMeshPairwiseSession};
use crate::core::secure_mesh_pqxdh::SecureMeshMlKem1024PreKeySeed;
use crate::core::secure_mesh_prekey::{
    SecureMeshPairwisePreKeyBundle, SecureMeshPreKeyKind, SecureMeshPreKeyValidationPolicy,
    authorize_test_pairwise_prekey_bundle, sign_prekey_record,
};
use crate::core::secure_mesh_trust::{DeviceTrustPublicIdentity, DeviceTrustState};

#[test]
fn secure_mesh_lifecycle_service_actions_seal_only_inside_pairwise_envelopes() {
    let alice_identity_secret = SecureMeshPairwisePrivateKey::generate();
    let alice_signing = SigningKey::generate(&mut OsRng);
    let alice_identity = DeviceTrustPublicIdentity::new(
        "desktop_gui:alice",
        alice_identity_secret.public_key(),
        alice_signing.verifying_key().to_bytes(),
        1,
    )
    .unwrap();
    let bob_identity_secret = SecureMeshPairwisePrivateKey::generate();
    let bob_signing = SigningKey::generate(&mut OsRng);
    let bob_identity = DeviceTrustPublicIdentity::new(
        "mobile:bob",
        bob_identity_secret.public_key(),
        bob_signing.verifying_key().to_bytes(),
        1,
    )
    .unwrap();
    let signed_secret = SecureMeshPairwisePrivateKey::generate();
    let one_time_secret = SecureMeshPairwisePrivateKey::generate();
    let one_time_mlkem1024_prekey_seed = SecureMeshMlKem1024PreKeySeed::generate();
    let bundle = SecureMeshPairwisePreKeyBundle {
        endpoint_identity: bob_identity.clone(),
        trust_state: DeviceTrustState::Verified,
        signed_prekey: sign_prekey_record(
            &bob_signing,
            &bob_identity,
            SecureMeshPreKeyKind::SignedPreKey,
            "spk-life-1",
            signed_secret.public_key(),
            "2026-07-11T00:00:00Z",
            "2026-08-11T00:00:00Z",
        )
        .unwrap(),
        one_time_prekey: Some(
            sign_prekey_record(
                &bob_signing,
                &bob_identity,
                SecureMeshPreKeyKind::OneTimePreKey,
                "otpk-life-1",
                one_time_secret.public_key(),
                "2026-07-11T00:00:00Z",
                "2026-08-11T00:00:00Z",
            )
            .unwrap(),
        ),
        one_time_mlkem1024_prekey: sign_prekey_record(
            &bob_signing,
            &bob_identity,
            SecureMeshPreKeyKind::OneTimeMlKem1024PreKey,
            "pqotpk-life-1",
            one_time_mlkem1024_prekey_seed.public_key(),
            "2026-07-11T00:00:00Z",
            "2026-08-11T00:00:00Z",
        )
        .unwrap(),
        prekey_publication_version: 1,
    };
    let directory_authorization = authorize_test_pairwise_prekey_bundle(&bundle);
    let now = OffsetDateTime::parse(
        "2026-07-11T00:00:01Z",
        &time::format_description::well_known::Rfc3339,
    )
    .unwrap();
    let (mut alice_session, intro) = SecureMeshPairwiseSession::initiate(
        &alice_identity,
        &alice_identity_secret,
        &alice_signing,
        &bundle,
        &directory_authorization,
        &SecureMeshPreKeyValidationPolicy::default(),
        &crate::core::secure_mesh_pairwise::secure_mesh_pairwise_test_capability_evaluation()
            .unwrap(),
        now,
    )
    .unwrap();
    let (mut bob_session, accepted) = SecureMeshPairwiseSession::accept(
        &bob_identity,
        &bob_identity_secret,
        &bob_signing,
        &alice_identity,
        &signed_secret,
        Some(&one_time_secret),
        &one_time_mlkem1024_prekey_seed,
        &intro,
        &crate::core::secure_mesh_pairwise::secure_mesh_pairwise_test_capability_evaluation()
            .unwrap(),
        now,
        &mut crate::core::secure_mesh_session_negotiation::CapabilityProofReplayGuard::default(),
    )
    .unwrap();
    let finished = alice_session
        .complete_initiator_handshake(
            &alice_identity,
            &bob_identity,
            &accepted,
            now,
            &mut crate::core::secure_mesh_session_negotiation::CapabilityProofReplayGuard::default(
            ),
        )
        .unwrap();
    bob_session.complete_responder_handshake(&finished).unwrap();

    let params = json!({
        "actionKind": "ack_purge",
        "endpointId": "lifecycle-private-endpoint",
        "fileTransferId": "lifecycle-private-file",
        "acknowledged": true,
        "transferComplete": true,
        "body": "lifecycle-plaintext-canary"
    });
    let plaintext_forbidden =
        reject_plaintext_lifecycle_service_action_transport(&params).unwrap_err();
    assert!(
        plaintext_forbidden
            .to_string()
            .contains("plaintext transport is forbidden")
    );

    let created_at = OffsetDateTime::now_utc();
    let expires_at = created_at + Duration::minutes(10);
    let context = SecureMeshContentContext::new(
        base64::engine::general_purpose::URL_SAFE_NO_PAD
            .encode(&Sha256::digest(b"env-life-1")[..24]),
        "msg-life-1",
        base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(Sha256::digest(b"mailbox-life-1")),
        "desktop_gui:alice",
        "mobile:bob",
        alice_session.session_id.clone(),
        created_at.format(&Rfc3339).unwrap(),
        expires_at.format(&Rfc3339).unwrap(),
    );
    let envelope =
        seal_lifecycle_service_action_pairwise(&mut alice_session, &context, &params).unwrap();
    for forbidden in [
        "lifecycle-private-endpoint",
        "lifecycle-private-file",
        "lifecycle-plaintext-canary",
    ] {
        assert!(
            !envelope.ciphertext().contains(forbidden),
            "lifecycle envelope carrier leaked {forbidden}"
        );
    }
    let (_opened, value) =
        open_lifecycle_service_action_pairwise(&mut bob_session, &context, &envelope).unwrap();
    assert_eq!(value["actionKind"], "ack_purge");
    assert_eq!(value["requiresPairwiseOrMlsEnvelope"], true);
}
