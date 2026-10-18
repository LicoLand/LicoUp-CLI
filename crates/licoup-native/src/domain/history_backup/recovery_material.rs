use std::collections::BTreeMap;

use chacha20poly1305::{
    ChaCha20Poly1305, Key, KeyInit, Nonce,
    aead::{Aead, Payload},
};
use hkdf::Hkdf;
use rand_core::{OsRng, RngCore};
use serde::{Deserialize, Serialize};
use sha2::Sha256;
use thiserror::Error;
use zeroize::{Zeroize, ZeroizeOnDrop, Zeroizing};

use super::policy::HistoryKeySource;

const PACKAGE_SALT: &[u8] = b"licoup.history-backup.recovery-package.v1";
const IDENTITY_DOMAIN: &[u8] = b"licoup.history-backup.identity-authority.v1";
const KEY_RING_DOMAIN: &[u8] = b"licoup.history-backup.history-key-ring.v1";

#[derive(Clone, Zeroize, ZeroizeOnDrop)]
pub struct RecoverySecret([u8; 32]);

impl RecoverySecret {
    pub fn from_bytes(bytes: [u8; 32]) -> Self {
        Self(bytes)
    }
}

#[derive(Clone, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct WrappedSection {
    nonce: [u8; 12],
    ciphertext: Vec<u8>,
}

#[derive(Clone, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RecoveryPackage {
    format_version: u16,
    identity_authority: WrappedSection,
    history_key_ring: WrappedSection,
}

#[derive(Clone, Eq, PartialEq)]
pub struct RecoveryMaterial {
    pub identity_authority: Zeroizing<Vec<u8>>,
    pub history_keys: BTreeMap<u64, [u8; 32]>,
}

impl Drop for RecoveryMaterial {
    fn drop(&mut self) {
        for key in self.history_keys.values_mut() {
            key.zeroize();
        }
        self.history_keys.clear();
    }
}

impl HistoryKeySource for RecoveryMaterial {
    fn key_for_generation(&self, generation: u64) -> Option<Zeroizing<[u8; 32]>> {
        self.history_keys
            .get(&generation)
            .copied()
            .map(Zeroizing::new)
    }
}

#[derive(Clone, Copy, Debug, Eq, Error, PartialEq)]
pub enum RecoveryMaterialError {
    #[error("identity recovery material is required")]
    IdentityAuthorityRequired,
    #[error("recovery package is invalid")]
    InvalidPackage,
    #[error("recovery secret was rejected")]
    SecretRejected,
}

impl RecoveryPackage {
    pub fn wrap(
        secret: &RecoverySecret,
        identity_authority: &[u8],
        history_keys: &BTreeMap<u64, [u8; 32]>,
    ) -> Result<Self, RecoveryMaterialError> {
        if identity_authority.is_empty() {
            return Err(RecoveryMaterialError::IdentityAuthorityRequired);
        }
        let key_ring = Zeroizing::new(
            serde_json::to_vec(history_keys).map_err(|_| RecoveryMaterialError::InvalidPackage)?,
        );
        Ok(Self {
            format_version: 1,
            identity_authority: wrap_section(secret, IDENTITY_DOMAIN, identity_authority)?,
            history_key_ring: wrap_section(secret, KEY_RING_DOMAIN, &key_ring)?,
        })
    }

    pub fn open(&self, secret: &RecoverySecret) -> Result<RecoveryMaterial, RecoveryMaterialError> {
        if self.format_version != 1 {
            return Err(RecoveryMaterialError::InvalidPackage);
        }
        let identity_authority = open_section(secret, IDENTITY_DOMAIN, &self.identity_authority)?;
        if identity_authority.is_empty() {
            return Err(RecoveryMaterialError::IdentityAuthorityRequired);
        }
        let key_ring = open_section(secret, KEY_RING_DOMAIN, &self.history_key_ring)?;
        let history_keys =
            serde_json::from_slice(&key_ring).map_err(|_| RecoveryMaterialError::InvalidPackage)?;
        Ok(RecoveryMaterial {
            identity_authority,
            history_keys,
        })
    }

    #[cfg(test)]
    pub(crate) fn synthetic_without_identity(
        secret: &RecoverySecret,
        history_keys: &BTreeMap<u64, [u8; 32]>,
    ) -> Self {
        let key_ring = Zeroizing::new(
            serde_json::to_vec(history_keys).expect("synthetic key ring serializes"),
        );
        Self {
            format_version: 1,
            identity_authority: wrap_section(secret, IDENTITY_DOMAIN, &[])
                .expect("synthetic section wraps"),
            history_key_ring: wrap_section(secret, KEY_RING_DOMAIN, &key_ring)
                .expect("synthetic section wraps"),
        }
    }
}

fn wrap_section(
    secret: &RecoverySecret,
    domain: &[u8],
    plaintext: &[u8],
) -> Result<WrappedSection, RecoveryMaterialError> {
    let mut nonce = [0_u8; 12];
    OsRng.fill_bytes(&mut nonce);
    let key = derive_section_key(secret, domain);
    let ciphertext = ChaCha20Poly1305::new(Key::from_slice(&key[..]))
        .encrypt(
            Nonce::from_slice(&nonce),
            Payload {
                msg: plaintext,
                aad: domain,
            },
        )
        .map_err(|_| RecoveryMaterialError::InvalidPackage)?;
    Ok(WrappedSection { nonce, ciphertext })
}

fn open_section(
    secret: &RecoverySecret,
    domain: &[u8],
    section: &WrappedSection,
) -> Result<Zeroizing<Vec<u8>>, RecoveryMaterialError> {
    let key = derive_section_key(secret, domain);
    ChaCha20Poly1305::new(Key::from_slice(&key[..]))
        .decrypt(
            Nonce::from_slice(&section.nonce),
            Payload {
                msg: &section.ciphertext,
                aad: domain,
            },
        )
        .map(Zeroizing::new)
        .map_err(|_| RecoveryMaterialError::SecretRejected)
}

fn derive_section_key(secret: &RecoverySecret, domain: &[u8]) -> Zeroizing<[u8; 32]> {
    let hkdf = Hkdf::<Sha256>::new(Some(PACKAGE_SALT), &secret.0);
    let mut output = Zeroizing::new([0_u8; 32]);
    hkdf.expand(domain, &mut output[..])
        .expect("fixed-size HKDF output is valid");
    output
}
