//! Live client replacement: the one apply route that changes installed bytes.
//!
//! Whether an installed client may be replaced belongs to the host's own
//! maintenance guard (`UPDATE-IDLE-ADMISSION`), and this route is where that
//! decision is executed. A live apply asks the guard about the data root the
//! switch would change, and the answer decides:
//!
//! * **idle** — the barrier is taken by the same call that reads the decision,
//!   so no work can be admitted between the answer and the replacement, and the
//!   switch proceeds.
//! * **blocked** — this host still owns unfinished local work.
//! * **closed** — another maintenance switch already holds the barrier.
//! * **no readable answer** — a decision nobody could read is a refusal, never
//!   an assumed idle host.
//!
//! Every refusal is published in the vocabulary the client's own update
//! projection already renders (`client_update_admission_*`), carries the native
//! decision and its blockers, and leaves installed state untouched. The planning
//! path (`execute: false`) stages nothing and therefore asks nothing: it is a
//! preview, not maintenance.
//!
//! Holding is not completing. On abort the barrier is released here, because
//! ownership never left this process. On success ownership passes to the
//! candidate, and the barrier is retired by the admission of the switched-to
//! client (`domain::client_state_migration`), which is the only point that can
//! state the replacement finished.

use anyhow::Result;
use serde_json::{Value, json};
use std::path::Path;

use super::{
    constants::CLIENT_UPDATE_MODE,
    model::VerifiedUpdateSelection,
    params::{bool_param, live_data_root},
    staging::reject_artifact_overrides,
    verify::verify_staged_selection,
};
use crate::domain::work_admission::{MaintenanceAdmission, MaintenanceOperation, WorkAdmission};

/// The refusal this surface publishes when this host still owns unfinished work.
///
/// The client's update projection renders this code, so the same refusal reads
/// the same way whether it was decided before the switch or at it.
pub const ADMISSION_BLOCKED: &str = "client_update_admission_blocked";

/// The refusal this surface publishes when another maintenance switch holds the
/// close-admission barrier.
pub const ADMISSION_CLOSED: &str = "client_update_admission_closed";

/// The refusal this surface publishes when no admission decision could be read.
pub const ADMISSION_UNAVAILABLE: &str = "client_update_admission_unavailable";

pub fn apply(params: &Value) -> Result<Value> {
    reject_artifact_overrides(params)?;
    let (selection, staged_path) = verify_staged_selection(params)?;
    let execute = bool_param(params, "execute")?;
    if !execute {
        return Ok(planned(&selection));
    }
    let data_root = live_data_root(params)?;
    let owner = match admit_replacement(&data_root) {
        Replacement::Held { owner } => owner,
        Replacement::Refused { code, answer } => return Ok(refusal(&selection, code, answer)),
    };
    match super::native_runner::apply_live(&selection, &staged_path, params) {
        Ok(applied) => Ok(applied),
        // Dispatch failed before ownership could pass to the candidate, so this
        // process still owns the switch and releases the barrier it took. A
        // release that itself fails is reported instead of the abort: a host
        // left with admission closed is the more serious fact, and the abort
        // travels as context on it.
        Err(error) => {
            owner.release_admission().map_err(|release| {
                release.context(format!("client update apply aborted: {error:#}"))
            })?;
            Err(error)
        }
    }
}

/// The maintenance verdict for one live apply.
pub(super) enum Replacement {
    /// The decision admitted this switch, and the barrier is held for it.
    Held { owner: WorkAdmission },
    /// The decision refused this switch. `code` is the refusal this surface
    /// publishes; `answer` is the native decision the client reads, or null when
    /// no decision could be read at all.
    Refused { code: &'static str, answer: Value },
}

/// Ask the host's maintenance guard about one data root, and hold the barrier
/// when it admits this switch.
///
/// The decision and the barrier are one call, so an idle answer cannot be lost
/// to work admitted in between. A guard whose decision cannot be read is
/// refused: an unreadable answer is not an idle host.
pub(super) fn admit_replacement(data_root: &Path) -> Replacement {
    let owner = WorkAdmission::open(data_root);
    match owner.begin_maintenance(MaintenanceOperation::ClientReplacement) {
        Ok(MaintenanceAdmission::Held(_)) => Replacement::Held { owner },
        Ok(MaintenanceAdmission::AlreadyClosed(_)) => Replacement::Refused {
            code: ADMISSION_CLOSED,
            answer: json!({ "decision": "closed", "blockers": [], "truncated": false }),
        },
        Ok(MaintenanceAdmission::Blocked {
            blockers,
            truncated,
        }) => Replacement::Refused {
            code: ADMISSION_BLOCKED,
            answer: json!({
                "decision": "blocked",
                "blockers": blockers,
                "truncated": truncated,
            }),
        },
        Err(_) => Replacement::Refused {
            code: ADMISSION_UNAVAILABLE,
            answer: Value::Null,
        },
    }
}

/// The report a live apply that was admitted but did not execute publishes.
fn planned(selection: &VerifiedUpdateSelection) -> Value {
    json!({
        "ok": true,
        "mode": CLIENT_UPDATE_MODE,
        "phase": "applyPlanned",
        "runningVersion": selection.running_version,
        "runningReleaseTrack": selection.running_release_track,
        "targetReleaseTrack": selection.target_release_track,
        "availableVersion": selection.version,
        "targetId": selection.artifact.target_id,
        "installerStrategy": selection.artifact.installer_strategy,
        "stagedArtifactId": selection.receipt()["receiptId"],
        "artifactSha256": selection.artifact.sha256,
        "artifactReceipt": selection.receipt(),
        "executed": false,
        "restartRequired": true,
        "preUpdateStateRecord": {
            "runningVersion": selection.running_version,
            "recorded": true,
            "pathRedacted": true,
        },
        "productionReady": false,
        "publicMetadataOnly": true,
        "storeCredentialsRequired": false,
    })
}

/// The report one refused live apply publishes.
///
/// It is a report and not a transport error: the request was admitted, the host
/// refused the switch, and that reason is a fact the caller renders. The
/// artifact facts travel with it because the client binds its verified receipt
/// to whatever this route returns.
fn refusal(selection: &VerifiedUpdateSelection, code: &str, answer: Value) -> Value {
    json!({
        "ok": false,
        "mode": CLIENT_UPDATE_MODE,
        "phase": "blocked",
        "errorCode": code,
        "reasonCode": code,
        "admission": answer,
        "runningVersion": selection.running_version,
        "runningReleaseTrack": selection.running_release_track,
        "targetReleaseTrack": selection.target_release_track,
        "availableVersion": selection.version,
        "targetId": selection.artifact.target_id,
        "installerStrategy": selection.artifact.installer_strategy,
        "stagedArtifactId": selection.receipt()["receiptId"],
        "artifactSha256": selection.artifact.sha256,
        "artifactReceipt": selection.receipt(),
        "executed": false,
        "restartRequired": false,
        "productionReady": false,
        "publicMetadataOnly": true,
        "storeCredentialsRequired": false,
    })
}
