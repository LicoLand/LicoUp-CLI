use serde_json::json;

use super::super::{
    SECURE_MESH_LIFECYCLE_CONTENT_TYPE, evaluate_service_action_json,
    projection::{decode_protected_projection, protected_plaintext},
};
use crate::core::secure_mesh_crypto::{OpenedSecureMeshPayload, SecureMeshPayloadKind};

#[test]
fn lifecycle_projection_is_a_typed_protected_service_action() {
    let projected = evaluate_service_action_json(&json!({
        "actionKind": "screenshot_detected",
        "messageId": "projection-private-message"
    }))
    .unwrap();
    let plaintext = protected_plaintext(&projected).unwrap();

    assert_eq!(plaintext.kind, SecureMeshPayloadKind::ServiceAction);
    assert_eq!(
        plaintext.content_type.as_deref(),
        Some(SECURE_MESH_LIFECYCLE_CONTENT_TYPE)
    );
    assert!(!String::from_utf8_lossy(&plaintext.body).contains("projection-private-message"));

    let opened = OpenedSecureMeshPayload {
        kind: plaintext.kind,
        body: plaintext.body,
        content_type: plaintext.content_type,
        created_at: "2026-01-01T00:00:00Z".to_string(),
        expires_at: "2026-01-01T00:01:00Z".to_string(),
    };
    let decoded = decode_protected_projection(&opened, "test").unwrap();
    assert_eq!(decoded, projected);
}

#[test]
fn lifecycle_projection_rejects_an_unprotected_shape() {
    let error = protected_plaintext(&json!({
        "requiresPairwiseOrMlsEnvelope": false,
        "serverVisiblePlaintextAllowed": false
    }))
    .unwrap_err();
    assert!(error.to_string().contains("protected envelope"));
}
