use crate::domain::mobile_relay::key_transparency::config::CONFIG_SCHEMA_VERSION;
use crate::domain::mobile_relay::key_transparency::projection::authority_challenge_response;
use serde_json::json;

#[test]
fn challenge_projection_exposes_only_confirmation_metadata() {
    let challenge = json!({
        "challengeId": "challenge-test",
        "proposalDigest": "digest-test",
        "expiresAtEpochSeconds": 42,
        "requiresSecurityReset": true,
        "confirmationSecret": "must-not-escape"
    });
    let response = authority_challenge_response(&challenge, CONFIG_SCHEMA_VERSION);
    assert_eq!(response["status"], json!("confirmation_required"));
    assert_eq!(response["requiresUserPresence"], json!(true));
    assert!(response.get("privateKeyMaterial").is_none());
    assert!(response.get("confirmationSecret").is_none());
}
