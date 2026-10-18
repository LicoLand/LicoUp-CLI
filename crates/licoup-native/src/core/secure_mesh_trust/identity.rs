use super::codec::{append_len_prefixed_bytes, hash_bytes};
use super::{DEVICE_IDENTITY_MAGIC, PUBLIC_KEY_LEN, SECURE_MESH_DEVICE_TRUST_PROTOCOL_VERSION};
use anyhow::{Result, anyhow, ensure};
use ed25519_dalek::VerifyingKey;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DeviceTrustPublicIdentity {
    pub endpoint_id: String,
    pub identity_public_key: [u8; PUBLIC_KEY_LEN],
    pub signing_public_key: [u8; PUBLIC_KEY_LEN],
    pub rotation_epoch: u64,
}

impl DeviceTrustPublicIdentity {
    pub fn new(
        endpoint_id: impl Into<String>,
        identity_public_key: [u8; PUBLIC_KEY_LEN],
        signing_public_key: [u8; PUBLIC_KEY_LEN],
        rotation_epoch: u64,
    ) -> Result<Self> {
        let value = Self {
            endpoint_id: endpoint_id.into(),
            identity_public_key,
            signing_public_key,
            rotation_epoch,
        };
        value.validate()?;
        Ok(value)
    }

    pub fn signing_verifying_key(&self) -> Result<VerifyingKey> {
        VerifyingKey::from_bytes(&self.signing_public_key)
            .map_err(|error| anyhow!("secure mesh signing public key is invalid: {error:?}"))
    }

    pub fn fingerprint(&self) -> Result<String> {
        Ok(hash_bytes(&self.canonical_bytes()?))
    }

    pub(super) fn validate(&self) -> Result<()> {
        ensure!(
            !self.endpoint_id.trim().is_empty(),
            "secure mesh endpoint id is required"
        );
        ensure!(
            self.endpoint_id.len() <= 255,
            "secure mesh endpoint id is too large"
        );
        Ok(())
    }

    pub(super) fn canonical_bytes(&self) -> Result<Vec<u8>> {
        let mut out = Vec::new();
        out.extend_from_slice(DEVICE_IDENTITY_MAGIC);
        append_len_prefixed_bytes(
            &mut out,
            SECURE_MESH_DEVICE_TRUST_PROTOCOL_VERSION.as_bytes(),
        )?;
        append_len_prefixed_bytes(&mut out, self.endpoint_id.as_bytes())?;
        out.extend_from_slice(&self.rotation_epoch.to_be_bytes());
        append_len_prefixed_bytes(&mut out, &self.identity_public_key)?;
        append_len_prefixed_bytes(&mut out, &self.signing_public_key)?;
        Ok(out)
    }
}

pub fn detect_identity_key_change(
    previous: &DeviceTrustPublicIdentity,
    current: &DeviceTrustPublicIdentity,
) -> Result<super::model::DeviceTrustState> {
    ensure!(
        previous.endpoint_id == current.endpoint_id,
        "secure mesh device key-change check endpoint mismatch"
    );
    if previous.fingerprint()? == current.fingerprint()? {
        Ok(super::model::DeviceTrustState::Verified)
    } else {
        Ok(super::model::DeviceTrustState::KeyChanged)
    }
}
