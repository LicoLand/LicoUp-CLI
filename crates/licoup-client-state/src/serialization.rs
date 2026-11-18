use anyhow::{Result, ensure};
use licoup_foundation::platform::file_security::{
    atomic_write_private_text, atomic_write_private_text_bounded, read_private_text_bounded,
};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

pub fn read_json_or_default<F>(path: &Path, max_bytes: usize, default_value: F) -> Result<Value>
where
    F: FnOnce() -> Value,
{
    let Some(raw) = read_private_text_bounded(path, max_bytes)? else {
        return Ok(default_value());
    };
    if raw.trim().is_empty() {
        return Ok(default_value());
    }
    Ok(serde_json::from_str(&raw)?)
}

pub fn atomic_write_json(path: &Path, value: &Value, max_bytes: usize) -> Result<()> {
    let content = format!("{}\n", serde_json::to_string_pretty(value)?);
    atomic_write_private_text_bounded(path, &content, max_bytes)
}

pub fn atomic_write_local_text_bounded(path: &Path, content: &str, max_bytes: usize) -> Result<()> {
    ensure!(
        content.len() <= max_bytes,
        "local snapshot content exceeds its bounded size"
    );
    atomic_write_private_text(path, content)
}

pub fn hash_text(content: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(content.as_bytes());
    format!("{:x}", hasher.finalize())
}

pub fn sanitize_id(value: &str) -> String {
    let sanitized = value
        .chars()
        .take(64)
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || ch == '-' || ch == '_' {
                ch
            } else {
                '-'
            }
        })
        .collect::<String>()
        .trim_matches('-')
        .to_string();
    if sanitized.is_empty() {
        "item".to_string()
    } else {
        sanitized
    }
}

pub fn timestamp() -> String {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default();
    format!("{}-{}", now.as_secs(), now.subsec_nanos())
}
