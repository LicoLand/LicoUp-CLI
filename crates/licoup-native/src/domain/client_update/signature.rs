use std::collections::{BTreeMap, BTreeSet};

use anyhow::{Context, Result, anyhow, ensure};
use base64::{Engine as _, engine::general_purpose};
use ed25519_dalek::{Signature, Verifier, VerifyingKey};
use serde_json::Value;

use super::canonical::canonical_unsigned_bytes;

pub(super) fn verify_manifest_role_signatures(
    manifest: &Value,
    public_keys: &BTreeMap<String, VerifyingKey>,
    offline_root_key_id: &str,
    online_signing_key_id: &str,
) -> Result<Vec<String>> {
    ensure!(
        offline_root_key_id != online_signing_key_id,
        "client update offline root and online signing keys must be distinct"
    );
    let verified = verify_all_signatures(manifest, public_keys)?;
    ensure!(
        verified.contains(offline_root_key_id),
        "client update manifest requires a valid offline root signature"
    );
    ensure!(
        verified.contains(online_signing_key_id),
        "client update manifest requires a valid online signing signature"
    );
    Ok(verified.into_iter().collect())
}

pub(super) fn verify_required_signature(
    document: &Value,
    public_keys: &BTreeMap<String, VerifyingKey>,
    required_key_id: &str,
    label: &str,
) -> Result<()> {
    let verified = verify_all_signatures(document, public_keys)?;
    ensure!(
        verified.contains(required_key_id),
        "{label} requires its declared signing key"
    );
    Ok(())
}

fn verify_all_signatures(
    document: &Value,
    public_keys: &BTreeMap<String, VerifyingKey>,
) -> Result<BTreeSet<String>> {
    let signatures = document
        .get("signatures")
        .and_then(Value::as_array)
        .ok_or_else(|| anyhow!("client update signed document signatures are required"))?;
    ensure!(
        !signatures.is_empty(),
        "client update signed document has no signatures"
    );
    let payload = canonical_unsigned_bytes(document);
    let mut verified = BTreeSet::new();
    for entry in signatures {
        let object = entry
            .as_object()
            .ok_or_else(|| anyhow!("client update signature entry must be an object"))?;
        ensure!(
            object
                .keys()
                .all(|key| matches!(key.as_str(), "keyId" | "algorithm" | "signature")),
            "client update signature entry contains unsupported fields"
        );
        let key_id = object
            .get("keyId")
            .and_then(Value::as_str)
            .ok_or_else(|| anyhow!("client update signature keyId is required"))?;
        super::params::validate_public_identifier(key_id, "client update signature keyId")?;
        ensure!(
            verified.insert(key_id.to_string()),
            "client update signature keyId must be unique"
        );
        ensure!(
            object.get("algorithm").and_then(Value::as_str) == Some("Ed25519"),
            "client update signature algorithm must be Ed25519"
        );
        let encoded = object
            .get("signature")
            .and_then(Value::as_str)
            .ok_or_else(|| anyhow!("client update signature is required"))?;
        let bytes = general_purpose::STANDARD
            .decode(encoded)
            .or_else(|_| general_purpose::URL_SAFE_NO_PAD.decode(encoded))
            .context("client update signature is not valid base64")?;
        let array: [u8; 64] = bytes
            .as_slice()
            .try_into()
            .map_err(|_| anyhow!("client update signature length is invalid"))?;
        let key = public_keys
            .get(key_id)
            .ok_or_else(|| anyhow!("client update signature key is unknown"))?;
        key.verify(&payload, &Signature::from_bytes(&array))
            .map_err(|_| anyhow!("client update signed document verification failed"))?;
    }
    Ok(verified)
}
