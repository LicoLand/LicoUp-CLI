use super::super::ensure_mobile_relay_endpoint_material;
use super::super::identity_generation::{
    derive_identity_public, generate_identity_material, signing_material,
};
use crate::domain::mobile_relay::secret_custody::{
    MobileRelayE2eeSecretField, RuntimeSecretMaterial,
};
use licoup_foundation::platform::paths::set_portable_data_dir_override;
use serde_json::json;

struct FixtureHome {
    path: std::path::PathBuf,
    previous: Option<std::path::PathBuf>,
}

impl FixtureHome {
    fn enter(name: &str) -> Self {
        let path = std::env::temp_dir().join(format!("licoup-{name}-{}", uuid::Uuid::new_v4()));
        let previous = set_portable_data_dir_override(Some(path.clone()));
        Self { path, previous }
    }
}

impl Drop for FixtureHome {
    fn drop(&mut self) {
        set_portable_data_dir_override(self.previous.take());
        let _ = std::fs::remove_dir_all(&self.path);
    }
}

#[test]
fn identity_and_signing_generation_round_trip_without_config_mutation() {
    let identity = generate_identity_material();
    let (_, public_key, fingerprint) = derive_identity_public(&identity.private_key).unwrap();
    assert_eq!(public_key, identity.public_key);
    assert_eq!(fingerprint, identity.fingerprint);

    let signing = signing_material(None).unwrap();
    let restored = signing_material(Some(&signing.private_key)).unwrap();
    assert_eq!(restored.public_key, signing.public_key);
}

#[test]
fn incomplete_existing_identity_is_not_a_fresh_identity() {
    let _home = FixtureHome::enter("incomplete-identity");
    for e2ee in [
        json!({"endpointId": "pc_existing"}),
        json!({"privateKeyBase64url": "synthetic-existing-private-material"}),
    ] {
        let mut config = json!({"mobileRelayE2ee": e2ee});
        let before = config.clone();
        let mut material = RuntimeSecretMaterial::new();
        let error =
            ensure_mobile_relay_endpoint_material(&mut config, &mut material, "desktop_sidecar")
                .unwrap_err();
        assert!(error.to_string().contains("metadata is incomplete"));
        assert_eq!(config, before);
        assert!(material.is_empty());
    }
}

#[test]
fn identity_generation_refuses_metadata_bound_to_another_data_home() {
    let _home = FixtureHome::enter("identity-generation-data-home");
    let mut config = json!({
        "mobileRelayE2ee": {
            "endpointId": "pc_fixture-identity",
            "publicKeyBase64url": "synthetic-public-key",
            "fingerprint": "synthetic-fingerprint",
            "signingPublicKeyBase64url": "synthetic-signing-key",
            "custodyNamespace": "0000000000000000000000000000000000000000000000000000000000000000"
        }
    });
    let before = config.clone();
    let mut material = RuntimeSecretMaterial::new();

    let error =
        ensure_mobile_relay_endpoint_material(&mut config, &mut material, "desktop_sidecar")
            .unwrap_err()
            .to_string();

    assert!(error.contains("custody is unavailable"), "{error}");
    assert_eq!(config, before);
    assert!(
        material
            .e2ee_secret(MobileRelayE2eeSecretField::PrivateKey)
            .is_none()
    );
}

#[test]
fn active_target_identity_with_another_home_binding_is_refused_without_mutation() {
    let _home = FixtureHome::enter("identity-binding-active-target");
    let source_identity = generate_identity_material();
    let target_identity = generate_identity_material();
    let signing = signing_material(None).unwrap();
    let mut config = json!({
        "mobileRelayE2ee": {
            "endpointId": "pc_active_target",
            "publicKeyBase64url": source_identity.public_key,
            "fingerprint": source_identity.fingerprint,
            "signingPublicKeyBase64url": signing.public_key,
            "custodyNamespace": "ffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff"
        }
    });
    let before = config.clone();
    let mut material = RuntimeSecretMaterial::new();
    material
        .insert_e2ee_secret(
            MobileRelayE2eeSecretField::PrivateKey,
            crate::core::secure_mesh_secret_store::SecretBytes::try_from_string(
                target_identity.private_key,
            )
            .unwrap(),
        )
        .unwrap();

    let error =
        ensure_mobile_relay_endpoint_material(&mut config, &mut material, "desktop_sidecar")
            .unwrap_err()
            .to_string();

    assert!(error.contains("does not match"), "{error}");
    assert_eq!(config, before);
    assert!(
        material
            .e2ee_secret(MobileRelayE2eeSecretField::PrivateKey)
            .is_some()
    );
}
