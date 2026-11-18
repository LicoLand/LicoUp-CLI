use anyhow::{Context, Result, anyhow, bail, ensure};
use semver::Version;
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::{
    fs::{self, File},
    path::{Path, PathBuf},
};

use super::{
    MigrationFrontier, ReleaseTrack, frontier_projection_for, running_product_version,
    update_handoff, write_json_atomic,
};

pub(super) const UPDATE_HANDOFF_SCHEMA: &str = "v0.0.1:client-update-handoff-1";

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct UpdateHandoff {
    pub(super) schema_version: String,
    pub(super) state: update_handoff::State,
    pub(super) version: String,
    pub(super) target_release_track: String,
    pub(super) migration_frontier: serde_json::Value,
    pub(super) receipt_id: String,
    pub(super) target_path: String,
    pub(super) backup_path: String,
}

pub(super) fn claim_update_handoff(path: &Path, frontier: &MigrationFrontier) -> Result<()> {
    let Some(raw) = licoup_foundation::platform::file_security::read_existing_private_text_bounded(
        path,
        256 * 1024,
    )
    .context("update_handoff_mismatch")?
    else {
        return Ok(());
    };
    let mut handoff: UpdateHandoff =
        serde_json::from_str(&raw).context("update_handoff_mismatch")?;
    ensure!(
        handoff.schema_version == UPDATE_HANDOFF_SCHEMA
            && handoff.version == running_product_version()?
            && handoff.target_release_track == ReleaseTrack::running()?.as_str()
            && handoff.migration_frontier == frontier_projection_for(frontier)
            && handoff.receipt_id.starts_with("sha256:")
            && handoff.receipt_id.len() == 71,
        "update_handoff_mismatch"
    );
    let target_path = PathBuf::from(&handoff.target_path);
    let backup_path = PathBuf::from(&handoff.backup_path);
    ensure!(
        target_path.is_absolute()
            && backup_path == pre_claim_backup_path(&target_path, &handoff.receipt_id)?,
        "update_handoff_mismatch"
    );
    if handoff.state == update_handoff::State::Pending {
        let claimed = update_handoff::transition(handoff.state, update_handoff::Event::Claim)
            .ok_or_else(|| anyhow!("update_handoff_mismatch"))?;
        handoff.state = claimed;
        write_json_atomic(path, &handoff).context("update_handoff_mismatch")?;
    }
    remove_pre_claim_backup(&backup_path)?;
    Ok(())
}

pub(super) fn update_handoff_is_claimed(path: &Path) -> bool {
    let Ok(Some(raw)) =
        licoup_foundation::platform::file_security::read_existing_private_text_bounded(
            path,
            256 * 1024,
        )
    else {
        return false;
    };
    serde_json::from_str::<UpdateHandoff>(&raw)
        .is_ok_and(|handoff| handoff.state == update_handoff::State::Claimed)
}

pub(crate) fn prepare_update_handoff(
    data_root: &Path,
    receipt: &serde_json::Value,
    target_path: &Path,
) -> Result<super::PreparedUpdateHandoff> {
    ensure!(data_root.is_absolute(), "update_handoff_mismatch");
    ensure!(target_path.is_absolute(), "update_handoff_mismatch");
    let version = receipt
        .get("version")
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| anyhow!("update_handoff_mismatch"))?;
    Version::parse(version).context("update_handoff_mismatch")?;
    let target_release_track = receipt
        .get("targetReleaseTrack")
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| anyhow!("update_handoff_mismatch"))?;
    ensure!(
        matches!(target_release_track, "nightly" | "stable"),
        "update_handoff_mismatch"
    );
    let migration_frontier = receipt
        .get("migrationFrontier")
        .cloned()
        .ok_or_else(|| anyhow!("update_handoff_mismatch"))?;
    ensure!(
        migration_frontier
            .get("frontierId")
            .and_then(serde_json::Value::as_str)
            .is_some_and(|value| !value.is_empty())
            && migration_frontier
                .get("domains")
                .and_then(serde_json::Value::as_array)
                .is_some_and(|domains| !domains.is_empty()),
        "update_handoff_mismatch"
    );
    let receipt_id = receipt
        .get("receiptId")
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| anyhow!("update_handoff_mismatch"))?;
    ensure!(
        receipt_id.starts_with("sha256:") && receipt_id.len() == 71,
        "update_handoff_mismatch"
    );
    let backup_path = pre_claim_backup_path(target_path, receipt_id)?;
    let handoff = UpdateHandoff {
        schema_version: UPDATE_HANDOFF_SCHEMA.to_owned(),
        state: update_handoff::State::Pending,
        version: version.to_owned(),
        target_release_track: target_release_track.to_owned(),
        migration_frontier,
        receipt_id: receipt_id.to_owned(),
        target_path: target_path.to_string_lossy().into_owned(),
        backup_path: backup_path.to_string_lossy().into_owned(),
    };
    let path = data_root.join("client-state/migrations/update-handoff.json");
    ensure!(!path.exists(), "update_handoff_mismatch");
    let rejected = update_handoff_rejection_path(&path)?;
    if rejected.exists() {
        let metadata = fs::symlink_metadata(&rejected).context("update_handoff_mismatch")?;
        ensure!(
            metadata.is_file() && !metadata.file_type().is_symlink(),
            "update_handoff_mismatch"
        );
        fs::remove_file(&rejected).context("update_handoff_mismatch")?;
    }
    write_json_atomic(&path, &handoff).context("update_handoff_mismatch")?;
    Ok(super::PreparedUpdateHandoff {
        handoff_path: path,
        backup_path,
    })
}

pub(super) fn update_handoff_rejection_path(path: &Path) -> Result<PathBuf> {
    let name = path
        .file_name()
        .and_then(|value| value.to_str())
        .ok_or_else(|| anyhow!("update_handoff_mismatch"))?;
    Ok(path.with_file_name(format!("{name}.rejected")))
}

pub(super) fn write_update_handoff_rejection(path: &Path) -> Result<()> {
    write_json_atomic(
        &update_handoff_rejection_path(path)?,
        &json!({
            "schemaVersion": "v0.0.1:client-update-handoff-rejection-1",
            "status": "rejected"
        }),
    )
    .context("update_handoff_mismatch")
}

pub(super) fn pre_claim_backup_path(target_path: &Path, receipt_id: &str) -> Result<PathBuf> {
    let parent = target_path
        .parent()
        .ok_or_else(|| anyhow!("update_handoff_mismatch"))?;
    let target_name = target_path
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| anyhow!("update_handoff_mismatch"))?;
    let binding = receipt_id
        .strip_prefix("sha256:")
        .filter(|value| value.len() == 64 && value.bytes().all(|byte| byte.is_ascii_hexdigit()))
        .ok_or_else(|| anyhow!("update_handoff_mismatch"))?;
    Ok(parent.join(format!(".{target_name}.{binding}.pre-claim")))
}

fn remove_pre_claim_backup(path: &Path) -> Result<()> {
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error).context("update_handoff_mismatch"),
    };
    ensure!(
        !metadata.file_type().is_symlink(),
        "update_handoff_mismatch"
    );
    if metadata.is_dir() {
        fs::remove_dir_all(path).context("update_handoff_mismatch")?;
    } else if metadata.is_file() {
        fs::remove_file(path).context("update_handoff_mismatch")?;
    } else {
        bail!("update_handoff_mismatch");
    }
    if let Some(parent) = path.parent() {
        File::open(parent)
            .and_then(|directory| directory.sync_all())
            .context("update_handoff_mismatch")?;
    }
    Ok(())
}
