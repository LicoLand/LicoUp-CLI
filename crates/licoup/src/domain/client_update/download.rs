use anyhow::Result;
use serde_json::{Value, json};

use super::{
    constants::CLIENT_UPDATE_MODE, model::VerifiedUpdateSelection,
    selection::require_available_selection, staging::stage_selected_artifact,
};

pub fn download(params: &Value) -> Result<Value> {
    let selection = require_available_selection(params)?;
    let staged = stage_selected_artifact(params, &selection)?;
    Ok(download_result_json(selection, staged.resumed))
}

/// Shared downloaded-phase response shape for the local copy path and the
/// remote GitHub download path so every staged artifact reports the same
/// digest-bound receipt.
pub(super) fn download_result_json(selection: VerifiedUpdateSelection, resumed: bool) -> Value {
    json!({
        "ok": true,
        "mode": CLIENT_UPDATE_MODE,
        "phase": "downloaded",
        "runningVersion": selection.running_version,
        "runningReleaseTrack": selection.running_release_track,
        "targetReleaseTrack": selection.target_release_track,
        "availableVersion": selection.version,
        "targetId": selection.artifact.target_id,
        "stagedArtifactId": selection.receipt()["receiptId"],
        "stagedBytes": selection.artifact.size,
        "totalBytes": selection.artifact.size,
        "artifactSha256": selection.artifact.sha256,
        "artifactReceipt": selection.receipt(),
        "stagingRootRedacted": true,
        "resumed": resumed,
        "productionReady": false,
        "publicMetadataOnly": true,
    })
}
