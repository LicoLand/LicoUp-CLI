//! Minimal private persistence for one-shot MCP transfer previews.

use crate::domain::mcp_adapter::McpApprovalPlanStore;
use anyhow::{Result, anyhow, ensure};
use fs2::FileExt;
use licoup_foundation::platform::file_security::{
    atomic_write_private_text_bounded, ensure_private_dir, open_private_lock_file,
    read_private_text_bounded,
};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};
use uuid::Uuid;

const PLAN_SCHEMA: &str = "licoup.mcp-transfer-plan.v2";
const PLAN_TTL_SECONDS: u64 = 120;
const MAX_PLAN_BYTES: usize = 1024;
const MAX_ACTIVE_PLANS: usize = 16;

pub(crate) struct PrivateMcpApprovalPlanStore {
    root: PathBuf,
}

#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct PlanRecord {
    schema_version: String,
    plan_id: String,
    approval_digest: String,
    kind: PlanKind,
    expires_at_epoch_seconds: u64,
}

#[derive(Clone, Copy, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
enum PlanKind {
    Preview,
}

impl PrivateMcpApprovalPlanStore {
    pub(crate) fn open_default() -> Result<Self> {
        Self::open(
            licoup_foundation::platform::paths::portable_data_dir()?.join("mcp-transfer-plans"),
        )
    }

    fn open(root: PathBuf) -> Result<Self> {
        ensure_private_dir(&root)?;
        Ok(Self { root })
    }

    fn path(&self, plan_id: &str) -> Result<PathBuf> {
        validate_plan_id(plan_id)?;
        Ok(self.root.join(format!("{plan_id}.json")))
    }

    fn lock(&self) -> Result<fs::File> {
        let lock = open_private_lock_file(&self.root.join(".lock"))?;
        lock.lock_exclusive()
            .map_err(|_| anyhow!("mcp_transfer_plan_store_unavailable"))?;
        Ok(lock)
    }

    fn cleanup_and_count(&self) -> Result<usize> {
        let now = epoch_seconds()?;
        let mut active = 0usize;
        for entry in fs::read_dir(&self.root)? {
            let entry = entry?;
            let path = entry.path();
            if entry.file_name().to_str() == Some(".lock") {
                continue;
            }
            let metadata = fs::symlink_metadata(&path)?;
            ensure!(
                metadata.is_file() && !metadata.file_type().is_symlink(),
                "mcp_transfer_plan_entry_invalid"
            );
            let record = read_record(&path);
            if record.is_ok_and(|record| record.expires_at_epoch_seconds > now) {
                active += 1;
            } else {
                fs::remove_file(path).map_err(|_| anyhow!("mcp_transfer_plan_cleanup_failed"))?;
            }
        }
        Ok(active)
    }
}

impl McpApprovalPlanStore for PrivateMcpApprovalPlanStore {
    fn stage(&self, approval_digest: &str) -> Result<String> {
        validate_digest(approval_digest)?;
        let _lock = self.lock()?;
        ensure!(
            self.cleanup_and_count()? < MAX_ACTIVE_PLANS,
            "mcp_transfer_plan_limit_reached"
        );
        let plan_id = Uuid::new_v4().to_string();
        let record = PlanRecord {
            schema_version: PLAN_SCHEMA.to_owned(),
            plan_id: plan_id.clone(),
            approval_digest: approval_digest.to_owned(),
            kind: PlanKind::Preview,
            expires_at_epoch_seconds: epoch_seconds()?
                .checked_add(PLAN_TTL_SECONDS)
                .ok_or_else(|| anyhow!("mcp_transfer_plan_expiry_invalid"))?,
        };
        let text = serde_json::to_string(&record)?;
        atomic_write_private_text_bounded(&self.path(&plan_id)?, &text, MAX_PLAN_BYTES)?;
        Ok(plan_id)
    }

    fn claim(&self, plan_id: &str) -> Result<String> {
        self.claim_kind(plan_id, PlanKind::Preview)
    }
}

impl PrivateMcpApprovalPlanStore {
    fn claim_kind(&self, plan_id: &str, expected: PlanKind) -> Result<String> {
        let _lock = self.lock()?;
        let source = self.path(plan_id)?;
        let claimed = self.root.join(format!(".claimed-{}.json", Uuid::new_v4()));
        fs::rename(&source, &claimed).map_err(|_| anyhow!("mcp_transfer_plan_missing_or_used"))?;
        let outcome = read_record(&claimed).and_then(|record| {
            ensure!(
                record.plan_id == plan_id && record.kind == expected,
                "mcp_transfer_plan_invalid"
            );
            ensure!(
                record.expires_at_epoch_seconds > epoch_seconds()?,
                "mcp_transfer_plan_expired"
            );
            Ok(record.approval_digest)
        });
        let _ = fs::remove_file(&claimed);
        outcome
    }
}

fn read_record(path: &Path) -> Result<PlanRecord> {
    let text = read_private_text_bounded(path, MAX_PLAN_BYTES)?
        .ok_or_else(|| anyhow!("mcp_transfer_plan_missing_or_used"))?;
    let record: PlanRecord =
        serde_json::from_str(&text).map_err(|_| anyhow!("mcp_transfer_plan_invalid"))?;
    ensure!(
        record.schema_version == PLAN_SCHEMA,
        "mcp_transfer_plan_invalid"
    );
    validate_plan_id(&record.plan_id)?;
    validate_digest(&record.approval_digest)?;
    Ok(record)
}

fn validate_plan_id(plan_id: &str) -> Result<()> {
    let parsed = Uuid::parse_str(plan_id).map_err(|_| anyhow!("mcp_transfer_plan_id_invalid"))?;
    ensure!(
        parsed.to_string() == plan_id,
        "mcp_transfer_plan_id_invalid"
    );
    Ok(())
}

fn validate_digest(digest: &str) -> Result<()> {
    ensure!(
        digest.len() == 64 && digest.bytes().all(|byte| byte.is_ascii_hexdigit()),
        "mcp_transfer_approval_digest_invalid"
    );
    Ok(())
}

fn epoch_seconds() -> Result<u64> {
    Ok(SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|_| anyhow!("mcp_transfer_clock_invalid"))?
        .as_secs())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn private_plan_is_claimed_exactly_once() {
        let root = std::env::temp_dir().join(format!("lico-mcp-plan-{}", Uuid::new_v4()));
        let store = PrivateMcpApprovalPlanStore::open(root.clone()).unwrap();
        let digest = "a".repeat(64);
        let plan_id = store.stage(&digest).unwrap();
        assert_eq!(store.claim(&plan_id).unwrap(), digest);
        assert!(store.claim(&plan_id).is_err());
        let _ = fs::remove_dir_all(root);
    }
}
