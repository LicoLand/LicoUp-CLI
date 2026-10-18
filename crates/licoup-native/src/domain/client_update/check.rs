use anyhow::Result;
use serde_json::{Value, json};

use super::{constants::CLIENT_UPDATE_MODE, selection::verify_update_selection};

pub fn check(params: &Value) -> Result<Value> {
    let verified = verify_update_selection(params)?;
    let Some(selection) = verified.selected else {
        let output = json!({
            "ok": true,
            "mode": CLIENT_UPDATE_MODE,
            "phase": "upToDate",
            "runningReleaseTrack": verified.running_release_track,
            "targetReleaseTrack": verified.target_release_track,
            "runningVersion": verified.running_version,
            "updateAvailable": false,
            "verifiedKeyIds": verified.verified_key_ids,
            "manifestSha256": verified.manifest_sha256,
            "productionReady": false,
            "publicMetadataOnly": true,
            "storeCredentialsRequired": false,
        });
        if params.get("deferReceiptBinding") != Some(&Value::Bool(true)) {
            super::receipt::bind_check_result(params, &output)?;
        }
        return Ok(output);
    };
    let output = json!({
        "ok": true,
        "mode": CLIENT_UPDATE_MODE,
        "phase": "updateAvailable",
        "runningReleaseTrack": selection.running_release_track,
        "targetReleaseTrack": selection.target_release_track,
        "runningVersion": selection.running_version,
        "updateAvailable": true,
        "availableVersion": selection.version,
        "classification": selection.classification,
        "releaseNotesUrl": selection.release_notes_url,
        "migrationNotes": selection.migration_notes,
        "migrationFrontier": selection.migration_frontier,
        "artifact": selection.artifact.public_projection(),
        "artifactReceipt": selection.receipt(),
        "verifiedKeyIds": selection.verified_key_ids,
        "manifestSha256": selection.manifest_sha256,
        "productionReady": false,
        "publicMetadataOnly": true,
        "storeCredentialsRequired": false,
    });
    if params.get("deferReceiptBinding") != Some(&Value::Bool(true)) {
        super::receipt::bind_check_result(params, &output)?;
    }
    Ok(output)
}
