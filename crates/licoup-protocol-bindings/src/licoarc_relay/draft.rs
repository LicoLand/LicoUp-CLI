//! Draft construction validates and binds Lico Arc metadata before encryption.

use std::fmt;

use anyhow::{Result, ensure};
use base64::{Engine as _, engine::general_purpose};
use rand_core::{OsRng, RngCore};

use super::aad::licoarc_outer_authenticated_data;
use super::carrier::{encode_carrier, preflight_carrier_size};
use super::codec::{validate_expires_at, validate_licoarc_id};
use super::constants::{
    DELIVERY_ID_BYTES, LICOARC_ENCRYPTED_HEADER_BYTES, LICOARC_RELAY_CONTRACT_VERSION,
};
use super::envelope::LicoArcRelayEnvelope;
use super::mailbox::SecureMeshMailboxToken;

#[derive(Clone, Eq, PartialEq)]
pub struct LicoArcRelayEnvelopeDraft {
    envelope_id: String,
    mailbox_id: String,
    expires_at: String,
    content_ciphertext_bytes: usize,
}

impl LicoArcRelayEnvelopeDraft {
    pub fn begin(
        mailbox_id: &SecureMeshMailboxToken,
        expires_at: &str,
        content_ciphertext_bytes: usize,
    ) -> Result<Self> {
        let mut envelope_id = [0u8; DELIVERY_ID_BYTES];
        OsRng.fill_bytes(&mut envelope_id);
        Self::begin_with_envelope_id(
            mailbox_id,
            expires_at,
            content_ciphertext_bytes,
            envelope_id,
        )
    }

    pub fn from_contract_fields(
        mailbox_id: &str,
        envelope_id: &str,
        expires_at: &str,
        content_ciphertext_bytes: usize,
    ) -> Result<Self> {
        validate_licoarc_id("mailboxId", mailbox_id)?;
        validate_licoarc_id("envelopeId", envelope_id)?;
        validate_expires_at(expires_at)?;
        preflight_carrier_size(content_ciphertext_bytes)?;
        let draft = Self {
            envelope_id: envelope_id.to_string(),
            mailbox_id: mailbox_id.to_string(),
            expires_at: expires_at.to_string(),
            content_ciphertext_bytes,
        };
        draft.authenticated_outer_data()?;
        Ok(draft)
    }

    pub fn authenticated_outer_data(&self) -> Result<Vec<u8>> {
        licoarc_outer_authenticated_data(
            &self.envelope_id,
            &self.mailbox_id,
            &self.expires_at,
            self.content_ciphertext_bytes,
        )
    }

    pub fn finish(
        self,
        encrypted_header: &[u8],
        content_ciphertext: &[u8],
    ) -> Result<LicoArcRelayEnvelope> {
        ensure!(
            encrypted_header.len() == LICOARC_ENCRYPTED_HEADER_BYTES,
            "Lico Arc encrypted header does not match the fixed carrier length"
        );
        ensure!(
            content_ciphertext.len() == self.content_ciphertext_bytes,
            "Lico Arc content ciphertext does not match the bound carrier length"
        );
        let envelope = LicoArcRelayEnvelope {
            contract_version: LICOARC_RELAY_CONTRACT_VERSION.to_string(),
            envelope_id: self.envelope_id,
            mailbox_id: self.mailbox_id,
            ciphertext: encode_carrier(encrypted_header, content_ciphertext)?,
            expires_at: self.expires_at,
        };
        envelope.validate()?;
        Ok(envelope)
    }

    pub(in crate::licoarc_relay) fn begin_with_envelope_id(
        mailbox_id: &SecureMeshMailboxToken,
        expires_at: &str,
        content_ciphertext_bytes: usize,
        envelope_id: [u8; DELIVERY_ID_BYTES],
    ) -> Result<Self> {
        Self::from_contract_fields(
            mailbox_id.as_str(),
            &general_purpose::URL_SAFE_NO_PAD.encode(envelope_id),
            expires_at,
            content_ciphertext_bytes,
        )
    }
}

impl fmt::Debug for LicoArcRelayEnvelopeDraft {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("LicoArcRelayEnvelopeDraft")
            .field("envelope_id", &"[redacted]")
            .field("mailbox_id", &"[redacted]")
            .field("expires_at", &"[redacted]")
            .field("content_ciphertext_bytes", &self.content_ciphertext_bytes)
            .finish()
    }
}
