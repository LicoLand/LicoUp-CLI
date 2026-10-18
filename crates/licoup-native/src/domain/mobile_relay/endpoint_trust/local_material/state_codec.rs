use super::state::LocalEndpointState;
use crate::core::secure_mesh_trust::DeviceTrustPublicIdentity;
use crate::domain::mobile_relay::endpoint_trust::{
    decode_key_32, descriptor_text, public_key_fingerprint,
};
use crate::domain::mobile_relay::secret_custody::{
    MobileRelayE2eeSecretField, RuntimeSecretMaterial,
};
use anyhow::{Result, anyhow};
use serde_json::Value;

pub(in crate::domain::mobile_relay) fn hex_encode_bytes(bytes: &[u8]) -> String {
    let mut result = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        use std::fmt::Write as _;
        let _ = write!(result, "{byte:02x}");
    }
    result
}

pub(in crate::domain::mobile_relay) fn local_endpoint_state<'a>(
    config: &Value,
    secret_material: &'a RuntimeSecretMaterial,
) -> Result<LocalEndpointState<'a>> {
    super::material_mutation::validate_existing_identity_custody(config, secret_material)?;
    let state = config
        .get("mobileRelayE2ee")
        .ok_or_else(|| anyhow!("mobile relay E2EE endpoint state is missing"))?;
    let endpoint_id = descriptor_text(state, "endpointId")?;
    let endpoint_kind = descriptor_text(state, "endpointKind")?;
    let private_key = secret_material
        .e2ee_secret(MobileRelayE2eeSecretField::PrivateKey)
        .ok_or_else(|| anyhow!("mobile relay local private key is missing"))?
        .expose_utf8()?;
    let public_key = descriptor_text(state, "publicKeyBase64url")?;
    let signing_key = secret_material
        .e2ee_secret(MobileRelayE2eeSecretField::SigningKey)
        .ok_or_else(|| anyhow!("mobile relay local signing key is missing"))?
        .expose_utf8()?;
    let signing_public_key = descriptor_text(state, "signingPublicKeyBase64url")?;
    let rotation_epoch = state
        .get("rotationEpoch")
        .and_then(Value::as_u64)
        .unwrap_or(1);
    let mailbox_rotation_epoch = state
        .get("mailboxRotationEpoch")
        .and_then(Value::as_u64)
        .ok_or_else(|| anyhow!("Lico Arc mailbox rotation epoch is missing"))?;
    let prekey_publication_version = state
        .get("prekeyPublicationVersion")
        .and_then(Value::as_u64)
        .ok_or_else(|| anyhow!("mobile relay prekey publication version is missing"))?;
    let signed_prekey_id = descriptor_text(state, "signedPrekeyId")?;
    let signed_prekey_private_key = secret_material
        .e2ee_secret(MobileRelayE2eeSecretField::SignedPrekeyPrivateKey)
        .ok_or_else(|| anyhow!("mobile relay signed prekey private key is missing"))?
        .expose_utf8()?;
    let signed_prekey_public_key = descriptor_text(state, "signedPrekeyPublicKeyBase64url")?;
    let signed_prekey_signature = descriptor_text(state, "signedPrekeySignatureBase64url")?;
    let signed_prekey_created_at = descriptor_text(state, "signedPrekeyCreatedAt")?;
    let signed_prekey_expires_at = descriptor_text(state, "signedPrekeyExpiresAt")?;
    let one_time_prekey_id = descriptor_text(state, "oneTimePrekeyId")?;
    let one_time_prekey_private_key = secret_material
        .e2ee_secret(MobileRelayE2eeSecretField::OneTimePrekeyPrivateKey)
        .ok_or_else(|| anyhow!("mobile relay one-time prekey private key is missing"))?
        .expose_utf8()?;
    let one_time_prekey_public_key = descriptor_text(state, "oneTimePrekeyPublicKeyBase64url")?;
    let one_time_prekey_signature = descriptor_text(state, "oneTimePrekeySignatureBase64url")?;
    let one_time_prekey_created_at = descriptor_text(state, "oneTimePrekeyCreatedAt")?;
    let one_time_prekey_expires_at = descriptor_text(state, "oneTimePrekeyExpiresAt")?;
    let one_time_mlkem1024_prekey_id = descriptor_text(state, "oneTimeMlKem1024PrekeyId")?;
    let one_time_mlkem1024_prekey_seed = secret_material
        .e2ee_secret(MobileRelayE2eeSecretField::OneTimeMlKem1024PrekeySeed)
        .ok_or_else(|| anyhow!("mobile relay ML-KEM seed is missing"))?
        .expose_utf8()?;
    let one_time_mlkem1024_prekey_public_key =
        descriptor_text(state, "oneTimeMlKem1024PrekeyPublicKeyBase64url")?;
    let one_time_mlkem1024_prekey_signature =
        descriptor_text(state, "oneTimeMlKem1024PrekeySignatureBase64url")?;
    let one_time_mlkem1024_prekey_created_at =
        descriptor_text(state, "oneTimeMlKem1024PrekeyCreatedAt")?;
    let one_time_mlkem1024_prekey_expires_at =
        descriptor_text(state, "oneTimeMlKem1024PrekeyExpiresAt")?;
    let public_bytes = decode_key_32(&public_key, "mobile relay public key")?;
    let session_id = descriptor_text(state, "sessionId")?;
    let key_transparency_response = state
        .get("keyTransparencyResponse")
        .filter(|value| value.is_object())
        .cloned();
    Ok(LocalEndpointState {
        endpoint_id,
        endpoint_kind,
        private_key,
        public_key,
        signing_key,
        signing_public_key,
        rotation_epoch,
        mailbox_rotation_epoch,
        prekey_publication_version,
        signed_prekey_id,
        signed_prekey_private_key,
        signed_prekey_public_key,
        signed_prekey_signature,
        signed_prekey_created_at,
        signed_prekey_expires_at,
        one_time_prekey_id,
        one_time_prekey_private_key,
        one_time_prekey_public_key,
        one_time_prekey_signature,
        one_time_prekey_created_at,
        one_time_prekey_expires_at,
        one_time_mlkem1024_prekey_id,
        one_time_mlkem1024_prekey_seed,
        one_time_mlkem1024_prekey_public_key,
        one_time_mlkem1024_prekey_signature,
        one_time_mlkem1024_prekey_created_at,
        one_time_mlkem1024_prekey_expires_at,
        fingerprint: public_key_fingerprint(&public_bytes),
        session_id,
        pending_pairwise_intro: state.get("pendingPairwiseIntro").cloned(),
        pairwise_accepted: state.get("pairwiseAccepted").cloned(),
        pairwise_finished: state.get("pairwiseFinished").cloned(),
        key_transparency_response,
    })
}

pub(in crate::domain::mobile_relay) fn local_public_device_identity(
    config: &Value,
) -> Result<DeviceTrustPublicIdentity> {
    let state = config
        .get("mobileRelayE2ee")
        .ok_or_else(|| anyhow!("mobile relay E2EE endpoint state is missing"))?;
    DeviceTrustPublicIdentity::new(
        descriptor_text(state, "endpointId")?,
        decode_key_32(
            &descriptor_text(state, "publicKeyBase64url")?,
            "mobile relay identity public key",
        )?,
        decode_key_32(
            &descriptor_text(state, "signingPublicKeyBase64url")?,
            "mobile relay signing public key",
        )?,
        state
            .get("rotationEpoch")
            .and_then(Value::as_u64)
            .unwrap_or(1),
    )
}
