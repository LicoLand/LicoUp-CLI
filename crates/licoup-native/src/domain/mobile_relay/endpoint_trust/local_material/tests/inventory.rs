use super::super::material_mutation::ensure_mobile_relay_endpoint_material;
use crate::domain::mobile_relay::secret_custody::MobileRelayE2eeSecretField;
use crate::domain::mobile_relay::test_runtime_secret_material;
use serde_json::json;

#[test]
fn endpoint_material_builds_complete_pqxdh_inventory() {
    let mut config = json!({});

    let mut material = test_runtime_secret_material(stringify!(&mut config));
    ensure_mobile_relay_endpoint_material(&mut config, &mut material, "desktop").unwrap();

    let state = config["mobileRelayE2ee"].as_object().unwrap();
    for field in [
        "publicKeyBase64url",
        "signingPublicKeyBase64url",
        "signedPrekeyId",
        "signedPrekeyPublicKeyBase64url",
        "oneTimePrekeyId",
        "oneTimePrekeyPublicKeyBase64url",
        "oneTimeMlKem1024PrekeyId",
        "oneTimeMlKem1024PrekeyPublicKeyBase64url",
    ] {
        assert!(
            state
                .get(field)
                .and_then(|value| value.as_str())
                .is_some_and(|value| !value.is_empty()),
            "missing local material field: {field}"
        );
    }
    for field in [
        MobileRelayE2eeSecretField::PrivateKey,
        MobileRelayE2eeSecretField::SigningKey,
        MobileRelayE2eeSecretField::SignedPrekeyPrivateKey,
        MobileRelayE2eeSecretField::OneTimePrekeyPrivateKey,
        MobileRelayE2eeSecretField::OneTimeMlKem1024PrekeySeed,
    ] {
        assert!(
            material.e2ee_secret(field).is_some(),
            "missing runtime-only local secret field: {}",
            field.config_field()
        );
        assert!(
            state.get(field.config_field()).is_none(),
            "runtime-only local secret leaked into public config: {}",
            field.config_field()
        );
    }
    assert_eq!(state["prekeyPublicationVersion"], 1);
}
