use std::{collections::BTreeMap, fs};

use anyhow::{Context, Result, anyhow, bail, ensure};
use base64::{Engine as _, engine::general_purpose};
use ed25519_dalek::VerifyingKey;
use serde_json::{Map, Value};

use super::{constants::MAX_UPDATE_METADATA_BYTES, params::json_text};

pub(super) fn load_public_keys(params: &Value) -> Result<BTreeMap<String, VerifyingKey>> {
    let document = if let Some(path) =
        json_text(params, &["publicKeysPath", "public-keys-path", "keysPath"])
    {
        let metadata = fs::metadata(&path)
            .with_context(|| "failed to inspect client update public keys document")?;
        ensure!(
            metadata.is_file() && metadata.len() <= MAX_UPDATE_METADATA_BYTES,
            "client update public keys document is invalid"
        );
        let raw = fs::read_to_string(&path)
            .with_context(|| "failed to read client update public keys document")?;
        serde_json::from_str::<Value>(&raw)
            .context("client update public keys document is not valid JSON")?
    } else {
        params
            .get("publicKeys")
            .cloned()
            .ok_or_else(|| anyhow!("client update public keys document is required"))?
    };
    parse_public_keys_document(&document)
}

fn parse_public_keys_document(document: &Value) -> Result<BTreeMap<String, VerifyingKey>> {
    let root = document
        .as_object()
        .ok_or_else(|| anyhow!("client update public keys document must be an object"))?;
    let entries = root
        .get("keys")
        .and_then(Value::as_object)
        .ok_or_else(|| anyhow!("client update public keys document must contain a keys object"))?;
    ensure!(
        !entries.is_empty(),
        "client update public keys document has no keys"
    );
    entries
        .iter()
        .map(|(key_id, entry)| {
            super::params::validate_public_identifier(key_id, "client update keyId")?;
            Ok((key_id.clone(), decode_key_entry(entry)?))
        })
        .collect()
}

fn decode_key_entry(entry: &Value) -> Result<VerifyingKey> {
    let encoded = match entry {
        Value::String(value) => value.as_str(),
        Value::Object(object) => exact_public_key_field(object)?,
        _ => bail!("client update public key entry is invalid"),
    };
    decode_public_key(encoded)
}

fn exact_public_key_field(object: &Map<String, Value>) -> Result<&str> {
    ensure!(
        object.keys().all(|key| key == "publicKey"),
        "client update public key entry contains unsupported fields"
    );
    object
        .get("publicKey")
        .and_then(Value::as_str)
        .ok_or_else(|| anyhow!("client update public key entry is invalid"))
}

fn decode_public_key(value: &str) -> Result<VerifyingKey> {
    let trimmed = value.trim();
    let bytes = general_purpose::STANDARD
        .decode(trimmed)
        .or_else(|_| general_purpose::URL_SAFE_NO_PAD.decode(trimmed))
        .context("client update public key encoding is unsupported")?;
    let array: [u8; 32] = bytes
        .as_slice()
        .try_into()
        .map_err(|_| anyhow!("client update public key must be 32 raw Ed25519 bytes"))?;
    VerifyingKey::from_bytes(&array).map_err(|_| anyhow!("client update public key is invalid"))
}
