use ed25519_dalek::SigningKey;
use rand_core::OsRng;
use serde_json::json;

use super::super::{SECURE_MESH_LIFECYCLE_CONTENT_TYPE, mls::prepare_lifecycle_service_action};
use crate::core::secure_mesh_crypto::SecureMeshPayloadKind;

#[test]
fn lifecycle_mls_preparation_is_redacted_and_service_action_typed() {
    let plaintext = prepare_lifecycle_service_action(&json!({
        "actionKind": "ack_purge",
        "fileTransferId": "mls-private-transfer",
        "acknowledged": true,
        "transferComplete": true,
        "body": "mls-private-plaintext"
    }))
    .unwrap();

    assert_eq!(plaintext.kind, SecureMeshPayloadKind::ServiceAction);
    assert_eq!(
        plaintext.content_type.as_deref(),
        Some(SECURE_MESH_LIFECYCLE_CONTENT_TYPE)
    );
    let encoded = String::from_utf8(plaintext.body).unwrap();
    assert!(!encoded.contains("mls-private-transfer"));
    assert!(!encoded.contains("mls-private-plaintext"));
}

#[test]
fn lifecycle_mls_seal_fails_closed_before_capability_negotiation() {
    use super::super::seal_lifecycle_service_action_mls;
    use crate::core::secure_mesh_crypto::SecureMeshContentContext;
    use crate::core::secure_mesh_mls_product::{
        create_product_group, participant_from_device_identity,
    };
    use crate::core::secure_mesh_trust::{DeviceTrustPublicIdentity, DeviceTrustState};
    use std::collections::BTreeMap;

    let identity_key = SigningKey::generate(&mut OsRng);
    let signing_key = SigningKey::generate(&mut OsRng);
    let identity = DeviceTrustPublicIdentity::new(
        "desktop_gui:lifecycle-owner",
        identity_key.verifying_key().to_bytes(),
        signing_key.verifying_key().to_bytes(),
        1,
    )
    .unwrap();
    let participant = participant_from_device_identity(&identity, &signing_key).unwrap();
    let mut group = create_product_group(
        &participant,
        &identity,
        &DeviceTrustState::Verified,
        b"lifecycle-mls-negotiation",
    )
    .unwrap();
    let roster = BTreeMap::from([(identity.endpoint_id.clone(), identity.clone())]);
    let context = SecureMeshContentContext::new(
        "env-lifecycle-mls",
        "msg-lifecycle-mls",
        "mailbox-lifecycle-mls",
        &identity.endpoint_id,
        &identity.endpoint_id,
        "mls:lifecycle-pending",
        "2026-01-01T00:00:00Z",
        "2026-01-01T00:01:00Z",
    );

    let error = seal_lifecycle_service_action_mls(
        &mut group,
        &participant,
        &identity,
        &DeviceTrustState::Verified,
        &roster,
        &context,
        &json!({"actionKind": "screenshot_detected"}),
    )
    .unwrap_err();
    assert!(error.to_string().contains("capability"));
}
