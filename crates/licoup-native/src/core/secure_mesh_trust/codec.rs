use anyhow::{Result, anyhow};
use base64::{Engine as _, engine::general_purpose};
use sha2::{Digest, Sha256};

pub(super) fn append_len_prefixed_bytes(out: &mut Vec<u8>, value: &[u8]) -> Result<()> {
    let len = u32::try_from(value.len())
        .map_err(|_| anyhow!("secure mesh device trust field is too large"))?;
    out.extend_from_slice(&len.to_be_bytes());
    out.extend_from_slice(value);
    Ok(())
}

pub(super) fn hash_bytes(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    format!("sha256:{}", general_purpose::URL_SAFE_NO_PAD.encode(digest))
}
