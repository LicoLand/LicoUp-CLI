use super::constants::*;
use anyhow::{Context, Result, anyhow, ensure};
use base64::{Engine as _, engine::general_purpose};
use sha2::{Digest, Sha256};
use std::path::{Component, Path, PathBuf};

pub(super) fn validate_text(label: &str, value: &str, max: usize) -> Result<()> {
    ensure!(
        !value.trim().is_empty(),
        "secure mesh file {label} is required"
    );
    ensure!(value.len() <= max, "secure mesh file {label} is too large");
    Ok(())
}

pub(super) fn validate_crypto_context_text(label: &str, value: &str, max: usize) -> Result<()> {
    validate_text(label, value, max)?;
    ensure!(
        value == value.trim() && !value.chars().any(char::is_control),
        "secure mesh file {label} is not canonical"
    );
    Ok(())
}

pub(super) fn validate_relative_path(value: &str) -> Result<()> {
    ensure!(
        value.len() <= MAX_RELATIVE_PATH_BYTES,
        "secure mesh file relative path is too large"
    );
    ensure!(
        !value.starts_with('/') && !value.starts_with('\\'),
        "secure mesh file relative path must be relative"
    );
    for segment in value.split(['/', '\\']) {
        ensure!(
            segment != "." && segment != "..",
            "secure mesh file relative path must not traverse"
        );
    }
    Ok(())
}

pub(super) fn validate_file_name_segment(value: &str) -> Result<()> {
    ensure!(
        !value.contains('/') && !value.contains('\\'),
        "secure mesh file name must not contain path separators"
    );
    ensure!(
        value != "." && value != "..",
        "secure mesh file name must be a file name segment"
    );
    Ok(())
}

pub(super) fn normalized_relative_path(value: &str) -> Result<PathBuf> {
    validate_relative_path(value)?;
    let mut path = PathBuf::new();
    for segment in value
        .split(['/', '\\'])
        .filter(|segment| !segment.is_empty())
    {
        path.push(segment);
    }
    Ok(path)
}

pub(super) fn path_is_clean_relative(path: &Path) -> bool {
    !path.is_absolute()
        && path
            .components()
            .all(|component| matches!(component, Component::Normal(_) | Component::CurDir))
}

pub(super) fn path_to_string(path: &Path) -> Result<String> {
    path.to_str()
        .map(ToString::to_string)
        .ok_or_else(|| anyhow!("secure mesh file path is not valid UTF-8"))
}

pub(super) fn path_to_bytes(path: &Path) -> Vec<u8> {
    path.to_string_lossy().as_bytes().to_vec()
}

pub(super) fn hash_bytes(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    format!("sha256:{}", general_purpose::URL_SAFE_NO_PAD.encode(digest))
}

pub(super) fn validate_file_hash(label: &str, value: &str) -> Result<()> {
    let encoded = value
        .strip_prefix("sha256:")
        .ok_or_else(|| anyhow!("secure mesh file {label} algorithm is unsupported"))?;
    decode_exact_base64url(label, encoded, 32)?;
    Ok(())
}

pub(super) fn validate_authenticated_digest(label: &str, value: &str) -> Result<()> {
    let encoded = value
        .strip_prefix("sha256:")
        .or_else(|| value.strip_prefix("hmac-sha256:"))
        .ok_or_else(|| anyhow!("secure mesh file {label} algorithm is unsupported"))?;
    decode_exact_base64url(label, encoded, 32)?;
    Ok(())
}

pub(super) fn validate_file_chunk_hash(label: &str, value: &str) -> Result<()> {
    let encoded = value
        .strip_prefix("hmac-sha256:")
        .ok_or_else(|| anyhow!("secure mesh file {label} algorithm is unsupported"))?;
    decode_exact_base64url(label, encoded, 32)?;
    Ok(())
}

pub(super) fn decode_exact_base64url(
    label: &str,
    value: &str,
    expected_len: usize,
) -> Result<Vec<u8>> {
    ensure!(
        !value.contains('=')
            && !value
                .chars()
                .any(|character| matches!(character, '+' | '/')),
        "secure mesh {label} is not canonical base64url"
    );
    let decoded = general_purpose::URL_SAFE_NO_PAD
        .decode(value)
        .with_context(|| format!("secure mesh {label} is not base64url"))?;
    ensure!(
        decoded.len() == expected_len,
        "secure mesh {label} length is invalid"
    );
    ensure!(
        general_purpose::URL_SAFE_NO_PAD.encode(&decoded) == value,
        "secure mesh {label} is not canonical base64url"
    );
    Ok(decoded)
}

pub(super) fn read_string(reader: &mut SliceReader<'_>, label: &str) -> Result<String> {
    let bytes = reader.read_len_prefixed_bytes()?;
    String::from_utf8(bytes.to_vec())
        .map_err(|_| anyhow!("secure mesh file {label} is not valid UTF-8"))
}

pub(super) fn append_len_prefixed_bytes(out: &mut Vec<u8>, value: &[u8]) -> Result<()> {
    let len =
        u32::try_from(value.len()).map_err(|_| anyhow!("secure mesh file field is too large"))?;
    out.extend_from_slice(&len.to_be_bytes());
    out.extend_from_slice(value);
    Ok(())
}

pub(super) struct SliceReader<'a> {
    bytes: &'a [u8],
    offset: usize,
}

impl<'a> SliceReader<'a> {
    pub(super) fn new(bytes: &'a [u8]) -> Self {
        Self { bytes, offset: 0 }
    }

    pub(super) fn expect_bytes(&mut self, expected: &[u8]) -> Result<()> {
        let actual = self.read_exact(expected.len())?;
        ensure!(actual == expected, "secure mesh file magic is invalid");
        Ok(())
    }

    pub(super) fn read_u32(&mut self) -> Result<u32> {
        let bytes = self.read_exact(4)?;
        Ok(u32::from_be_bytes(
            bytes
                .try_into()
                .map_err(|_| anyhow!("secure mesh file u32 is invalid"))?,
        ))
    }

    pub(super) fn read_u64(&mut self) -> Result<u64> {
        let bytes = self.read_exact(8)?;
        Ok(u64::from_be_bytes(
            bytes
                .try_into()
                .map_err(|_| anyhow!("secure mesh file u64 is invalid"))?,
        ))
    }

    pub(super) fn read_len_prefixed_bytes(&mut self) -> Result<&'a [u8]> {
        let len = self.read_u32()? as usize;
        self.read_exact(len)
    }

    pub(super) fn read_exact(&mut self, len: usize) -> Result<&'a [u8]> {
        let end = self
            .offset
            .checked_add(len)
            .ok_or_else(|| anyhow!("secure mesh file length overflow"))?;
        ensure!(
            end <= self.bytes.len(),
            "secure mesh file payload is truncated"
        );
        let value = &self.bytes[self.offset..end];
        self.offset = end;
        Ok(value)
    }

    pub(super) fn is_empty(&self) -> bool {
        self.offset == self.bytes.len()
    }
}
