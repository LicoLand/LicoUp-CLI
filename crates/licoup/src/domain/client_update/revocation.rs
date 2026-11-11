use std::collections::BTreeMap;

use anyhow::{Result, anyhow, ensure};
use ed25519_dalek::VerifyingKey;
use serde_json::Value;

use super::{
    constants::CLIENT_UPDATE_REVOCATION_SCHEMA, model::VerifiedUpdateSelection, release::is_sha256,
    signature::verify_required_signature,
};

pub(super) fn enforce_revocation_policy(
    manifest: &Value,
    revocation: Option<&Value>,
    public_keys: &BTreeMap<String, VerifyingKey>,
    offline_root_key_id: &str,
    online_signing_key_id: &str,
    release_track: &str,
    selection: Option<&VerifiedUpdateSelection>,
) -> Result<()> {
    let required = manifest
        .pointer("/releaseTrackPolicy/revocationPolicy")
        .and_then(Value::as_str)
        == Some("signed-revocation-list-required");
    if required {
        ensure!(
            revocation.is_some(),
            "client update signed revocation list is required by release track policy"
        );
    }
    let Some(document) = revocation else {
        return Ok(());
    };
    super::selection::ensure_exact_object_keys(
        document,
        &[
            "schemaVersion",
            "releaseTrack",
            "offlineRootKeyId",
            "revokedKeyIds",
            "revokedVersions",
            "revokedArtifactDigests",
            "signatures",
        ],
        "client update revocation list contract is not closed",
    )?;
    ensure!(
        document.get("schemaVersion").and_then(Value::as_str)
            == Some(CLIENT_UPDATE_REVOCATION_SCHEMA),
        "client update revocation list schema is unsupported"
    );
    ensure!(
        document.get("releaseTrack").and_then(Value::as_str) == Some(release_track),
        "client update revocation list release track does not match"
    );
    ensure!(
        document.get("offlineRootKeyId").and_then(Value::as_str) == Some(offline_root_key_id),
        "client update revocation list offline root key does not match release track policy"
    );
    verify_required_signature(
        document,
        public_keys,
        offline_root_key_id,
        "client update revocation list",
    )?;
    let revoked_keys = string_array(document, "revokedKeyIds")?;
    ensure!(
        !revoked_keys
            .iter()
            .any(|key| key == offline_root_key_id || key == online_signing_key_id),
        "client update release signing key is revoked"
    );
    let revoked_versions = string_array(document, "revokedVersions")?;
    let revoked_digests = string_array(document, "revokedArtifactDigests")?;
    ensure!(
        revoked_digests.iter().all(|digest| is_sha256(digest)),
        "client update revocation list contains an invalid artifact digest"
    );
    if let Some(selection) = selection {
        ensure!(
            !revoked_versions.contains(&selection.version),
            "client update release version is revoked"
        );
        ensure!(
            !revoked_digests.contains(&selection.artifact.sha256),
            "client update release artifact is revoked"
        );
    }
    Ok(())
}

fn string_array(document: &Value, field: &str) -> Result<Vec<String>> {
    let Some(value) = document.get(field) else {
        return Ok(Vec::new());
    };
    let entries = value
        .as_array()
        .ok_or_else(|| anyhow!("client update revocation list {field} must be an array"))?;
    entries
        .iter()
        .map(|entry| {
            entry
                .as_str()
                .map(str::trim)
                .filter(|text| !text.is_empty())
                .map(ToOwned::to_owned)
                .ok_or_else(|| {
                    anyhow!("client update revocation list {field} contains an invalid value")
                })
        })
        .collect()
}
