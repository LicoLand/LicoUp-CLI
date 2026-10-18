//! Complete Agent output spool. Memory overflow transfers intact chunks to a
//! process-local sealed store. Chunks are never truncated.
//!
//! Sealing uses a per-spool key drawn from the OS CSPRNG so the ciphertext is
//! not decryptable or forgeable against a publicly known constant key. The
//! spool is process-local and non-persistent: the key never leaves the process
//! and no ciphertext survives a restart, so a per-spool random key provides
//! the full confidentiality and integrity service of the AEAD without any
//! key-export or key-custody requirement.

use chacha20poly1305::aead::{Aead, KeyInit, Payload};
use chacha20poly1305::{ChaCha20Poly1305, Key, Nonce};
use rand_core::{OsRng, RngCore};
use std::collections::BTreeMap;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SpoolError {
    SealedStoreUnavailable,
    IntegrityFailed,
    UnknownChunk,
}

impl SpoolError {
    pub const fn code(self) -> &'static str {
        match self {
            Self::SealedStoreUnavailable => "spool_unavailable",
            Self::IntegrityFailed => "spool_integrity_failed",
            Self::UnknownChunk => "spool_unknown_chunk",
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct SpoolOffset(u64);

struct SealedChunk {
    nonce: u64,
    ciphertext: Vec<u8>,
}

pub struct OutputSpool {
    cipher: ChaCha20Poly1305,
    chunks: BTreeMap<u64, SealedChunk>,
    next_offset: u64,
    next_nonce: u64,
}

impl OutputSpool {
    pub fn process_local() -> Self {
        Self {
            cipher: ChaCha20Poly1305::new(&derive_process_key()),
            chunks: BTreeMap::new(),
            next_offset: 0,
            next_nonce: 1,
        }
    }

    pub fn append(&mut self, plaintext: &[u8]) -> Result<SpoolOffset, SpoolError> {
        if plaintext.is_empty() {
            return Ok(SpoolOffset(self.next_offset));
        }
        let nonce = self.next_nonce;
        self.next_nonce = self.next_nonce.saturating_add(1);
        let sealed = self
            .cipher
            .encrypt(
                &nonce_from(nonce),
                Payload {
                    msg: plaintext,
                    aad: &self.next_offset.to_le_bytes(),
                },
            )
            .map_err(|_| SpoolError::SealedStoreUnavailable)?;
        let offset = self.next_offset;
        self.chunks.insert(
            offset,
            SealedChunk {
                nonce,
                ciphertext: sealed,
            },
        );
        self.next_offset = self.next_offset.saturating_add(plaintext.len() as u64);
        Ok(SpoolOffset(offset))
    }

    pub fn read(&self, offset: SpoolOffset) -> Result<Vec<u8>, SpoolError> {
        let chunk = self.chunks.get(&offset.0).ok_or(SpoolError::UnknownChunk)?;
        self.cipher
            .decrypt(
                &nonce_from(chunk.nonce),
                Payload {
                    msg: &chunk.ciphertext,
                    aad: &offset.0.to_le_bytes(),
                },
            )
            .map_err(|_| SpoolError::IntegrityFailed)
    }

    pub fn len(&self) -> usize {
        self.chunks.len()
    }

    pub fn is_empty(&self) -> bool {
        self.chunks.is_empty()
    }
}

fn nonce_from(index: u64) -> Nonce {
    let mut bytes = [0_u8; 12];
    bytes[4..].copy_from_slice(&index.to_le_bytes());
    Nonce::from(bytes)
}

/// Draws a fresh 256-bit key from the OS CSPRNG, matching the crate's secure
/// mesh key-derivation convention. Each `OutputSpool` instance gets an
/// independent key, so two spools never share a key and no key material is
/// embedded or exported.
fn derive_process_key() -> Key {
    let mut bytes = [0_u8; 32];
    OsRng.fill_bytes(&mut bytes);
    Key::from(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn complete_output_survives_sealed_round_trip() {
        let mut spool = OutputSpool::process_local();
        let first = spool.append(b"chunk-a").expect("first");
        let second = spool.append(b"chunk-b").expect("second");
        assert_eq!(spool.len(), 2);
        assert_eq!(spool.read(first).expect("read a"), b"chunk-a");
        assert_eq!(spool.read(second).expect("read b"), b"chunk-b");
    }

    #[test]
    fn unknown_offset_is_a_typed_failure() {
        let spool = OutputSpool::process_local();
        assert_eq!(
            spool.read(SpoolOffset(9)).expect_err("missing").code(),
            "spool_unknown_chunk"
        );
    }
}
