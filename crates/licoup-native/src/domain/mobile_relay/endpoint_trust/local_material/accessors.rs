use super::state::LocalEndpointState;
use crate::core::secure_mesh_pairwise::SecureMeshPairwisePrivateKey;
use crate::core::secure_mesh_pqxdh::{
    ML_KEM_1024_KEY_GENERATION_SEED_BYTES, ML_KEM_1024_PUBLIC_KEY_BYTES,
    SecureMeshMlKem1024PreKeySeed,
};
use crate::core::secure_mesh_prekey::{SecureMeshPairwisePreKeyBundle, SecureMeshPreKeyRecord};
use crate::core::secure_mesh_trust::{DeviceTrustPublicIdentity, DeviceTrustState};
use crate::domain::mobile_relay::endpoint_trust::{decode_fixed_base64url, decode_key_32};
use anyhow::{Result, anyhow, ensure};
use ed25519_dalek::SigningKey;

impl LocalEndpointState<'_> {
    pub(in crate::domain::mobile_relay) fn device_identity(
        &self,
    ) -> Result<DeviceTrustPublicIdentity> {
        DeviceTrustPublicIdentity::new(
            self.endpoint_id.clone(),
            decode_key_32(&self.public_key, "mobile relay identity public key")?,
            decode_key_32(&self.signing_public_key, "mobile relay signing public key")?,
            self.rotation_epoch,
        )
    }

    pub(in crate::domain::mobile_relay) fn identity_secret(
        &self,
    ) -> Result<SecureMeshPairwisePrivateKey> {
        Ok(SecureMeshPairwisePrivateKey::from_bytes(decode_key_32(
            &self.private_key,
            "mobile relay local private key",
        )?))
    }

    pub(in crate::domain::mobile_relay) fn signing_key(&self) -> Result<SigningKey> {
        Ok(SigningKey::from_bytes(&decode_key_32(
            &self.signing_key,
            "mobile relay local signing key",
        )?))
    }

    pub(in crate::domain::mobile_relay) fn pairwise_prekey_bundle(
        &self,
    ) -> Result<SecureMeshPairwisePreKeyBundle> {
        Ok(SecureMeshPairwisePreKeyBundle {
            endpoint_identity: self.device_identity()?,
            trust_state: DeviceTrustState::Verified,
            signed_prekey: SecureMeshPreKeyRecord {
                prekey_id: self.signed_prekey_id.clone(),
                public_key: decode_key_32(
                    &self.signed_prekey_public_key,
                    "mobile relay signed prekey public key",
                )?
                .to_vec(),
                signature: self.signed_prekey_signature.clone(),
                created_at: self.signed_prekey_created_at.clone(),
                expires_at: self.signed_prekey_expires_at.clone(),
            },
            one_time_prekey: Some(SecureMeshPreKeyRecord {
                prekey_id: self.one_time_prekey_id.clone(),
                public_key: decode_key_32(
                    &self.one_time_prekey_public_key,
                    "mobile relay one-time prekey public key",
                )?
                .to_vec(),
                signature: self.one_time_prekey_signature.clone(),
                created_at: self.one_time_prekey_created_at.clone(),
                expires_at: self.one_time_prekey_expires_at.clone(),
            }),
            one_time_mlkem1024_prekey: SecureMeshPreKeyRecord {
                prekey_id: self.one_time_mlkem1024_prekey_id.clone(),
                public_key: decode_fixed_base64url::<ML_KEM_1024_PUBLIC_KEY_BYTES>(
                    &self.one_time_mlkem1024_prekey_public_key,
                    "mobile relay ML-KEM-1024 one-time prekey public key",
                )?
                .to_vec(),
                signature: self.one_time_mlkem1024_prekey_signature.clone(),
                created_at: self.one_time_mlkem1024_prekey_created_at.clone(),
                expires_at: self.one_time_mlkem1024_prekey_expires_at.clone(),
            },
            prekey_publication_version: self.prekey_publication_version,
        })
    }

    pub(in crate::domain::mobile_relay) fn signed_prekey_secret(
        &self,
    ) -> Result<SecureMeshPairwisePrivateKey> {
        Ok(SecureMeshPairwisePrivateKey::from_bytes(decode_key_32(
            &self.signed_prekey_private_key,
            "mobile relay signed prekey private key",
        )?))
    }

    pub(in crate::domain::mobile_relay) fn one_time_prekey_secret_for(
        &self,
        requested_id: Option<&str>,
    ) -> Result<Option<SecureMeshPairwisePrivateKey>> {
        match requested_id {
            Some(id) if id == self.one_time_prekey_id => Ok(Some(
                SecureMeshPairwisePrivateKey::from_bytes(decode_key_32(
                    &self.one_time_prekey_private_key,
                    "mobile relay one-time prekey private key",
                )?),
            )),
            Some(_) => Err(anyhow!(
                "mobile relay one-time prekey secret does not match pairwise intro"
            )),
            None => Ok(None),
        }
    }

    pub(in crate::domain::mobile_relay) fn one_time_mlkem1024_prekey_seed_for(
        &self,
        requested_id: &str,
    ) -> Result<SecureMeshMlKem1024PreKeySeed> {
        ensure!(
            requested_id == self.one_time_mlkem1024_prekey_id,
            "mobile relay ML-KEM-1024 one-time prekey seed does not match pairwise intro"
        );
        Ok(SecureMeshMlKem1024PreKeySeed::from_bytes(
            decode_fixed_base64url::<ML_KEM_1024_KEY_GENERATION_SEED_BYTES>(
                &self.one_time_mlkem1024_prekey_seed,
                "mobile relay ML-KEM-1024 one-time prekey seed",
            )?,
        ))
    }
}
