//! Canonical binary carrier for LicoUp-owned protected payload parts.

use anyhow::{Context, Result, anyhow, ensure};
use base64::{Engine as _, engine::general_purpose};

use super::codec::base64url_encoded_len;
use super::constants::{
    CARRIER_LENGTH_BYTES, CARRIER_MAGIC, CARRIER_PREFIX_BYTES, CARRIER_VERSION,
    LICOARC_ENCRYPTED_HEADER_BYTES, LICOARC_MAX_CIPHERTEXT_CHARS,
};
use crate::validate_authenticated_padding_bucket;

pub(super) struct DecodedCarrier {
    pub(super) encrypted_header: Vec<u8>,
    pub(super) content_ciphertext: Vec<u8>,
}

pub(super) fn preflight_carrier_size(content_ciphertext_bytes: usize) -> Result<usize> {
    validate_authenticated_padding_bucket(content_ciphertext_bytes)?;
    let frame_bytes = CARRIER_PREFIX_BYTES
        .checked_add(LICOARC_ENCRYPTED_HEADER_BYTES)
        .and_then(|size| size.checked_add(content_ciphertext_bytes))
        .ok_or_else(|| anyhow!("Lico Arc carrier length overflow"))?;
    ensure!(
        base64url_encoded_len(frame_bytes)? <= LICOARC_MAX_CIPHERTEXT_CHARS,
        "Lico Arc carrier exceeds the ciphertext character limit"
    );
    u32::try_from(LICOARC_ENCRYPTED_HEADER_BYTES)
        .map_err(|_| anyhow!("Lico Arc encrypted-header length is outside framing bounds"))?;
    u32::try_from(content_ciphertext_bytes)
        .map_err(|_| anyhow!("Lico Arc content ciphertext length is outside framing bounds"))?;
    Ok(frame_bytes)
}

pub(super) fn encode_carrier(encrypted_header: &[u8], content_ciphertext: &[u8]) -> Result<String> {
    ensure!(
        encrypted_header.len() == LICOARC_ENCRYPTED_HEADER_BYTES,
        "Lico Arc encrypted header does not match the fixed carrier length"
    );
    let frame_bytes = preflight_carrier_size(content_ciphertext.len())?;
    let header_length = u32::try_from(encrypted_header.len())
        .map_err(|_| anyhow!("Lico Arc encrypted-header length is outside framing bounds"))?;
    let ciphertext_length = u32::try_from(content_ciphertext.len())
        .map_err(|_| anyhow!("Lico Arc content ciphertext length is outside framing bounds"))?;
    let mut frame = Vec::with_capacity(frame_bytes);
    frame.extend_from_slice(CARRIER_MAGIC);
    frame.push(CARRIER_VERSION);
    frame.extend_from_slice(&header_length.to_be_bytes());
    frame.extend_from_slice(&ciphertext_length.to_be_bytes());
    frame.extend_from_slice(encrypted_header);
    frame.extend_from_slice(content_ciphertext);
    ensure!(
        frame.len() == frame_bytes,
        "Lico Arc carrier encoder length mismatch"
    );
    let encoded = general_purpose::URL_SAFE_NO_PAD.encode(frame);
    ensure!(
        encoded.len() <= LICOARC_MAX_CIPHERTEXT_CHARS,
        "Lico Arc carrier exceeds the ciphertext character limit"
    );
    Ok(encoded)
}

pub(super) fn decode_carrier(encoded: &str) -> Result<DecodedCarrier> {
    ensure!(
        !encoded.is_empty() && encoded.len() <= LICOARC_MAX_CIPHERTEXT_CHARS,
        "Lico Arc ciphertext is outside encoded bounds"
    );
    let minimum_frame_bytes = CARRIER_PREFIX_BYTES
        .checked_add(LICOARC_ENCRYPTED_HEADER_BYTES)
        .ok_or_else(|| anyhow!("Lico Arc carrier minimum length overflow"))?;
    ensure!(
        encoded.len() >= base64url_encoded_len(minimum_frame_bytes)?,
        "Lico Arc carrier is truncated"
    );
    let decoded = general_purpose::URL_SAFE_NO_PAD
        .decode(encoded)
        .context("Lico Arc ciphertext is not base64url")?;
    ensure!(
        general_purpose::URL_SAFE_NO_PAD.encode(&decoded) == encoded,
        "Lico Arc ciphertext is not canonical base64url"
    );
    ensure!(
        decoded.len() >= minimum_frame_bytes && decoded.starts_with(CARRIER_MAGIC),
        "Lico Arc carrier framing is invalid"
    );
    ensure!(
        decoded[CARRIER_MAGIC.len()] == CARRIER_VERSION,
        "Lico Arc carrier version is unsupported"
    );
    let mut offset = CARRIER_MAGIC.len() + 1;
    let header_length = read_u32(&decoded, &mut offset, "encrypted-header")?;
    let ciphertext_length = read_u32(&decoded, &mut offset, "content-ciphertext")?;
    ensure!(
        header_length == LICOARC_ENCRYPTED_HEADER_BYTES,
        "Lico Arc encrypted-header length is invalid"
    );
    preflight_carrier_size(ciphertext_length)?;
    let header_end = offset
        .checked_add(header_length)
        .ok_or_else(|| anyhow!("Lico Arc encrypted-header offset overflow"))?;
    let ciphertext_end = header_end
        .checked_add(ciphertext_length)
        .ok_or_else(|| anyhow!("Lico Arc content-ciphertext offset overflow"))?;
    ensure!(
        ciphertext_end == decoded.len(),
        "Lico Arc carrier length does not match its framing"
    );
    Ok(DecodedCarrier {
        encrypted_header: decoded[offset..header_end].to_vec(),
        content_ciphertext: decoded[header_end..ciphertext_end].to_vec(),
    })
}

fn read_u32(input: &[u8], offset: &mut usize, label: &str) -> Result<usize> {
    let end = offset
        .checked_add(CARRIER_LENGTH_BYTES)
        .ok_or_else(|| anyhow!("Lico Arc {label} length offset overflow"))?;
    let value = input
        .get(*offset..end)
        .ok_or_else(|| anyhow!("Lico Arc carrier is truncated"))?;
    *offset = end;
    usize::try_from(u32::from_be_bytes(
        value
            .try_into()
            .map_err(|_| anyhow!("Lico Arc {label} length is invalid"))?,
    ))
    .map_err(|_| anyhow!("Lico Arc {label} length is outside platform bounds"))
}
