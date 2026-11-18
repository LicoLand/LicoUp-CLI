use std::path::PathBuf;

use anyhow::Result;
use serde_json::{Value, json};

use super::{
    constants::CLIENT_UPDATE_MODE, model::VerifiedUpdateSelection,
    selection::require_available_selection, staging::verified_staged_artifact,
};

pub fn verify(params: &Value) -> Result<Value> {
    let (selection, _) = verify_staged_selection(params)?;
    Ok(json!({
        "ok": true,
        "mode": CLIENT_UPDATE_MODE,
        "phase": "verified",
        "runningVersion": selection.running_version,
        "runningReleaseTrack": selection.running_release_track,
        "targetReleaseTrack": selection.target_release_track,
        "availableVersion": selection.version,
        "targetId": selection.artifact.target_id,
        "stagedArtifactId": selection.receipt()["receiptId"],
        "artifactSha256": selection.artifact.sha256,
        "artifactReceipt": selection.receipt(),
        "manifestVerified": true,
        "digestMatched": true,
        "productionReady": false,
        "publicMetadataOnly": true,
    }))
}

pub(super) fn verify_staged_selection(
    params: &Value,
) -> Result<(VerifiedUpdateSelection, PathBuf)> {
    let selection = require_available_selection(params)?;
    let path = verified_staged_artifact(params, &selection.artifact)?;
    Ok((selection, path))
}
