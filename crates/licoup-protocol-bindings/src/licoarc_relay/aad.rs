//! Canonical associated data binds the complete Lico Arc routing context.

use anyhow::{Result, anyhow};

use super::carrier::preflight_carrier_size;
use super::codec::{append_len_prefixed, validate_expires_at, validate_licoarc_id};
use super::constants::{
    CARRIER_MAGIC, CARRIER_VERSION, LICOARC_ENCRYPTED_HEADER_BYTES, LICOARC_RELAY_CONTRACT_VERSION,
    OUTER_AAD_MAGIC,
};

pub(in crate::licoarc_relay) fn licoarc_outer_authenticated_data(
    envelope_id: &str,
    mailbox_id: &str,
    expires_at: &str,
    content_ciphertext_bytes: usize,
) -> Result<Vec<u8>> {
    validate_licoarc_id("envelopeId", envelope_id)?;
    validate_licoarc_id("mailboxId", mailbox_id)?;
    validate_expires_at(expires_at)?;
    preflight_carrier_size(content_ciphertext_bytes)?;
    let encrypted_header_bytes = u32::try_from(LICOARC_ENCRYPTED_HEADER_BYTES)
        .map_err(|_| anyhow!("Lico Arc encrypted-header length is outside framing bounds"))?;
    let content_ciphertext_bytes = u32::try_from(content_ciphertext_bytes)
        .map_err(|_| anyhow!("Lico Arc content ciphertext length is outside framing bounds"))?;
    let mut aad = Vec::with_capacity(256);
    append_len_prefixed(&mut aad, OUTER_AAD_MAGIC)?;
    append_len_prefixed(&mut aad, LICOARC_RELAY_CONTRACT_VERSION.as_bytes())?;
    append_len_prefixed(&mut aad, envelope_id.as_bytes())?;
    append_len_prefixed(&mut aad, mailbox_id.as_bytes())?;
    append_len_prefixed(&mut aad, expires_at.as_bytes())?;
    append_len_prefixed(&mut aad, CARRIER_MAGIC)?;
    aad.push(CARRIER_VERSION);
    aad.extend_from_slice(&encrypted_header_bytes.to_be_bytes());
    aad.extend_from_slice(&content_ciphertext_bytes.to_be_bytes());
    Ok(aad)
}
