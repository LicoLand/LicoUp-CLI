use super::config::validate_canonical_sha256_hex;
use crate::core::secure_mesh_directory::{
    SecureMeshDirectoryKeyMaterialCommitment, SecureMeshDirectoryLeafClaim,
};
use crate::core::secure_mesh_pqxdh::ML_KEM_1024_PUBLIC_KEY_BYTES;
use crate::core::secure_mesh_prekey::{
    SecureMeshPairwisePreKeyBundle, SecureMeshPreKeyRecord, one_time_prekey_batch_digest,
    signed_prekey_bundle_digest,
};
use crate::core::secure_mesh_transparency::SecureMeshTransparencyLeafBody;
use crate::core::secure_mesh_trust::{DeviceTrustPublicIdentity, DeviceTrustState};
use crate::domain::mobile_relay::endpoint_trust::{
    decode_fixed_base64url, decode_key_32, descriptor_text, hex_encode_bytes, now_iso,
};
use anyhow::{Result, anyhow};
use serde_json::Value;

pub(in crate::domain::mobile_relay) fn local_pairwise_prekey_bundle_from_config(
    config: &Value,
) -> Result<SecureMeshPairwisePreKeyBundle> {
    let state = config
        .get("mobileRelayE2ee")
        .ok_or_else(|| anyhow!("mobile relay E2EE endpoint state is missing"))?;
    let identity = DeviceTrustPublicIdentity::new(
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
            .ok_or_else(|| anyhow!("mobile relay identity rotation epoch is missing"))?,
    )?;
    Ok(SecureMeshPairwisePreKeyBundle {
        endpoint_identity: identity,
        trust_state: DeviceTrustState::Verified,
        signed_prekey: SecureMeshPreKeyRecord {
            prekey_id: descriptor_text(state, "signedPrekeyId")?,
            public_key: decode_key_32(
                &descriptor_text(state, "signedPrekeyPublicKeyBase64url")?,
                "mobile relay signed prekey public key",
            )?
            .to_vec(),
            signature: descriptor_text(state, "signedPrekeySignatureBase64url")?,
            created_at: descriptor_text(state, "signedPrekeyCreatedAt")?,
            expires_at: descriptor_text(state, "signedPrekeyExpiresAt")?,
        },
        one_time_prekey: Some(SecureMeshPreKeyRecord {
            prekey_id: descriptor_text(state, "oneTimePrekeyId")?,
            public_key: decode_key_32(
                &descriptor_text(state, "oneTimePrekeyPublicKeyBase64url")?,
                "mobile relay one-time prekey public key",
            )?
            .to_vec(),
            signature: descriptor_text(state, "oneTimePrekeySignatureBase64url")?,
            created_at: descriptor_text(state, "oneTimePrekeyCreatedAt")?,
            expires_at: descriptor_text(state, "oneTimePrekeyExpiresAt")?,
        }),
        one_time_mlkem1024_prekey: SecureMeshPreKeyRecord {
            prekey_id: descriptor_text(state, "oneTimeMlKem1024PrekeyId")?,
            public_key: decode_fixed_base64url::<ML_KEM_1024_PUBLIC_KEY_BYTES>(
                &descriptor_text(state, "oneTimeMlKem1024PrekeyPublicKeyBase64url")?,
                "mobile relay ML-KEM-1024 one-time prekey public key",
            )?
            .to_vec(),
            signature: descriptor_text(state, "oneTimeMlKem1024PrekeySignatureBase64url")?,
            created_at: descriptor_text(state, "oneTimeMlKem1024PrekeyCreatedAt")?,
            expires_at: descriptor_text(state, "oneTimeMlKem1024PrekeyExpiresAt")?,
        },
        prekey_publication_version: state
            .get("prekeyPublicationVersion")
            .and_then(Value::as_u64)
            .ok_or_else(|| anyhow!("mobile relay prekey publication version is missing"))?,
    })
}

pub(in crate::domain::mobile_relay) fn build_local_directory_claim(
    config: &Value,
    directory_scope_commitment: &str,
    directory_version: u64,
    directory_state: &str,
    mls_key_package_digest: &str,
    mls_key_package_version: u64,
) -> Result<SecureMeshDirectoryLeafClaim> {
    validate_canonical_sha256_hex(directory_scope_commitment, "directory scope commitment")?;
    validate_canonical_sha256_hex(mls_key_package_digest, "MLS KeyPackage digest")?;
    let bundle = local_pairwise_prekey_bundle_from_config(config)?;
    let state = config
        .get("mobileRelayE2ee")
        .ok_or_else(|| anyhow!("mobile relay E2EE endpoint state is missing"))?;
    Ok(SecureMeshDirectoryLeafClaim {
        endpoint: SecureMeshTransparencyLeafBody {
            directory_scope_commitment: directory_scope_commitment.to_string(),
            endpoint_id: bundle.endpoint_identity.endpoint_id.clone(),
            endpoint_kind: descriptor_text(state, "endpointKind")?,
            identity_public_key: hex_encode_bytes(&bundle.endpoint_identity.identity_public_key),
            signing_public_key: hex_encode_bytes(&bundle.endpoint_identity.signing_public_key),
            fingerprint: bundle.endpoint_identity.fingerprint()?,
            rotation_epoch: bundle.endpoint_identity.rotation_epoch,
            directory_state: directory_state.to_string(),
            updated_at: now_iso(),
        },
        key_material: SecureMeshDirectoryKeyMaterialCommitment {
            signed_prekey_bundle_digest: signed_prekey_bundle_digest(&bundle)?,
            one_time_prekey_batch_digest: one_time_prekey_batch_digest(&bundle)?,
            pairwise_prekey_version: bundle.prekey_publication_version,
            mls_key_package_digest: mls_key_package_digest.to_string(),
            mls_key_package_version,
        },
        directory_version,
    })
}
