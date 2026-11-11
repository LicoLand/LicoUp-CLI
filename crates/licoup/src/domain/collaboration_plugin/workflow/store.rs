use super::super::lifecycle::{collaboration_root, epoch_seconds};
use super::model::{MAX_ACTIVE_WORKFLOW_PLANS, MAX_WORKFLOW_PLAN_BYTES, WorkflowPlanRecord};
use anyhow::{Result, anyhow, ensure};
use std::fs;
use std::path::{Path, PathBuf};
use uuid::Uuid;

use crate::platform::client_state::ClientStateStore;
use licoup_foundation::platform::file_security::{
    atomic_write_private_text_bounded, ensure_private_dir,
};

pub(super) struct ClaimedPlan {
    pub(super) root: PathBuf,
    pub(super) record: WorkflowPlanRecord,
}

pub(super) fn prepare_new_plan(store: &ClientStateStore) -> Result<()> {
    cleanup_plans(store)
}

pub(super) fn persist_plan(store: &ClientStateStore, record: &WorkflowPlanRecord) -> Result<()> {
    record.validate()?;
    let root = plan_root(store, &record.plan_id)?;
    ensure!(!root.exists(), "collaboration_workflow_plan_exists");
    ensure_private_dir(&root)?;
    let result = write_record(&root, record);
    if result.is_err() {
        let _ = fs::remove_dir_all(root);
    }
    result
}

pub(super) fn claim_plan(store: &ClientStateStore, plan_id: &str) -> Result<ClaimedPlan> {
    let source = plan_root(store, plan_id)?;
    let claimed = plans_root(store)?.join(format!(".applying-{plan_id}-{}", Uuid::new_v4()));
    fs::rename(&source, &claimed)
        .map_err(|_| anyhow!("collaboration_workflow_plan_missing_or_consumed"))?;
    match read_record(&claimed) {
        Ok(record) => Ok(ClaimedPlan {
            root: claimed,
            record,
        }),
        Err(error) => {
            let _ = consume_claim_path(store, &claimed, false);
            Err(error)
        }
    }
}

pub(super) fn finish_claim(
    store: &ClientStateStore,
    claim: &ClaimedPlan,
    simulate_cleanup_failure: bool,
) -> bool {
    consume_claim_path(store, &claim.root, simulate_cleanup_failure).unwrap_or(true)
}

pub(super) fn abandon_claim(store: &ClientStateStore, claim: &ClaimedPlan) {
    let _ = consume_claim_path(store, &claim.root, false);
}

pub(super) fn restore_claim(store: &ClientStateStore, claim: &ClaimedPlan) -> Result<()> {
    let original = plan_root(store, &claim.record.plan_id)?;
    ensure!(
        !original.exists(),
        "collaboration_workflow_plan_restore_conflict"
    );
    fs::rename(&claim.root, original)
        .map_err(|_| anyhow!("collaboration_workflow_plan_restore_failed"))
}

pub(super) fn cancel_claim(store: &ClientStateStore, claim: &ClaimedPlan) -> Result<()> {
    let cancelled = plans_root(store)?.join(format!(
        ".cancelled-{}-{}",
        claim.record.plan_id,
        Uuid::new_v4()
    ));
    fs::rename(&claim.root, &cancelled)
        .map_err(|_| anyhow!("collaboration_workflow_cancel_prepare_failed"))?;
    if let Err(error) = fs::remove_dir_all(&cancelled) {
        let original = plan_root(store, &claim.record.plan_id)?;
        if !original.exists() {
            let _ = fs::rename(&cancelled, &original);
        }
        return Err(error.into());
    }
    Ok(())
}

fn consume_claim_path(
    store: &ClientStateStore,
    claim_root: &Path,
    simulate_cleanup_failure: bool,
) -> Result<bool> {
    let consumed = plans_root(store)?.join(format!(".consumed-{}", Uuid::new_v4()));
    let cleanup_root = if fs::rename(claim_root, &consumed).is_ok() {
        consumed
    } else {
        claim_root.to_path_buf()
    };
    if simulate_cleanup_failure {
        return Ok(true);
    }
    Ok(cleanup_root.exists() && fs::remove_dir_all(cleanup_root).is_err())
}

fn cleanup_plans(store: &ClientStateStore) -> Result<()> {
    let root = plans_root(store)?;
    let now = epoch_seconds();
    let mut active = 0usize;
    for entry in fs::read_dir(&root)? {
        let entry = entry?;
        let path = entry.path();
        let metadata = fs::symlink_metadata(&path)?;
        ensure!(
            metadata.is_dir() && !metadata.file_type().is_symlink(),
            "collaboration_workflow_plan_entry_invalid"
        );
        let name = entry.file_name();
        let name = name
            .to_str()
            .ok_or_else(|| anyhow!("collaboration_workflow_plan_entry_invalid"))?;
        if name.starts_with(".consumed-") || name.starts_with(".cancelled-") {
            fs::remove_dir_all(path)
                .map_err(|_| anyhow!("collaboration_workflow_plan_cleanup_failed"))?;
            continue;
        }
        let record = read_record(&path);
        match record {
            Ok(record) if record.expires_at_epoch_seconds > now => active += 1,
            _ => fs::remove_dir_all(path)
                .map_err(|_| anyhow!("collaboration_workflow_plan_cleanup_failed"))?,
        }
    }
    ensure!(
        active < MAX_ACTIVE_WORKFLOW_PLANS,
        "collaboration_workflow_active_plan_limit_reached"
    );
    Ok(())
}

fn plans_root(store: &ClientStateStore) -> Result<PathBuf> {
    let root = collaboration_root(store).join("workflow-plans");
    ensure_private_dir(&root)?;
    Ok(root)
}

fn plan_root(store: &ClientStateStore, plan_id: &str) -> Result<PathBuf> {
    let parsed =
        Uuid::parse_str(plan_id).map_err(|_| anyhow!("collaboration_workflow_plan_id_invalid"))?;
    ensure!(
        parsed.to_string() == plan_id,
        "collaboration_workflow_plan_id_invalid"
    );
    Ok(plans_root(store)?.join(plan_id))
}

fn write_record(root: &Path, record: &WorkflowPlanRecord) -> Result<()> {
    let text = serde_json::to_string(record)?;
    atomic_write_private_text_bounded(&root.join("plan.json"), &text, MAX_WORKFLOW_PLAN_BYTES)
}

fn read_record(root: &Path) -> Result<WorkflowPlanRecord> {
    let bytes = fs::read(root.join("plan.json"))
        .map_err(|_| anyhow!("collaboration_workflow_plan_missing_or_consumed"))?;
    ensure!(
        bytes.len() <= MAX_WORKFLOW_PLAN_BYTES,
        "collaboration_workflow_plan_too_large"
    );
    let record: WorkflowPlanRecord = serde_json::from_slice(&bytes)
        .map_err(|_| anyhow!("collaboration_workflow_plan_invalid"))?;
    record.validate()?;
    Ok(record)
}

#[cfg(test)]
pub(super) fn rewrite_record_for_test(
    store: &ClientStateStore,
    plan_id: &str,
    update: impl FnOnce(&mut WorkflowPlanRecord),
) -> Result<WorkflowPlanRecord> {
    let root = plan_root(store, plan_id)?;
    let mut record = read_record(&root)?;
    update(&mut record);
    record.seal()?;
    write_record(&root, &record)?;
    Ok(record)
}
