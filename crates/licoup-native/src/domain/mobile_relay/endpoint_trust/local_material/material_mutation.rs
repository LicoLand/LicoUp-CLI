use super::identity_generation::{
    derive_identity_public, generate_endpoint_id, generate_identity_material,
    generate_pairing_secret, generate_session_id,
};
use super::prekey_inventory::ensure_mobile_relay_pqxdh_material;
use super::protocol_reset::ensure_local_pairwise_protocol_compatible;
use crate::core::secure_mesh_secret_store::SecretBytes;
use crate::domain::mobile_relay::endpoint_trust::decode_key_32;
use crate::domain::mobile_relay::relay_operations::current_mailbox_rotation_epoch;
use crate::domain::mobile_relay::secret_custody::{
    MobileRelayE2eeSecretField, RuntimeSecretMaterial,
};
use crate::domain::mobile_relay::support::MOBILE_RELAY_E2EE_PROTOCOL_VERSION;
use anyhow::{Result, ensure};
use base64::{Engine, engine::general_purpose};
use ed25519_dalek::SigningKey;
use serde_json::{Map, Value, json};
use zeroize::Zeroizing;

/// Presence of identity-bearing fields. Regenerating material over any of
/// these would silently replace an existing device identity.
pub(in crate::domain::mobile_relay) fn local_identity_metadata_present(
    object: &Map<String, Value>,
) -> bool {
    const IDENTITY_FIELDS: [&str; 6] = [
        "endpointId",
        "publicKeyBase64url",
        "fingerprint",
        "signingPublicKeyBase64url",
        "privateKeyBase64url",
        "signingKeyBase64url",
    ];
    IDENTITY_FIELDS.iter().any(|field| {
        object
            .get(*field)
            .and_then(Value::as_str)
            .is_some_and(|value| !value.trim().is_empty())
    })
}

/// Metadata cannot substitute for custody or silently change the local identity.
/// The actual authorized material must reproduce the recorded public identity.
pub(in crate::domain::mobile_relay) fn existing_identity_requires_custody(config: &Value) -> bool {
    [
        "endpointId",
        "publicKeyBase64url",
        "fingerprint",
        "signingPublicKeyBase64url",
        "privateKeyMaterial",
        "signingKeyMaterial",
        "privateKeyBase64url",
        "signingKeyBase64url",
    ]
    .into_iter()
    .any(|field| {
        config
            .get("mobileRelayE2ee")
            .and_then(|value| value.get(field))
            .and_then(Value::as_str)
            .is_some_and(|value| !value.trim().is_empty())
    })
}

pub(in crate::domain::mobile_relay) fn validate_existing_identity_custody(
    config: &Value,
    material: &RuntimeSecretMaterial,
) -> Result<()> {
    ensure_local_pairwise_protocol_compatible(config)?;
    let Some(object) = config.get("mobileRelayE2ee").and_then(Value::as_object) else {
        return Ok(());
    };
    let text = |name| {
        object
            .get(name)
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|s| !s.is_empty())
    };
    if !existing_identity_requires_custody(config) {
        ensure!(
            material
                .e2ee_secret(MobileRelayE2eeSecretField::PrivateKey)
                .is_none()
                && material
                    .e2ee_secret(MobileRelayE2eeSecretField::SigningKey)
                    .is_none(),
            "mobile relay existing local custody requires its recorded public identity"
        );
        return Ok(());
    }
    ensure!(
        text("endpointId").is_some()
            && text("publicKeyBase64url").is_some()
            && text("signingPublicKeyBase64url").is_some(),
        "mobile relay existing identity metadata is incomplete"
    );
    let private = material.e2ee_secret(MobileRelayE2eeSecretField::PrivateKey)
        .ok_or_else(|| anyhow::anyhow!("mobile relay existing identity custody is unavailable; refusing identity regeneration"))?;
    let (_, public, fingerprint) = derive_identity_public(private.expose_utf8()?)?;
    ensure!(
        text("publicKeyBase64url").is_none_or(|expected| expected == public)
            && text("fingerprint").is_none_or(|expected| expected == fingerprint),
        "mobile relay existing identity custody does not match its recorded owner"
    );
    if let Some(expected) = text("signingPublicKeyBase64url") {
        let private = material.e2ee_secret(MobileRelayE2eeSecretField::SigningKey)
            .ok_or_else(|| anyhow::anyhow!("mobile relay existing signing custody is unavailable; refusing identity regeneration"))?;
        let decoded = Zeroizing::new(decode_key_32(
            private.expose_utf8()?,
            "mobile relay signing custody",
        )?);
        let signing = SigningKey::from_bytes(&decoded);
        ensure!(
            general_purpose::URL_SAFE_NO_PAD.encode(signing.verifying_key().to_bytes()) == expected,
            "mobile relay existing identity custody does not match its recorded owner"
        );
    }
    if let Some(origin) =
        crate::domain::mobile_relay::secret_custody::recorded_custody_namespace(config)?
    {
        let current = crate::domain::mobile_relay::secret_custody::current_custody_namespace()?;
        if !crate::domain::mobile_relay::secret_custody::custody_locator_matches_current_home(
            origin,
        )? {
            ensure!(
                material
                    .local_custody_namespace()
                    .is_some_and(|observed| observed == origin || observed == current),
                "mobile relay different-home identity requires authorized local custody"
            );
        }
    }
    Ok(())
}

pub(in crate::domain::mobile_relay) fn ensure_mobile_relay_endpoint_material(
    config: &mut Value,
    secret_material: &mut RuntimeSecretMaterial,
    endpoint_kind: &str,
) -> Result<()> {
    // Validate before protocol normalization or any generated replacement.
    validate_existing_identity_custody(config, secret_material)?;
    if config
        .get("mobileRelayE2ee")
        .and_then(Value::as_object)
        .is_none()
    {
        config["mobileRelayE2ee"] = json!({});
    }
    if let Some(object) = config
        .get_mut("mobileRelayE2ee")
        .and_then(Value::as_object_mut)
    {
        if secret_material
            .e2ee_secret(MobileRelayE2eeSecretField::PrivateKey)
            .is_none()
        {
            let generated = generate_identity_material();
            secret_material.insert_e2ee_secret(
                MobileRelayE2eeSecretField::PrivateKey,
                SecretBytes::try_from_string(generated.private_key)?,
            )?;
            object.insert(
                "publicKeyBase64url".to_string(),
                json!(generated.public_key),
            );
            object.insert("fingerprint".to_string(), json!(generated.fingerprint));
        }
        if !object
            .get("endpointId")
            .and_then(Value::as_str)
            .is_some_and(|value| !value.trim().is_empty())
        {
            object.insert(
                "endpointId".to_string(),
                json!(generate_endpoint_id(endpoint_kind)),
            );
        }
        if !object
            .get("sessionId")
            .and_then(Value::as_str)
            .is_some_and(|value| !value.trim().is_empty())
        {
            object.insert("sessionId".to_string(), json!(generate_session_id()));
        }
        object.insert(
            "protocolVersion".to_string(),
            json!(MOBILE_RELAY_E2EE_PROTOCOL_VERSION),
        );
        object.insert("endpointKind".to_string(), json!(endpoint_kind));
        object
            .entry("peerVerified".to_string())
            .or_insert_with(|| json!(false));
        if object
            .get("mailboxRotationEpoch")
            .and_then(Value::as_u64)
            .is_none()
        {
            object.insert(
                "mailboxRotationEpoch".to_string(),
                json!(current_mailbox_rotation_epoch()?),
            );
        }
        if secret_material
            .e2ee_secret(MobileRelayE2eeSecretField::PairingSecret)
            .is_none()
        {
            secret_material.insert_e2ee_secret(
                MobileRelayE2eeSecretField::PairingSecret,
                SecretBytes::try_from_string(generate_pairing_secret())?,
            )?;
        }
    }
    ensure_mobile_relay_pqxdh_material(config, secret_material)
}
