use std::path::PathBuf;

use anyhow::{Context, Result, anyhow, ensure};
use serde::{Deserialize, Serialize};
use serde_json::Value;

const ACTIVE_RECEIPT_SCHEMA: &str = "v0.0.1:client-update:active-receipt-1";
const ACTIVE_RECEIPT_MAX_BYTES: usize = 4 * 1024;

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ActiveReceipt {
    schema_version: String,
    receipt_id: String,
    target_release_track: String,
}

fn receipt_path(params: &Value) -> PathBuf {
    super::params::state_root(params).join("active-artifact-receipt.json")
}

pub(super) fn bind_check_result(params: &Value, result: &Value) -> Result<()> {
    let path = receipt_path(params);
    if result.get("updateAvailable").and_then(Value::as_bool) != Some(true) {
        licoup_foundation::platform::file_security::remove_private_state_marker(&path)
            .context("client update active receipt could not be cleared")?;
        return Ok(());
    }
    let receipt = result
        .get("artifactReceipt")
        .and_then(Value::as_object)
        .ok_or_else(|| anyhow!("client update check did not produce an artifact receipt"))?;
    let receipt_id = receipt
        .get("receiptId")
        .and_then(Value::as_str)
        .filter(|value| value.starts_with("sha256:") && value.len() == 71)
        .ok_or_else(|| anyhow!("client update artifact receipt id is invalid"))?;
    let target_release_track = receipt
        .get("targetReleaseTrack")
        .and_then(Value::as_str)
        .filter(|value| matches!(*value, "nightly" | "stable"))
        .ok_or_else(|| anyhow!("client update artifact receipt track is invalid"))?;
    let binding = ActiveReceipt {
        schema_version: ACTIVE_RECEIPT_SCHEMA.to_owned(),
        receipt_id: receipt_id.to_owned(),
        target_release_track: target_release_track.to_owned(),
    };
    let serialized = serde_json::to_string(&binding)
        .context("client update active receipt could not be serialized")?;
    licoup_foundation::platform::file_security::atomic_write_private_text_bounded(
        &path,
        &format!("{serialized}\n"),
        ACTIVE_RECEIPT_MAX_BYTES,
    )
    .context("client update active receipt could not be persisted")
}

pub(super) fn params_with_bound_track(params: &Value) -> Result<(Value, String)> {
    ensure!(
        params.get("targetReleaseTrack").is_none() && params.get("target-release-track").is_none(),
        "client update target release track is accepted only during check"
    );
    let path = receipt_path(params);
    let raw = licoup_foundation::platform::file_security::read_private_text_bounded(
        &path,
        ACTIVE_RECEIPT_MAX_BYTES,
    )?
    .ok_or_else(|| anyhow!("client update signed check receipt is required before this step"))?;
    let binding: ActiveReceipt =
        serde_json::from_str(&raw).context("client update active receipt is invalid")?;
    ensure!(
        binding.schema_version == ACTIVE_RECEIPT_SCHEMA
            && binding.receipt_id.starts_with("sha256:")
            && binding.receipt_id.len() == 71
            && matches!(binding.target_release_track.as_str(), "nightly" | "stable"),
        "client update active receipt is invalid"
    );
    let mut effective = params.clone();
    if let Some(existing) = effective
        .get("boundTargetReleaseTrack")
        .and_then(Value::as_str)
    {
        ensure!(
            existing == binding.target_release_track,
            "client update active receipt track is inconsistent"
        );
    } else {
        effective["boundTargetReleaseTrack"] = Value::String(binding.target_release_track);
    }
    Ok((effective, binding.receipt_id))
}

pub(super) fn ensure_selection_matches(
    selection: &super::model::VerifiedUpdateSelection,
    receipt_id: &str,
) -> Result<()> {
    ensure!(
        selection.receipt().get("receiptId").and_then(Value::as_str) == Some(receipt_id),
        "client update artifact does not match the signed check receipt"
    );
    Ok(())
}
