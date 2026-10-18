use anyhow::{Result, anyhow, ensure};
use hkdf::Hkdf;
use hmac::{Hmac, Mac};
use serde::{Deserialize, Serialize};
use sha2::Sha256;
use zeroize::Zeroizing;

use super::{
    constants::{
        AUTH_UPDATE_LABEL, CIPHERTEXT_MAC_LABEL, HEADER_MAC_LABEL, ML_KEM_BRAID_MAC_BYTES,
        PROTOCOL_INFO,
    },
    secret::SecretBytes,
};

type HmacSha256 = Hmac<Sha256>;

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct RatchetedAuthenticator {
    pub(super) root_key: SecretBytes,
    pub(super) mac_key: SecretBytes,
}

impl RatchetedAuthenticator {
    pub(super) fn initialize(epoch: u64, key: &[u8]) -> Result<Self> {
        let mut auth = Self {
            root_key: SecretBytes::new(vec![0u8; 32]),
            mac_key: SecretBytes::new(vec![0u8; 32]),
        };
        auth.update(epoch, key)?;
        Ok(auth)
    }

    pub(super) fn update(&mut self, epoch: u64, key: &[u8]) -> Result<()> {
        let mut info = Vec::with_capacity(PROTOCOL_INFO.len() + AUTH_UPDATE_LABEL.len() + 8);
        info.extend_from_slice(PROTOCOL_INFO);
        info.extend_from_slice(AUTH_UPDATE_LABEL);
        info.extend_from_slice(&epoch.to_be_bytes());
        let mut expanded = Zeroizing::new([0u8; 64]);
        Hkdf::<Sha256>::new(Some(self.root_key.as_slice()), key)
            .expand(&info, expanded.as_mut())
            .map_err(|_| anyhow!("ML-KEM Braid authenticator KDF failed"))?;
        self.root_key.0.copy_from_slice(&expanded[..32]);
        self.mac_key.0.copy_from_slice(&expanded[32..]);
        Ok(())
    }

    pub(super) fn mac_header(&self, epoch: u64, header: &[u8]) -> Result<[u8; 32]> {
        self.mac(HEADER_MAC_LABEL, epoch, header)
    }

    pub(super) fn mac_ciphertext(&self, epoch: u64, ciphertext: &[u8]) -> Result<[u8; 32]> {
        self.mac(CIPHERTEXT_MAC_LABEL, epoch, ciphertext)
    }

    pub(super) fn verify_header(&self, epoch: u64, header: &[u8], expected: &[u8]) -> Result<()> {
        self.verify(HEADER_MAC_LABEL, epoch, header, expected)
    }

    pub(super) fn verify_ciphertext(
        &self,
        epoch: u64,
        ciphertext: &[u8],
        expected: &[u8],
    ) -> Result<()> {
        self.verify(CIPHERTEXT_MAC_LABEL, epoch, ciphertext, expected)
    }

    pub(super) fn mac(&self, label: &[u8], epoch: u64, body: &[u8]) -> Result<[u8; 32]> {
        let mut mac = <HmacSha256 as Mac>::new_from_slice(self.mac_key.as_slice())
            .map_err(|_| anyhow!("ML-KEM Braid MAC key is invalid"))?;
        mac.update(PROTOCOL_INFO);
        mac.update(label);
        mac.update(&epoch.to_be_bytes());
        mac.update(body);
        let mut output = [0u8; 32];
        output.copy_from_slice(&mac.finalize().into_bytes());
        Ok(output)
    }

    pub(super) fn verify(
        &self,
        label: &[u8],
        epoch: u64,
        body: &[u8],
        expected: &[u8],
    ) -> Result<()> {
        ensure!(
            expected.len() == ML_KEM_BRAID_MAC_BYTES,
            "ML-KEM Braid MAC length is invalid"
        );
        let mut mac = <HmacSha256 as Mac>::new_from_slice(self.mac_key.as_slice())
            .map_err(|_| anyhow!("ML-KEM Braid MAC key is invalid"))?;
        mac.update(PROTOCOL_INFO);
        mac.update(label);
        mac.update(&epoch.to_be_bytes());
        mac.update(body);
        mac.verify_slice(expected)
            .map_err(|_| anyhow!("ML-KEM Braid authentication failed"))
    }

    pub(super) fn validate(&self) -> Result<()> {
        self.root_key.ensure_len(32)?;
        self.mac_key.ensure_len(32)
    }
}
