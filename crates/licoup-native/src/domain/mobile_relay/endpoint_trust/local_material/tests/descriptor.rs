use super::super::{
    descriptor::local_endpoint_public_descriptor,
    material_mutation::ensure_mobile_relay_endpoint_material, state_codec::local_endpoint_state,
};
use crate::domain::mobile_relay::test_runtime_secret_material;
use serde_json::json;

#[test]
fn public_descriptors_never_expose_local_private_material() {
    let mut config = json!({});
    ensure_mobile_relay_endpoint_material(
        &mut config,
        &mut test_runtime_secret_material(stringify!(&mut config)),
        "desktop",
    )
    .unwrap();
    config["mobileRelayE2ee"]["keyTransparencyResponse"] = json!({"verified": true});

    let full_descriptor = local_endpoint_state(
        &config,
        &mut test_runtime_secret_material(stringify!(&config)),
    )
    .unwrap()
    .public_descriptor()
    .unwrap();
    let public_only_descriptor = local_endpoint_public_descriptor(&config).unwrap();

    for descriptor in [full_descriptor, public_only_descriptor] {
        let encoded = descriptor.to_string();
        assert!(!encoded.contains("privateKeyBase64url"));
        assert!(!encoded.contains("signingKeyBase64url"));
        assert!(!encoded.contains("PrekeySeedBase64url"));
    }
}

#[test]
fn public_descriptor_requires_key_transparency_proof() {
    let mut config = json!({});
    ensure_mobile_relay_endpoint_material(
        &mut config,
        &mut test_runtime_secret_material(stringify!(&mut config)),
        "desktop",
    )
    .unwrap();

    let error = local_endpoint_public_descriptor(&config).unwrap_err();

    assert!(error.to_string().contains("key transparency"));
}
