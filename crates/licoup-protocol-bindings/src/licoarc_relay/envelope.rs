//! Validated closed five-field Lico Arc envelope model.

use std::fmt;

use anyhow::Result;
use serde::Serialize;

use super::aad::licoarc_outer_authenticated_data;
use super::carrier::{DecodedCarrier, decode_carrier};
use super::codec::{validate_expires_at, validate_licoarc_id};
#[cfg(test)]
use super::constants::DELIVERY_ID_BYTES;
use super::constants::LICOARC_RELAY_CONTRACT_VERSION;
#[cfg(test)]
use super::draft::LicoArcRelayEnvelopeDraft;
#[cfg(test)]
use super::mailbox::SecureMeshMailboxToken;

#[derive(Clone, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct LicoArcRelayEnvelope {
    pub(in crate::licoarc_relay) contract_version: String,
    pub(in crate::licoarc_relay) envelope_id: String,
    pub(in crate::licoarc_relay) mailbox_id: String,
    pub(in crate::licoarc_relay) ciphertext: String,
    pub(in crate::licoarc_relay) expires_at: String,
}

impl LicoArcRelayEnvelope {
    #[cfg(test)]
    pub fn new(
        mailbox_id: &SecureMeshMailboxToken,
        expires_at: &str,
        encrypted_header: &[u8],
        content_ciphertext: &[u8],
    ) -> Result<Self> {
        LicoArcRelayEnvelopeDraft::begin(mailbox_id, expires_at, content_ciphertext.len())?
            .finish(encrypted_header, content_ciphertext)
    }

    pub fn contract_version(&self) -> &str {
        &self.contract_version
    }

    pub fn envelope_id(&self) -> &str {
        &self.envelope_id
    }

    pub fn mailbox_id(&self) -> &str {
        &self.mailbox_id
    }

    pub fn ciphertext(&self) -> &str {
        &self.ciphertext
    }

    pub fn expires_at(&self) -> &str {
        &self.expires_at
    }

    pub fn decoded_encrypted_header(&self) -> Result<Vec<u8>> {
        Ok(self.decode_validated_carrier()?.encrypted_header)
    }

    pub fn decoded_content_ciphertext(&self) -> Result<Vec<u8>> {
        Ok(self.decode_validated_carrier()?.content_ciphertext)
    }

    pub fn authenticated_outer_data(&self) -> Result<Vec<u8>> {
        let carrier = self.decode_validated_carrier()?;
        licoarc_outer_authenticated_data(
            &self.envelope_id,
            &self.mailbox_id,
            &self.expires_at,
            carrier.content_ciphertext.len(),
        )
    }

    pub(super) fn validate_contract_fields(&self) -> Result<()> {
        anyhow::ensure!(
            self.contract_version == LICOARC_RELAY_CONTRACT_VERSION,
            "Lico Arc relay contract version is unsupported"
        );
        validate_licoarc_id("envelopeId", &self.envelope_id)?;
        validate_licoarc_id("mailboxId", &self.mailbox_id)?;
        validate_expires_at(&self.expires_at)?;
        Ok(())
    }

    fn decode_validated_carrier(&self) -> Result<DecodedCarrier> {
        self.validate_contract_fields()?;
        decode_carrier(&self.ciphertext)
    }

    #[cfg(test)]
    pub(in crate::licoarc_relay) fn new_with_envelope_id(
        mailbox_id: &SecureMeshMailboxToken,
        expires_at: &str,
        encrypted_header: &[u8],
        content_ciphertext: &[u8],
        envelope_id: [u8; DELIVERY_ID_BYTES],
    ) -> Result<Self> {
        LicoArcRelayEnvelopeDraft::begin_with_envelope_id(
            mailbox_id,
            expires_at,
            content_ciphertext.len(),
            envelope_id,
        )?
        .finish(encrypted_header, content_ciphertext)
    }
}

impl fmt::Debug for LicoArcRelayEnvelope {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("LicoArcRelayEnvelope")
            .field("contract_version", &self.contract_version)
            .field("envelope_id", &"[redacted]")
            .field("mailbox_id", &"[redacted]")
            .field("ciphertext", &"[redacted]")
            .field("expires_at", &"[redacted]")
            .finish()
    }
}

impl TryFrom<&str> for LicoArcRelayEnvelope {
    type Error = anyhow::Error;

    fn try_from(value: &str) -> Result<Self> {
        Self::from_json(value)
    }
}
