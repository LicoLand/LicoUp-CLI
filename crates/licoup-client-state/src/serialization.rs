use anyhow::{Result, ensure};
use licoup_foundation::platform::file_security::read_existing_private_text_bounded;
use licoup_foundation::platform::file_security::{
    atomic_write_private_text, atomic_write_private_text_bounded, read_private_text_bounded,
};
use serde::Serialize;
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::io::{self, Write};
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

pub fn read_json_or_default_read_only<F>(
    path: &Path,
    max_bytes: usize,
    default_value: F,
) -> Result<Value>
where
    F: FnOnce() -> Value,
{
    let Some(raw) = read_existing_private_text_bounded(path, max_bytes)? else {
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

/// Keep a complete newest suffix of an oldest-first collection. The wrapper
/// measures each entry at the same indentation depth as the actual document;
/// strings, Unicode, escaping, and nested objects use the writer's own codec.
pub fn retain_latest_items(document: &mut Value, max_items: usize, max_bytes: usize) -> Result<()> {
    ensure!(
        max_items > 0,
        "collection retention requires at least one item"
    );
    let mut items = document
        .get_mut("items")
        .and_then(Value::as_array_mut)
        .map(std::mem::take)
        .ok_or_else(|| anyhow::anyhow!("collection retention requires an items array"))?;
    let empty_bytes = encoded_json_bytes(document)?;
    if items.is_empty() {
        ensure!(
            empty_bytes <= max_bytes,
            "collection metadata exceeds its bounded size"
        );
        return Ok(());
    }
    #[derive(Serialize)]
    struct Items<'a> {
        items: &'a [&'a Value],
    }
    let empty_wrapper = encoded_json_bytes(&Items { items: &[] })?;
    // A populated pretty array adds its opening/closing line breaks. Each
    // additional entry has the same byte contribution after its separator.
    let mut bytes = empty_bytes.saturating_add(2);
    let mut retained = 0;
    for item in items.iter().rev().take(max_items) {
        let item_bytes =
            encoded_json_bytes(&Items { items: &[item] })?.saturating_sub(empty_wrapper + 2);
        if bytes.saturating_add(item_bytes) > max_bytes {
            break;
        }
        bytes += item_bytes;
        retained += 1;
    }
    ensure!(
        retained > 0,
        "complete latest collection item exceeds its bounded size"
    );
    items.drain(..items.len() - retained);
    document["items"] = Value::Array(items);
    Ok(())
}

fn encoded_json_bytes(value: &impl Serialize) -> Result<usize> {
    #[derive(Default)]
    struct ByteCount(usize);
    impl Write for ByteCount {
        fn write(&mut self, buffer: &[u8]) -> io::Result<usize> {
            self.0 = self.0.saturating_add(buffer.len());
            Ok(buffer.len())
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }
    let mut count = ByteCount::default();
    serde_json::to_writer_pretty(&mut count, value)?;
    Ok(count.0.saturating_add(1)) // atomic_write_json's trailing LF
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
