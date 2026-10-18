use serde_json::{Value, json};

pub(super) fn native_device_trust_fixture() -> anyhow::Result<Value> {
    crate::core::secure_mesh_trust::evaluate_device_trust_verification_json(
        &json!({
            "localIdentity": native_device_identity_fixture("desktop-native-fixture", 1),
            "peerIdentity": native_device_identity_fixture("mobile-native-fixture", 2),
            "rosterEpoch": 1
        }),
        "sas",
    )
}

pub(super) fn native_device_identity_fixture(endpoint_id: &str, byte: u8) -> Value {
    json!({
        "endpointId": endpoint_id,
        "identityPublicKey": hex_bytes(byte),
        "signingPublicKey": hex_bytes(byte.saturating_add(1)),
        "rotationEpoch": 1
    })
}

pub(super) fn hex_bytes(byte: u8) -> String {
    vec![format!("{byte:02x}"); 32].join(":")
}
