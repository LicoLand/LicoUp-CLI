use anyhow::{Result, anyhow, ensure};
use zeroize::Zeroizing;

use super::constants::{AEAD_TAG_LEN, PADDED_PLAINTEXT_MAGIC};
use licoup_protocol_bindings::{
    LARGE_PADDING_BUCKET_STEP_BYTES, MAX_PADDING_BUCKET_BYTES, MIN_PADDING_BUCKET_BYTES,
    POWER_OF_TWO_PADDING_LIMIT_BYTES,
};

pub(super) fn padding_bucket_for_ciphertext_size(unpadded_plaintext_size: usize) -> Result<usize> {
    let framed_size = PADDED_PLAINTEXT_MAGIC
        .len()
        .checked_add(4)
        .and_then(|size| size.checked_add(unpadded_plaintext_size))
        .and_then(|size| size.checked_add(AEAD_TAG_LEN))
        .ok_or_else(|| anyhow!("secure mesh padded payload length overflow"))?;
    let bucket = if framed_size <= POWER_OF_TWO_PADDING_LIMIT_BYTES {
        framed_size
            .max(MIN_PADDING_BUCKET_BYTES)
            .checked_next_power_of_two()
            .ok_or_else(|| anyhow!("secure mesh padding bucket overflow"))?
    } else {
        framed_size
            .checked_add(LARGE_PADDING_BUCKET_STEP_BYTES - 1)
            .ok_or_else(|| anyhow!("secure mesh padding bucket overflow"))?
            / LARGE_PADDING_BUCKET_STEP_BYTES
            * LARGE_PADDING_BUCKET_STEP_BYTES
    };
    ensure!(
        bucket <= MAX_PADDING_BUCKET_BYTES,
        "secure mesh payload exceeds the maximum padding bucket"
    );
    Ok(bucket)
}

pub(super) fn add_bucket_padding(encoded_plaintext: &[u8]) -> Result<Zeroizing<Vec<u8>>> {
    let ciphertext_bucket = padding_bucket_for_ciphertext_size(encoded_plaintext.len())?;
    let padded_plaintext_size = ciphertext_bucket
        .checked_sub(AEAD_TAG_LEN)
        .ok_or_else(|| anyhow!("secure mesh padding bucket is invalid"))?;
    let original_len = u32::try_from(encoded_plaintext.len())
        .map_err(|_| anyhow!("secure mesh payload is too large to pad"))?;
    let mut padded = Zeroizing::new(Vec::with_capacity(padded_plaintext_size));
    padded.extend_from_slice(PADDED_PLAINTEXT_MAGIC);
    padded.extend_from_slice(&original_len.to_be_bytes());
    padded.extend_from_slice(encoded_plaintext);
    padded.resize(padded_plaintext_size, 0);
    Ok(padded)
}

pub(super) fn remove_authenticated_padding(padded_plaintext: &[u8]) -> Result<&[u8]> {
    let prefix_len = PADDED_PLAINTEXT_MAGIC.len() + 4;
    ensure!(
        padded_plaintext.len() >= prefix_len,
        "secure mesh padded payload is truncated"
    );
    ensure!(
        padded_plaintext.starts_with(PADDED_PLAINTEXT_MAGIC),
        "secure mesh padded payload magic is invalid"
    );
    let original_len = u32::from_be_bytes(
        padded_plaintext[PADDED_PLAINTEXT_MAGIC.len()..prefix_len]
            .try_into()
            .map_err(|_| anyhow!("secure mesh padded payload length is invalid"))?,
    ) as usize;
    let end = prefix_len
        .checked_add(original_len)
        .ok_or_else(|| anyhow!("secure mesh padded payload length overflow"))?;
    ensure!(
        end <= padded_plaintext.len(),
        "secure mesh padded payload length is invalid"
    );
    ensure!(
        padded_plaintext[end..].iter().all(|byte| *byte == 0),
        "secure mesh padded payload bytes are invalid"
    );
    Ok(&padded_plaintext[prefix_len..end])
}
