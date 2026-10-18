use super::state::LocalEndpointState;
use crate::core::secure_mesh_pairwise::SECURE_MESH_PAIRWISE_CIPHER_SUITE;
use crate::core::secure_mesh_trust::DeviceTrustPublicIdentity;
use crate::domain::mobile_relay::endpoint_trust::{
    decode_key_32, descriptor_text, device_identity_to_json, public_key_fingerprint,
};
use crate::domain::mobile_relay::support::MOBILE_RELAY_E2EE_PROTOCOL_VERSION;
use anyhow::{Result, anyhow};
use serde_json::{Value, json};

impl LocalEndpointState<'_> {
    pub(in crate::domain::mobile_relay) fn public_descriptor(&self) -> Result<Value> {
        let identity = self.device_identity()?;
        Ok(json!({
            "protocolVersion": MOBILE_RELAY_E2EE_PROTOCOL_VERSION,
            "endpointId": self.endpoint_id,
            "endpointKind": self.endpoint_kind,
            "publicKeyBase64url": self.public_key,
            "fingerprint": self.fingerprint,
            "deviceTrustFingerprint": identity.fingerprint()?,
            "identityPublicKeyBase64url": self.public_key,
            "signingPublicKeyBase64url": self.signing_public_key,
            "rotationEpoch": self.rotation_epoch,
            "mailboxRotationEpoch": self.mailbox_rotation_epoch,
            "prekeyPublicationVersion": self.prekey_publication_version,
            "sessionId": self.session_id,
            "keyAgreement": "pqxdh-x25519-ed25519-mlkem1024-triple-ratchet",
            "payloadCipher": SECURE_MESH_PAIRWISE_CIPHER_SUITE,
            "preKeyBundle": self.prekey_bundle_descriptor()?,
            "pairwiseIntro": self.pending_pairwise_intro_descriptor(),
            "pairwiseAccepted": self.pairwise_accepted_descriptor(),
            "pairwiseFinished": self.pairwise_finished_descriptor()
        }))
    }

    pub(in crate::domain::mobile_relay) fn prekey_bundle_descriptor(&self) -> Result<Value> {
        let identity = self.device_identity()?;
        let key_transparency_response = self
            .key_transparency_response
            .as_ref()
            .ok_or_else(|| anyhow!("mobile relay key transparency response is missing"))?;
        Ok(json!({
            "protocolVersion": crate::core::secure_mesh_prekey::SECURE_MESH_PREKEY_PROTOCOL_VERSION,
            "endpointIdentity": device_identity_to_json(&identity)?,
            "signedPrekey": {
                "prekeyId": self.signed_prekey_id,
                "publicKeyBase64url": self.signed_prekey_public_key,
                "signatureBase64url": self.signed_prekey_signature,
                "createdAt": self.signed_prekey_created_at,
                "expiresAt": self.signed_prekey_expires_at
            },
            "oneTimePrekey": {
                "prekeyId": self.one_time_prekey_id,
                "publicKeyBase64url": self.one_time_prekey_public_key,
                "signatureBase64url": self.one_time_prekey_signature,
                "createdAt": self.one_time_prekey_created_at,
                "expiresAt": self.one_time_prekey_expires_at
            },
            "oneTimeMlKem1024Prekey": {
                "prekeyId": self.one_time_mlkem1024_prekey_id,
                "publicKeyBase64url": self.one_time_mlkem1024_prekey_public_key,
                "signatureBase64url": self.one_time_mlkem1024_prekey_signature,
                "createdAt": self.one_time_mlkem1024_prekey_created_at,
                "expiresAt": self.one_time_mlkem1024_prekey_expires_at
            },
            "prekeyPublicationVersion": self.prekey_publication_version,
            "keyTransparency": key_transparency_response
        }))
    }

    pub(in crate::domain::mobile_relay) fn pending_pairwise_intro_descriptor(&self) -> Value {
        self.pending_pairwise_intro.clone().unwrap_or(Value::Null)
    }

    pub(in crate::domain::mobile_relay) fn pairwise_accepted_descriptor(&self) -> Value {
        self.pairwise_accepted.clone().unwrap_or(Value::Null)
    }

    pub(in crate::domain::mobile_relay) fn pairwise_finished_descriptor(&self) -> Value {
        self.pairwise_finished.clone().unwrap_or(Value::Null)
    }
}

pub(in crate::domain::mobile_relay) fn local_endpoint_public_descriptor(
    config: &Value,
) -> Result<Value> {
    let state = config
        .get("mobileRelayE2ee")
        .ok_or_else(|| anyhow!("mobile relay E2EE endpoint state is missing"))?;
    let endpoint_id = descriptor_text(state, "endpointId")?;
    let endpoint_kind = descriptor_text(state, "endpointKind")?;
    let public_key = descriptor_text(state, "publicKeyBase64url")?;
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
    let public_bytes = decode_key_32(&public_key, "mobile relay public key")?;
    let signing_public_bytes =
        decode_key_32(&signing_public_key, "mobile relay signing public key")?;
    let identity = DeviceTrustPublicIdentity::new(
        endpoint_id.clone(),
        public_bytes,
        signing_public_bytes,
        rotation_epoch,
    )?;
    Ok(json!({
        "protocolVersion": MOBILE_RELAY_E2EE_PROTOCOL_VERSION,
        "endpointId": endpoint_id,
        "endpointKind": endpoint_kind,
        "publicKeyBase64url": public_key,
        "fingerprint": public_key_fingerprint(&public_bytes),
        "deviceTrustFingerprint": identity.fingerprint()?,
        "identityPublicKeyBase64url": public_key,
        "signingPublicKeyBase64url": signing_public_key,
        "rotationEpoch": rotation_epoch,
        "mailboxRotationEpoch": mailbox_rotation_epoch,
        "prekeyPublicationVersion": prekey_publication_version,
        "sessionId": descriptor_text(state, "sessionId")?,
        "keyAgreement": "pqxdh-x25519-ed25519-mlkem1024-triple-ratchet",
        "payloadCipher": SECURE_MESH_PAIRWISE_CIPHER_SUITE,
        "preKeyBundle": {
            "protocolVersion": crate::core::secure_mesh_prekey::SECURE_MESH_PREKEY_PROTOCOL_VERSION,
            "endpointIdentity": device_identity_to_json(&identity)?,
            "signedPrekey": {
                "prekeyId": descriptor_text(state, "signedPrekeyId")?,
                "publicKeyBase64url": descriptor_text(state, "signedPrekeyPublicKeyBase64url")?,
                "signatureBase64url": descriptor_text(state, "signedPrekeySignatureBase64url")?,
                "createdAt": descriptor_text(state, "signedPrekeyCreatedAt")?,
                "expiresAt": descriptor_text(state, "signedPrekeyExpiresAt")?
            },
            "oneTimePrekey": {
                "prekeyId": descriptor_text(state, "oneTimePrekeyId")?,
                "publicKeyBase64url": descriptor_text(state, "oneTimePrekeyPublicKeyBase64url")?,
                "signatureBase64url": descriptor_text(state, "oneTimePrekeySignatureBase64url")?,
                "createdAt": descriptor_text(state, "oneTimePrekeyCreatedAt")?,
                "expiresAt": descriptor_text(state, "oneTimePrekeyExpiresAt")?
            },
            "oneTimeMlKem1024Prekey": {
                "prekeyId": descriptor_text(state, "oneTimeMlKem1024PrekeyId")?,
                "publicKeyBase64url": descriptor_text(state, "oneTimeMlKem1024PrekeyPublicKeyBase64url")?,
                "signatureBase64url": descriptor_text(state, "oneTimeMlKem1024PrekeySignatureBase64url")?,
                "createdAt": descriptor_text(state, "oneTimeMlKem1024PrekeyCreatedAt")?,
                "expiresAt": descriptor_text(state, "oneTimeMlKem1024PrekeyExpiresAt")?
            },
            "prekeyPublicationVersion": prekey_publication_version,
            "keyTransparency": state
                .get("keyTransparencyResponse")
                .filter(|value| value.is_object())
                .cloned()
                .ok_or_else(|| anyhow!("mobile relay key transparency response is missing"))?
        },
        "pairwiseIntro": state
            .get("pendingPairwiseIntro")
            .cloned()
            .unwrap_or(Value::Null),
        "pairwiseAccepted": state
            .get("pairwiseAccepted")
            .cloned()
            .unwrap_or(Value::Null),
        "pairwiseFinished": state
            .get("pairwiseFinished")
            .cloned()
            .unwrap_or(Value::Null)
    }))
}
