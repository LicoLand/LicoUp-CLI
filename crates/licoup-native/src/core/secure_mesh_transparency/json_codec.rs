//! Bounded canonical JSON, hexadecimal codecs, validation, and STH signing transcripts.

use anyhow::{Result, anyhow, ensure};
use ed25519_dalek::Signature;
use serde_json::Value;
use sha2::{Digest, Sha256};

use super::constants::{
    HASH_LEN, KT_JSON_SAFE_INTEGER_MAX, MAX_CONSISTENCY_PROOF_HASHES, MAX_TRANSPARENCY_FIELD_BYTES,
    STH_SIGN_MAGIC,
};
use super::model::{SecureMeshKtConsistencyProof, SecureMeshTransparencyLeafBody};
use super::signature::SecureMeshSignedTreeHead;

pub(super) fn domain_hash(domain: &[u8], bytes: &[u8]) -> [u8; HASH_LEN] {
    let mut hasher = Sha256::new();
    hasher.update(domain);
    hasher.update(bytes);
    hasher.finalize().into()
}

pub(super) fn sth_sign_payload(sth: &SecureMeshSignedTreeHead) -> Result<Vec<u8>> {
    validate_text("log_id", &sth.log_id)?;
    validate_text("key_id", &sth.key_id)?;
    validate_hex_hash("root_hash", &sth.root_hash)?;
    validate_hex_hash("map_root_hash", &sth.map_root_hash)?;
    let mut out = Vec::new();
    out.extend_from_slice(STH_SIGN_MAGIC);
    append_len_prefixed(&mut out, sth.protocol_version.as_bytes());
    append_len_prefixed(&mut out, sth.log_id.as_bytes());
    append_len_prefixed(&mut out, sth.key_id.as_bytes());
    out.extend_from_slice(&sth.tree_size.to_be_bytes());
    out.extend_from_slice(&parse_hash(&sth.root_hash)?);
    out.extend_from_slice(&parse_hash(&sth.map_root_hash)?);
    out.extend_from_slice(&sth.issued_at_epoch_seconds.to_be_bytes());
    Ok(out)
}

pub(super) fn append_len_prefixed(out: &mut Vec<u8>, value: &[u8]) {
    out.extend_from_slice(&(value.len() as u64).to_be_bytes());
    out.extend_from_slice(value);
}

pub(super) fn validate_leaf_body(body: &SecureMeshTransparencyLeafBody) -> Result<()> {
    validate_hex_hash(
        "directory_scope_commitment",
        &body.directory_scope_commitment,
    )?;
    validate_text("endpoint_id", &body.endpoint_id)?;
    validate_text("endpoint_kind", &body.endpoint_kind)?;
    validate_text("identity_public_key", &body.identity_public_key)?;
    validate_text("signing_public_key", &body.signing_public_key)?;
    validate_text("fingerprint", &body.fingerprint)?;
    validate_text("directory_state", &body.directory_state)?;
    validate_text("updated_at", &body.updated_at)?;
    ensure!(
        body.rotation_epoch <= KT_JSON_SAFE_INTEGER_MAX,
        "secure mesh transparency rotation epoch exceeds the cross-language safe range"
    );
    Ok(())
}

pub(super) fn validate_text(label: &str, value: &str) -> Result<()> {
    ensure!(
        !value.trim().is_empty(),
        "secure mesh transparency {label} is required"
    );
    ensure!(
        value.len() <= MAX_TRANSPARENCY_FIELD_BYTES,
        "secure mesh transparency {label} is too large"
    );
    Ok(())
}

pub(super) fn validate_hex_hash(label: &str, value: &str) -> Result<()> {
    validate_text(label, value)?;
    ensure!(
        value.len() == HASH_LEN * 2 && value.bytes().all(|byte| byte.is_ascii_hexdigit()),
        "secure mesh transparency {label} is not a sha256 hex digest"
    );
    Ok(())
}

pub(super) fn parse_hash_path(values: &[String], max: usize) -> Result<Vec<[u8; HASH_LEN]>> {
    ensure!(
        values.len() <= max,
        "secure mesh KT proof exceeds its hash bound"
    );
    values.iter().map(|value| parse_hash(value)).collect()
}

pub(super) fn parse_hash(value: &str) -> Result<[u8; HASH_LEN]> {
    validate_hex_hash("hash", value)?;
    let mut out = [0u8; HASH_LEN];
    for (index, chunk) in value.as_bytes().chunks(2).enumerate() {
        let text =
            std::str::from_utf8(chunk).map_err(|_| anyhow!("secure mesh KT hash is not utf8"))?;
        out[index] =
            u8::from_str_radix(text, 16).map_err(|_| anyhow!("secure mesh KT hash is not hex"))?;
    }
    Ok(out)
}

pub(super) fn parse_signature(value: &str) -> Result<Signature> {
    ensure!(
        value.len() == 128 && value.bytes().all(|byte| byte.is_ascii_hexdigit()),
        "secure mesh KT signature is not a 64-byte hex value"
    );
    let mut bytes = [0u8; 64];
    for (index, chunk) in value.as_bytes().chunks(2).enumerate() {
        let text = std::str::from_utf8(chunk)
            .map_err(|_| anyhow!("secure mesh KT signature is not utf8"))?;
        bytes[index] = u8::from_str_radix(text, 16)
            .map_err(|_| anyhow!("secure mesh KT signature is not hex"))?;
    }
    Ok(Signature::from_bytes(&bytes))
}

pub(super) fn canonical_json(value: &Value) -> Result<String> {
    match value {
        Value::Null | Value::Bool(_) | Value::Number(_) | Value::String(_) => {
            serde_json::to_string(value).map_err(Into::into)
        }
        Value::Array(items) => {
            let mut out = String::from("[");
            for (index, item) in items.iter().enumerate() {
                if index > 0 {
                    out.push(',');
                }
                out.push_str(&canonical_json(item)?);
            }
            out.push(']');
            Ok(out)
        }
        Value::Object(map) => {
            let mut keys = map.keys().collect::<Vec<_>>();
            keys.sort();
            let mut out = String::from("{");
            for (index, key) in keys.iter().enumerate() {
                if index > 0 {
                    out.push(',');
                }
                out.push_str(&serde_json::to_string(key)?);
                out.push(':');
                out.push_str(&canonical_json(&map[*key])?);
            }
            out.push('}');
            Ok(out)
        }
    }
}

pub(super) fn hex_encode(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

pub(super) fn sth_to_json(sth: &SecureMeshSignedTreeHead) -> Value {
    serde_json::json!({
        "protocolVersion": sth.protocol_version,
        "logId": sth.log_id,
        "keyId": sth.key_id,
        "treeSize": sth.tree_size,
        "rootHash": sth.root_hash,
        "mapRootHash": sth.map_root_hash,
        "issuedAtEpochSeconds": sth.issued_at_epoch_seconds,
        "signature": sth.signature,
    })
}

pub(super) fn consistency_to_json(proof: &SecureMeshKtConsistencyProof) -> Value {
    serde_json::json!({
        "firstTreeSize": proof.first_tree_size,
        "secondTreeSize": proof.second_tree_size,
        "firstRootHash": proof.first_root_hash,
        "path": proof.path,
        "secondSignedTreeHead": sth_to_json(&proof.second_signed_tree_head),
    })
}

pub(super) fn required_json_text(value: &Value, field: &str) -> Result<String> {
    let text = value
        .get(field)
        .and_then(Value::as_str)
        .ok_or_else(|| anyhow!("secure mesh KT JSON field is required: {field}"))?;
    validate_text(field, text)?;
    Ok(text.to_string())
}

pub(super) fn required_json_u64(value: &Value, field: &str) -> Result<u64> {
    let parsed = value
        .get(field)
        .and_then(Value::as_u64)
        .ok_or_else(|| anyhow!("secure mesh KT JSON integer is required: {field}"))?;
    ensure!(
        parsed <= KT_JSON_SAFE_INTEGER_MAX,
        "secure mesh KT JSON integer exceeds the cross-language safe range: {field}"
    );
    Ok(parsed)
}

pub(super) fn parse_sth_json(value: &Value) -> Result<SecureMeshSignedTreeHead> {
    Ok(SecureMeshSignedTreeHead {
        protocol_version: required_json_text(value, "protocolVersion")?,
        log_id: required_json_text(value, "logId")?,
        key_id: required_json_text(value, "keyId")?,
        tree_size: required_json_u64(value, "treeSize")?,
        root_hash: required_json_text(value, "rootHash")?,
        map_root_hash: required_json_text(value, "mapRootHash")?,
        issued_at_epoch_seconds: required_json_u64(value, "issuedAtEpochSeconds")?,
        signature: required_json_text(value, "signature")?,
    })
}

pub(super) fn parse_consistency_json(value: &Value) -> Result<SecureMeshKtConsistencyProof> {
    let path = value
        .get("path")
        .and_then(Value::as_array)
        .ok_or_else(|| anyhow!("secure mesh KT consistency path is required"))?;
    ensure!(
        path.len() <= MAX_CONSISTENCY_PROOF_HASHES,
        "secure mesh KT consistency path is too large"
    );
    let path = path
        .iter()
        .map(|item| {
            item.as_str()
                .map(str::to_string)
                .ok_or_else(|| anyhow!("secure mesh KT consistency path hash is invalid"))
        })
        .collect::<Result<Vec<_>>>()?;
    Ok(SecureMeshKtConsistencyProof {
        first_tree_size: required_json_u64(value, "firstTreeSize")?,
        second_tree_size: required_json_u64(value, "secondTreeSize")?,
        first_root_hash: required_json_text(value, "firstRootHash")?,
        path,
        second_signed_tree_head: parse_sth_json(
            value
                .get("secondSignedTreeHead")
                .ok_or_else(|| anyhow!("secure mesh KT consistency STH is required"))?,
        )?,
    })
}
