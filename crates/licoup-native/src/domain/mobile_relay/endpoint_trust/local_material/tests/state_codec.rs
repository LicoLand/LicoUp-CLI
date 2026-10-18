use super::super::state_codec::{hex_encode_bytes, local_endpoint_state};
use crate::domain::mobile_relay::secret_custody::RuntimeSecretMaterial;
use serde_json::json;

#[test]
fn state_codec_fails_closed_when_required_private_material_is_absent() {
    let material = RuntimeSecretMaterial::new();
    let error = local_endpoint_state(
        &json!({
            "mobileRelayE2ee": {
                "endpointId": "endpoint",
                "endpointKind": "desktop"
            }
        }),
        &material,
    )
    .err()
    .expect("missing private material must be rejected");

    assert!(
        error
            .to_string()
            .contains("existing identity metadata is incomplete")
    );
}

#[test]
fn hex_codec_is_stable_and_lowercase() {
    assert_eq!(hex_encode_bytes(&[0, 1, 15, 16, 255]), "00010f10ff");
}
