use std::{
    fmt::Write as _,
    fs,
    io::{BufReader, Read},
    path::Path,
};

use anyhow::{Context, Result, ensure};
use serde_json::Value;
use sha2::{Digest, Sha256};

use super::constants::UPDATE_COPY_BUFFER_BYTES;

pub(super) fn stable_stringify(value: &Value) -> String {
    match value {
        Value::Null => "null".to_string(),
        Value::Bool(flag) => if *flag { "true" } else { "false" }.to_string(),
        Value::Number(number) => number.to_string(),
        Value::String(text) => {
            serde_json::to_string(text).unwrap_or_else(|_| "\"<invalid>\"".into())
        }
        Value::Array(items) => {
            let body = items
                .iter()
                .map(stable_stringify)
                .collect::<Vec<_>>()
                .join(",");
            format!("[{body}]")
        }
        Value::Object(map) => {
            let mut keys = map.keys().collect::<Vec<_>>();
            keys.sort_unstable();
            let body = keys
                .into_iter()
                .map(|key| {
                    let encoded_key =
                        serde_json::to_string(key).unwrap_or_else(|_| "\"<invalid>\"".into());
                    format!("{encoded_key}:{}", stable_stringify(&map[key]))
                })
                .collect::<Vec<_>>()
                .join(",");
            format!("{{{body}}}")
        }
    }
}

pub(super) fn unsigned_document(document: &Value) -> Value {
    let mut clone = document.clone();
    if let Some(object) = clone.as_object_mut() {
        object.remove("signatures");
    }
    clone
}

pub(super) fn canonical_unsigned_bytes(document: &Value) -> Vec<u8> {
    stable_stringify(&unsigned_document(document)).into_bytes()
}

pub(super) fn sha256_hex(bytes: &[u8]) -> String {
    digest_to_string(&Sha256::digest(bytes))
}

pub(super) fn canonical_unsigned_sha256(document: &Value) -> String {
    sha256_hex(&canonical_unsigned_bytes(document))
}

pub(super) fn sha256_file_exact(path: &Path, expected_size: u64) -> Result<String> {
    let file =
        fs::File::open(path).with_context(|| "failed to open staged client update artifact")?;
    let metadata = file
        .metadata()
        .context("failed to inspect staged client update artifact")?;
    ensure!(
        metadata.is_file() && metadata.len() == expected_size,
        "client update artifact size does not match signed metadata"
    );
    let mut reader = BufReader::with_capacity(UPDATE_COPY_BUFFER_BYTES, file);
    let mut digest = Sha256::new();
    let mut buffer = [0_u8; UPDATE_COPY_BUFFER_BYTES];
    let mut total = 0_u64;
    loop {
        let read = reader
            .read(&mut buffer)
            .context("failed to read staged client update artifact")?;
        if read == 0 {
            break;
        }
        total = total
            .checked_add(read as u64)
            .context("client update artifact size overflow")?;
        ensure!(
            total <= expected_size,
            "client update artifact exceeds signed size"
        );
        digest.update(&buffer[..read]);
    }
    ensure!(
        total == expected_size,
        "client update artifact size does not match signed metadata"
    );
    Ok(digest_to_string(&digest.finalize()))
}

fn digest_to_string(bytes: &[u8]) -> String {
    let mut output = String::with_capacity(7 + bytes.len() * 2);
    output.push_str("sha256:");
    for byte in bytes {
        write!(&mut output, "{byte:02x}").expect("writing to String cannot fail");
    }
    output
}
