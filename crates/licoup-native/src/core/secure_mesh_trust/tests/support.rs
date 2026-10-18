use super::super::DeviceTrustPublicIdentity;
use base64::{Engine as _, engine::general_purpose};
use ed25519_dalek::{SigningKey, VerifyingKey};
use rand_core::OsRng;
use serde_json::{Value, json};

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

pub(super) fn identity_json(identity: &DeviceTrustPublicIdentity) -> Value {
    json!({
        "endpointId": identity.endpoint_id,
        "identityPublicKey": general_purpose::URL_SAFE_NO_PAD.encode(identity.identity_public_key),
        "signingPublicKey": general_purpose::URL_SAFE_NO_PAD.encode(identity.signing_public_key),
        "rotationEpoch": identity.rotation_epoch
    })
}
