use anyhow::{Context, Result, ensure};
use base64::{Engine as _, engine::general_purpose};
use sha2::{Digest, Sha256};

use super::constants::{
    AAD_HASH_LEN, CONTENT_NONCE_LEN, HEADER_MAGIC, PRIVATE_CONTEXT_AEAD_AAD,
    PRIVATE_CONTEXT_HEADER_MAGIC,
};

pub(super) fn encode_header(nonce: &[u8; CONTENT_NONCE_LEN], aad_hash: &[u8]) -> String {
    let mut out = Vec::with_capacity(HEADER_MAGIC.len() + CONTENT_NONCE_LEN + AAD_HASH_LEN);
    out.extend_from_slice(HEADER_MAGIC);
    out.extend_from_slice(nonce);
    out.extend_from_slice(aad_hash);
    general_purpose::URL_SAFE_NO_PAD.encode(out)
}

pub(super) fn encode_private_context_header(nonce: &[u8; CONTENT_NONCE_LEN]) -> String {
    let mut out =
        Vec::with_capacity(PRIVATE_CONTEXT_HEADER_MAGIC.len() + CONTENT_NONCE_LEN + AAD_HASH_LEN);
    out.extend_from_slice(PRIVATE_CONTEXT_HEADER_MAGIC);
    out.extend_from_slice(nonce);
    out.extend_from_slice(&Sha256::digest(PRIVATE_CONTEXT_AEAD_AAD));
    general_purpose::URL_SAFE_NO_PAD.encode(out)
}

pub(super) fn decode_private_context_header(value: &str) -> Result<[u8; CONTENT_NONCE_LEN]> {
    let expected_size = PRIVATE_CONTEXT_HEADER_MAGIC.len() + CONTENT_NONCE_LEN + AAD_HASH_LEN;
    let bytes = decode_canonical_base64url(
        "private-context encrypted header",
        value,
        expected_size,
        expected_size,
    )?;
    ensure!(
        bytes.starts_with(PRIVATE_CONTEXT_HEADER_MAGIC),
        "secure mesh private-context encrypted header magic is invalid"
    );
    let nonce_start = PRIVATE_CONTEXT_HEADER_MAGIC.len();
    let mut nonce = [0u8; CONTENT_NONCE_LEN];
    nonce.copy_from_slice(&bytes[nonce_start..nonce_start + CONTENT_NONCE_LEN]);
    let aad_hash_start = nonce_start + CONTENT_NONCE_LEN;
    ensure!(
        &bytes[aad_hash_start..] == Sha256::digest(PRIVATE_CONTEXT_AEAD_AAD).as_slice(),
        "secure mesh private-context encrypted header profile hash mismatch"
    );
    Ok(nonce)
}

pub(super) fn decode_canonical_base64url(
    label: &str,
    value: &str,
    minimum_decoded_bytes: usize,
    maximum_decoded_bytes: usize,
) -> Result<Vec<u8>> {
    ensure!(
        minimum_decoded_bytes <= maximum_decoded_bytes,
        "secure mesh base64url decoder bounds are invalid"
    );
    ensure!(
        !value.is_empty() && value.len() <= encoded_len_limit(maximum_decoded_bytes),
        "secure mesh {label} is outside encoded bounds"
    );
    let bytes = general_purpose::URL_SAFE_NO_PAD
        .decode(value)
        .with_context(|| format!("secure mesh {label} is not base64url"))?;
    ensure!(
        (minimum_decoded_bytes..=maximum_decoded_bytes).contains(&bytes.len()),
        "secure mesh {label} is outside decoded bounds"
    );
    ensure!(
        general_purpose::URL_SAFE_NO_PAD.encode(&bytes) == value,
        "secure mesh {label} is not canonical base64url"
    );
    Ok(bytes)
}

pub(super) fn decode_header(value: &str) -> Result<([u8; CONTENT_NONCE_LEN], [u8; AAD_HASH_LEN])> {
    ensure!(
        value.len() <= encoded_len_limit(HEADER_MAGIC.len() + CONTENT_NONCE_LEN + AAD_HASH_LEN),
        "secure mesh payload encrypted header is too large"
    );
    let bytes = general_purpose::URL_SAFE_NO_PAD
        .decode(value)
        .context("secure mesh payload encrypted header is not base64url")?;
    ensure!(
        bytes.len() == HEADER_MAGIC.len() + CONTENT_NONCE_LEN + AAD_HASH_LEN,
        "secure mesh payload encrypted header length is invalid"
    );
    ensure!(
        bytes.starts_with(HEADER_MAGIC),
        "secure mesh payload encrypted header magic is invalid"
    );
    let mut nonce = [0u8; CONTENT_NONCE_LEN];
    let nonce_start = HEADER_MAGIC.len();
    nonce.copy_from_slice(&bytes[nonce_start..nonce_start + CONTENT_NONCE_LEN]);
    let mut aad_hash = [0u8; AAD_HASH_LEN];
    let hash_start = nonce_start + CONTENT_NONCE_LEN;
    aad_hash.copy_from_slice(&bytes[hash_start..hash_start + AAD_HASH_LEN]);
    Ok((nonce, aad_hash))
}

pub(super) const fn encoded_len_limit(decoded_bytes: usize) -> usize {
    decoded_bytes.div_ceil(3) * 4
}
