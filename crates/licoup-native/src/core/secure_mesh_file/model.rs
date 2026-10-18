use super::constants::*;
use super::primitives::*;
use anyhow::{Context, Result, anyhow, ensure};
use rand_core::{OsRng, RngCore};
use serde::{Deserialize, Serialize};
use std::fmt;
use zeroize::Zeroizing;

use crate::core::secure_mesh_crypto::{SealedSecureMeshPayload, SecureMeshContentContext};

pub struct FileRootKey {
    bytes: Zeroizing<[u8; FILE_ROOT_KEY_BYTES]>,
}

impl FileRootKey {
    pub fn generate() -> Self {
        let mut bytes = Zeroizing::new([0u8; FILE_ROOT_KEY_BYTES]);
        OsRng.fill_bytes(bytes.as_mut());
        Self { bytes }
    }

    pub fn from_bytes(bytes: [u8; FILE_ROOT_KEY_BYTES]) -> Self {
        Self {
            bytes: Zeroizing::new(bytes),
        }
    }

    pub(super) fn as_bytes(&self) -> &[u8; FILE_ROOT_KEY_BYTES] {
        &self.bytes
    }
}

impl fmt::Debug for FileRootKey {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("FileRootKey([redacted])")
    }
}

pub struct FileKeyWrapSecret {
    bytes: Zeroizing<[u8; FILE_KEY_WRAP_SECRET_BYTES]>,
}

impl FileKeyWrapSecret {
    pub fn from_bytes(bytes: [u8; FILE_KEY_WRAP_SECRET_BYTES]) -> Self {
        Self {
            bytes: Zeroizing::new(bytes),
        }
    }

    pub(super) fn as_bytes(&self) -> &[u8; FILE_KEY_WRAP_SECRET_BYTES] {
        &self.bytes
    }
}

impl fmt::Debug for FileKeyWrapSecret {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("FileKeyWrapSecret([redacted])")
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FileKeyEnvelopeMode {
    PairwiseDevice,
    MlsEpoch,
}

impl FileKeyEnvelopeMode {
    pub(super) fn as_str(self) -> &'static str {
        match self {
            Self::PairwiseDevice => "pairwise_device",
            Self::MlsEpoch => "mls_epoch",
        }
    }

    pub(super) fn from_str(value: &str) -> Result<Self> {
        match value {
            "pairwise_device" => Ok(Self::PairwiseDevice),
            "mls_epoch" => Ok(Self::MlsEpoch),
            _ => Err(anyhow!("secure mesh file key envelope mode is unsupported")),
        }
    }
}

#[derive(Clone, Eq, PartialEq)]
pub(super) enum SecureMeshFileChannelBinding {
    PairwiseDevice,
    MlsEpoch { group_id: String, epoch: u64 },
}

#[derive(Clone, Eq, PartialEq)]
pub struct SecureMeshFileProtectionContext {
    pub(super) content_context: SecureMeshContentContext,
    pub(super) file_id: String,
    pub(super) chunk_count: u32,
    pub(super) file_hash: String,
    pub(super) expires_at_unix_seconds: u64,
    pub(super) channel: SecureMeshFileChannelBinding,
}

impl SecureMeshFileProtectionContext {
    pub fn for_pairwise_device(
        content_context: SecureMeshContentContext,
        file_id: impl Into<String>,
        chunk_count: u32,
        file_hash: impl Into<String>,
        expires_at_unix_seconds: u64,
    ) -> Result<Self> {
        let context = Self {
            content_context,
            file_id: file_id.into(),
            chunk_count,
            file_hash: file_hash.into(),
            expires_at_unix_seconds,
            channel: SecureMeshFileChannelBinding::PairwiseDevice,
        };
        context.validate()?;
        Ok(context)
    }

    #[allow(clippy::too_many_arguments)]
    pub fn for_mls_epoch(
        content_context: SecureMeshContentContext,
        file_id: impl Into<String>,
        chunk_count: u32,
        file_hash: impl Into<String>,
        group_id: impl Into<String>,
        epoch: u64,
        expires_at_unix_seconds: u64,
    ) -> Result<Self> {
        let context = Self {
            content_context,
            file_id: file_id.into(),
            chunk_count,
            file_hash: file_hash.into(),
            expires_at_unix_seconds,
            channel: SecureMeshFileChannelBinding::MlsEpoch {
                group_id: group_id.into(),
                epoch,
            },
        };
        context.validate()?;
        Ok(context)
    }

    pub fn content_context(&self) -> &SecureMeshContentContext {
        &self.content_context
    }

    pub fn file_id(&self) -> &str {
        &self.file_id
    }

    pub fn chunk_count(&self) -> u32 {
        self.chunk_count
    }

    pub fn file_hash(&self) -> &str {
        &self.file_hash
    }

    pub fn recipient_endpoint_id(&self) -> &str {
        &self.content_context.recipient_endpoint_id
    }

    pub fn expires_at_unix_seconds(&self) -> u64 {
        self.expires_at_unix_seconds
    }

    pub fn mls_epoch(&self) -> Option<u64> {
        match self.channel {
            SecureMeshFileChannelBinding::PairwiseDevice => None,
            SecureMeshFileChannelBinding::MlsEpoch { epoch, .. } => Some(epoch),
        }
    }

    pub(super) fn envelope_mode(&self) -> FileKeyEnvelopeMode {
        match self.channel {
            SecureMeshFileChannelBinding::PairwiseDevice => FileKeyEnvelopeMode::PairwiseDevice,
            SecureMeshFileChannelBinding::MlsEpoch { .. } => FileKeyEnvelopeMode::MlsEpoch,
        }
    }

    pub(super) fn validate(&self) -> Result<()> {
        validate_text("file crypto file_id", &self.file_id, MAX_FILE_NAME_BYTES)?;
        ensure!(
            self.chunk_count > 0 && self.chunk_count <= MAX_CHUNK_COUNT,
            "secure mesh file crypto chunk count is outside bounds"
        );
        validate_file_hash("file hash", &self.file_hash)?;
        validate_crypto_context_text(
            "file crypto sender endpoint",
            &self.content_context.sender_endpoint_id,
            MAX_FILE_CRYPTO_CONTEXT_BYTES,
        )?;
        validate_crypto_context_text(
            "file crypto recipient endpoint",
            &self.content_context.recipient_endpoint_id,
            MAX_FILE_CRYPTO_CONTEXT_BYTES,
        )?;
        validate_crypto_context_text(
            "file crypto session",
            &self.content_context.session_id,
            MAX_FILE_CRYPTO_CONTEXT_BYTES,
        )?;
        validate_text(
            "file crypto expiry",
            &self.content_context.expires_at,
            MAX_FILE_CRYPTO_CONTEXT_BYTES,
        )?;
        ensure!(
            self.expires_at_unix_seconds > 0,
            "secure mesh file crypto expiry is invalid"
        );
        if let SecureMeshFileChannelBinding::MlsEpoch { group_id, .. } = &self.channel {
            validate_crypto_context_text(
                "file crypto MLS group",
                group_id,
                MAX_FILE_CRYPTO_CONTEXT_BYTES,
            )?;
        }
        Ok(())
    }

    pub(super) fn ensure_not_expired(&self, now_unix_seconds: u64) -> Result<()> {
        ensure!(
            now_unix_seconds <= self.expires_at_unix_seconds,
            "secure mesh file key envelope is expired"
        );
        Ok(())
    }

    pub(super) fn same_transfer_as(&self, other: &Self) -> bool {
        self.file_id == other.file_id
            && self.chunk_count == other.chunk_count
            && self.file_hash == other.file_hash
            && self.content_context.sender_endpoint_id == other.content_context.sender_endpoint_id
            && self.expires_at_unix_seconds == other.expires_at_unix_seconds
    }
}

impl fmt::Debug for SecureMeshFileProtectionContext {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("SecureMeshFileProtectionContext")
            .field("file_id", &"[redacted]")
            .field("chunk_count", &self.chunk_count)
            .field("file_hash", &"[redacted]")
            .field("sender_endpoint_id", &"[redacted]")
            .field("recipient_endpoint_id", &"[redacted]")
            .field("channel", &self.envelope_mode())
            .field("expires_at_unix_seconds", &"[redacted]")
            .finish()
    }
}

#[derive(Clone, Eq, PartialEq)]
pub struct FileKeyEnvelope {
    pub(super) schema: String,
    pub(super) suite: String,
    pub(super) mode: FileKeyEnvelopeMode,
    pub(super) context_digest: String,
    pub(super) epoch: Option<u64>,
    pub(super) expires_at_unix_seconds: u64,
    pub(super) nonce: String,
    pub(super) ciphertext: String,
}

impl FileKeyEnvelope {
    pub fn to_json(&self) -> Result<String> {
        self.validate()?;
        let wire = FileKeyEnvelopeWire {
            schema: self.schema.clone(),
            suite: self.suite.clone(),
            mode: self.mode.as_str().to_string(),
            context_digest: self.context_digest.clone(),
            epoch: self.epoch,
            expires_at_unix_seconds: self.expires_at_unix_seconds,
            nonce: self.nonce.clone(),
            ciphertext: self.ciphertext.clone(),
        };
        serde_json::to_string(&wire).context("secure mesh file key envelope serialization failed")
    }

    pub fn from_json(value: &str) -> Result<Self> {
        ensure!(
            value.len() <= FILE_KEY_ENVELOPE_MAX_JSON_BYTES,
            "secure mesh file key envelope JSON is too large"
        );
        let wire: FileKeyEnvelopeWire =
            serde_json::from_str(value).context("secure mesh file key envelope JSON is invalid")?;
        let envelope = Self {
            schema: wire.schema,
            suite: wire.suite,
            mode: FileKeyEnvelopeMode::from_str(&wire.mode)?,
            context_digest: wire.context_digest,
            epoch: wire.epoch,
            expires_at_unix_seconds: wire.expires_at_unix_seconds,
            nonce: wire.nonce,
            ciphertext: wire.ciphertext,
        };
        envelope.validate()?;
        Ok(envelope)
    }

    pub fn mode(&self) -> FileKeyEnvelopeMode {
        self.mode
    }

    pub fn expires_at_unix_seconds(&self) -> u64 {
        self.expires_at_unix_seconds
    }

    pub(super) fn validate(&self) -> Result<()> {
        ensure!(
            self.schema == FILE_KEY_ENVELOPE_SCHEMA,
            "secure mesh file key envelope schema is unsupported"
        );
        ensure!(
            self.suite == SECURE_MESH_FILE_KEY_SUITE,
            "secure mesh file key envelope suite is unsupported"
        );
        decode_exact_base64url("file key context digest", &self.context_digest, 32)?;
        decode_exact_base64url("file key nonce", &self.nonce, FILE_KEY_ENVELOPE_NONCE_BYTES)?;
        decode_exact_base64url(
            "file key ciphertext",
            &self.ciphertext,
            FILE_KEY_ENVELOPE_FRAME_MAGIC.len()
                + FILE_ROOT_KEY_BYTES
                + 32
                + FILE_KEY_ENVELOPE_TAG_BYTES,
        )?;
        ensure!(
            self.expires_at_unix_seconds > 0,
            "secure mesh file key envelope expiry is invalid"
        );
        match self.mode {
            FileKeyEnvelopeMode::PairwiseDevice => ensure!(
                self.epoch.is_none(),
                "secure mesh pairwise file key envelope must not carry an MLS epoch"
            ),
            FileKeyEnvelopeMode::MlsEpoch => ensure!(
                self.epoch.is_some(),
                "secure mesh MLS file key envelope epoch is required"
            ),
        }
        Ok(())
    }
}

impl fmt::Debug for FileKeyEnvelope {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("FileKeyEnvelope")
            .field("schema", &self.schema)
            .field("suite", &self.suite)
            .field("mode", &self.mode)
            .field("context_digest", &"[redacted]")
            .field("epoch", &self.epoch)
            .field("expires_at_unix_seconds", &"[redacted]")
            .field("nonce", &"[redacted]")
            .field("ciphertext", &"[redacted]")
            .finish()
    }
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct FileKeyEnvelopeWire {
    pub(super) schema: String,
    pub(super) suite: String,
    pub(super) mode: String,
    pub(super) context_digest: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(super) epoch: Option<u64>,
    pub(super) expires_at_unix_seconds: u64,
    pub(super) nonce: String,
    pub(super) ciphertext: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AuthenticatedSecureMeshFileReceipt {
    pub chunk_index: u32,
    pub ciphertext_hash: String,
    pub authentication_tag: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SecureMeshFileManifest {
    pub file_id: String,
    pub file_name: String,
    pub mime_type: String,
    pub relative_path: String,
    pub total_size: u64,
    pub chunk_size: u32,
    pub chunk_count: u32,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SecureMeshFileChunk {
    pub file_id: String,
    pub chunk_index: u32,
    pub bytes: Vec<u8>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct EncryptedSecureMeshFileManifest {
    pub file_key_suite: String,
    pub file_aad_digest: String,
    pub sealed: SealedSecureMeshPayload,
    pub ciphertext_hash: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct EncryptedSecureMeshFileChunk {
    pub file_key_suite: String,
    pub file_aad_digest: String,
    pub file_id_hash: String,
    pub chunk_index: u32,
    pub chunk_hash: String,
    pub plaintext_size: usize,
    pub ciphertext_hash: String,
    pub sealed: SealedSecureMeshPayload,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SecureMeshFileChunkReceipt {
    pub chunk_index: u32,
    pub ciphertext_hash: String,
    pub plaintext_size: usize,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SecureMeshFileTransferState {
    pub file_id: String,
    pub file_id_hash: String,
    pub total_size: u64,
    pub chunk_size: u32,
    pub chunk_count: u32,
    pub received_chunks: Vec<Option<SecureMeshFileChunkReceipt>>,
    pub acknowledged_at: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SecureMeshFileResumeReport {
    pub received_chunk_count: usize,
    pub missing_chunk_indices: Vec<u32>,
    pub complete: bool,
    pub ack_required: bool,
    pub purge_local_ciphertext: bool,
}
