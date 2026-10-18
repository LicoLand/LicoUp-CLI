use std::fs;

use anyhow::{Result, ensure};
use serde_json::{Value, json};

use super::{
    constants::CLIENT_UPDATE_MODE,
    params::{product_version, staging_root, target_release_track},
};
use crate::domain::work_admission::WorkAdmission;

pub fn status(params: &Value) -> Result<Value> {
    let staging = staging_root(params)?;
    let mut staged_artifact_count = 0_u64;
    let mut staged_bytes = 0_u64;
    if staging.is_dir() {
        for entry in fs::read_dir(&staging)? {
            let entry = entry?;
            let metadata = fs::symlink_metadata(entry.path())?;
            ensure!(
                !metadata.file_type().is_symlink(),
                "client update staging state contains a symbolic link"
            );
            if metadata.is_file() {
                staged_artifact_count = staged_artifact_count.saturating_add(1);
                staged_bytes = staged_bytes.saturating_add(metadata.len());
            }
        }
    }
    let (phase, production_ready, admission) = maintenance_admission();
    Ok(json!({
        "ok": true,
        "mode": CLIENT_UPDATE_MODE,
        "phase": phase,
        "runningReleaseTrack": crate::domain::client_state_migration::ReleaseTrack::running()?.as_str(),
        "targetReleaseTrack": target_release_track(params)?.as_str(),
        "runningVersion": product_version(params)?,
        "productionReady": production_ready,
        "admission": admission,
        "publicMetadataOnly": true,
        "storeCredentialsRequired": false,
        "stagingRootRedacted": true,
        "stagedArtifactCount": staged_artifact_count,
        "stagedBytes": staged_bytes,
        "policy": {
            "manualCheck": true,
            "automaticDownload": false,
            "automaticInstall": false
        }
    }))
}

/// The host's own idle-admission answer, projected onto this status.
///
/// The phase is `idle` only while this host owns no unfinished local work and
/// holds no close-admission barrier. A host that still owns work, or that
/// already has another maintenance operation holding the barrier, is reported
/// `blocked`, and only an idle answer reports the switch as production ready.
///
/// The decision is read from this host's own data root through the read-only
/// resolver, because this verb observes installed state and must not create or
/// harden it. An answer this host cannot read is not permission: the caller is
/// told nothing (`admission` is `null`) and the switch is reported locked, which
/// is the same fail-closed default the client already applies to a host that did
/// not answer. Blockers are the bounded facts the decision owner publishes —
/// owner, kind, scope, identity and stored state — never paths or payloads.
fn maintenance_admission() -> (&'static str, bool, Value) {
    let admission = licoup_foundation::platform::paths::portable_data_dir_read_only()
        .and_then(|data_root| WorkAdmission::open(data_root).admission());
    match admission {
        Ok(admission) if admission.decision.allows_maintenance() => {
            ("idle", true, json!(admission))
        }
        Ok(admission) => ("blocked", false, json!(admission)),
        Err(_) => ("blocked", false, Value::Null),
    }
}
