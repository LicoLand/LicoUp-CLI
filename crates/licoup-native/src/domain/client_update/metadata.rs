use std::{fs, path::Path};

use anyhow::{Context, Result, anyhow, bail, ensure};
use serde_json::Value;

use super::{constants::MAX_UPDATE_METADATA_BYTES, params::json_text};

pub(super) fn load_manifest(params: &Value) -> Result<Value> {
    load_document(
        params,
        &["manifestPath", "manifest-path", "manifest"],
        "manifestJson",
        "client update manifest",
    )?
    .ok_or_else(|| anyhow!("client update signed manifest is required"))
}

pub(super) fn load_revocation_list(params: &Value) -> Result<Option<Value>> {
    load_document(
        params,
        &["revocationPath", "revocation-path"],
        "revocationList",
        "client update revocation list",
    )
}

fn load_document(
    params: &Value,
    path_keys: &[&str],
    inline_key: &str,
    label: &str,
) -> Result<Option<Value>> {
    if let Some(path) = json_text(params, path_keys) {
        return Ok(Some(read_json_file(Path::new(&path), label)?));
    }
    let Some(inline) = params.get(inline_key) else {
        return Ok(None);
    };
    if let Some(text) = inline.as_str() {
        ensure!(
            text.len() as u64 <= MAX_UPDATE_METADATA_BYTES,
            "{label} exceeds the metadata size limit"
        );
        return Ok(Some(
            serde_json::from_str(text).with_context(|| format!("{label} is not valid JSON"))?,
        ));
    }
    ensure!(inline.is_object(), "{label} must be a JSON object");
    Ok(Some(inline.clone()))
}

fn read_json_file(path: &Path, label: &str) -> Result<Value> {
    let metadata = fs::metadata(path).with_context(|| format!("failed to inspect {label}"))?;
    ensure!(
        metadata.is_file() && metadata.len() <= MAX_UPDATE_METADATA_BYTES,
        "{label} is invalid"
    );
    let raw = fs::read_to_string(path).with_context(|| format!("failed to read {label}"))?;
    match serde_json::from_str(&raw) {
        Ok(value) => Ok(value),
        Err(error) if error.is_eof() => bail!("{label} is truncated"),
        Err(error) => Err(error).with_context(|| format!("{label} is not valid JSON")),
    }
}
