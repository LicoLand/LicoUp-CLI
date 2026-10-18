use anyhow::{Result, anyhow, ensure};
use licoup_foundation::platform::file_security::ensure_private_dir;
use serde_json::{Value, json};
use std::fs;
use std::path::{Path, PathBuf};

use super::{paths, policy, redaction, serialization};

#[derive(Clone, Debug)]
pub struct SnapshotStore {
    root: PathBuf,
}

#[derive(Clone, Debug)]
pub struct SnapshotRecord {
    pub snapshot_id: String,
    pub snapshot_path: PathBuf,
    pub source_path: PathBuf,
    pub existed: bool,
    pub content: String,
}

impl SnapshotStore {
    pub fn portable() -> Result<Self> {
        let root = paths::portable_state_root()?;
        ensure_private_dir(&root)?;
        ensure_private_dir(&paths::snapshot_root(&root))?;
        Ok(Self::from_state_root(&root))
    }

    pub fn from_state_root(root: &Path) -> Self {
        Self {
            root: paths::snapshot_root(root),
        }
    }

    pub fn capture(
        &self,
        target: &str,
        source_path: &Path,
        metadata: Value,
    ) -> Result<SnapshotRecord> {
        validate_target(target)?;
        ensure_private_dir(&self.root)?;
        let content =
            paths::read_owned_local_text_bounded(source_path, policy::MAX_SNAPSHOT_SOURCE_BYTES)?;
        let existed = content.is_some();
        let content = content.unwrap_or_default();
        let redacted = redaction::redact_snapshot(&content, metadata)?;
        let snapshot_id = format!(
            "snapshot-{}-{}",
            serialization::sanitize_id(target),
            serialization::timestamp()
        );
        let snapshot_path = paths::snapshot_path(&self.root, &snapshot_id)?;
        let source_path_text = source_path
            .to_str()
            .ok_or_else(|| anyhow!("local snapshot path is not UTF-8"))?;
        let record = json!({
            "schemaVersion": policy::STATE_SCHEMA_VERSION,
            "snapshotId": snapshot_id,
            "target": target,
            "sourcePath": source_path_text,
            "capturedAt": serialization::timestamp(),
            "existed": existed,
            "size": content.len(),
            "hash": serialization::hash_text(&content),
            "content": redacted.content,
            "redaction": redacted.evidence,
            "metadata": redacted.metadata,
        });
        serialization::atomic_write_json(
            &snapshot_path,
            &record,
            policy::MAX_SNAPSHOT_RECORD_BYTES,
        )?;
        Ok(SnapshotRecord {
            snapshot_id,
            snapshot_path,
            source_path: source_path.to_path_buf(),
            existed,
            content: record
                .get("content")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string(),
        })
    }

    pub fn list(&self, filter: &Value) -> Result<Value> {
        ensure_private_dir(&self.root)?;
        let mut snapshots = Vec::<Value>::new();
        let mut snapshot_files = 0usize;
        for entry in fs::read_dir(&self.root)? {
            let entry = entry?;
            if entry.path().extension().and_then(|item| item.to_str()) != Some("json") {
                continue;
            }
            snapshot_files = snapshot_files.saturating_add(1);
            ensure!(
                snapshot_files <= policy::MAX_SNAPSHOT_FILES,
                "snapshot catalog exceeds its bounded size"
            );
            let snapshot = serialization::read_json_or_default(
                &entry.path(),
                policy::MAX_SNAPSHOT_RECORD_BYTES,
                || json!({}),
            )?;
            if matches_snapshot_filter(&snapshot, filter) {
                snapshots.push(snapshot_summary(&snapshot, entry.path()));
            }
        }
        snapshots.sort_by(|left, right| {
            let left_key = (
                left.get("capturedAt")
                    .and_then(Value::as_str)
                    .unwrap_or_default(),
                left.get("snapshotId")
                    .and_then(Value::as_str)
                    .unwrap_or_default(),
            );
            let right_key = (
                right
                    .get("capturedAt")
                    .and_then(Value::as_str)
                    .unwrap_or_default(),
                right
                    .get("snapshotId")
                    .and_then(Value::as_str)
                    .unwrap_or_default(),
            );
            left_key.cmp(&right_key)
        });
        Ok(json!({
            "ok": true,
            "schemaVersion": policy::STATE_SCHEMA_VERSION,
            "path": policy::SNAPSHOT_DIR,
            "snapshots": snapshots
        }))
    }

    pub fn restore(&self, snapshot_id: &str) -> Result<Value> {
        let snapshot_path = paths::snapshot_path(&self.root, snapshot_id)?;
        let snapshot = serialization::read_json_or_default(
            &snapshot_path,
            policy::MAX_SNAPSHOT_RECORD_BYTES,
            || json!({}),
        )?;
        ensure!(
            snapshot.get("snapshotId").and_then(Value::as_str) == Some(snapshot_id),
            "snapshot identity is missing or mismatched"
        );
        let source_path = snapshot
            .get("sourcePath")
            .and_then(Value::as_str)
            .map(PathBuf::from)
            .ok_or_else(|| anyhow!("snapshot source path is missing"))?;
        paths::validate_restore_destination(&source_path)?;
        let existed = snapshot
            .get("existed")
            .and_then(Value::as_bool)
            .unwrap_or(true);
        let content = snapshot
            .get("content")
            .and_then(Value::as_str)
            .unwrap_or_default();
        ensure!(
            content.len() <= policy::MAX_SNAPSHOT_SOURCE_BYTES,
            "snapshot restore content exceeds its bounded size"
        );
        let redaction_applied = snapshot
            .get("redaction")
            .and_then(|item| item.get("applied"))
            .and_then(Value::as_bool)
            .unwrap_or(false);
        let pre_restore = self.capture(
            snapshot
                .get("target")
                .and_then(Value::as_str)
                .unwrap_or("restore"),
            &source_path,
            json!({
                "reason": "pre-restore",
                "restoringSnapshotId": snapshot_id
            }),
        )?;
        if existed {
            serialization::atomic_write_local_text_bounded(
                &source_path,
                content,
                policy::MAX_SNAPSHOT_SOURCE_BYTES,
            )?;
        } else {
            paths::remove_owned_local_file_if_present(&source_path)?;
        }
        Ok(json!({
            "ok": true,
            "status": "restored",
            "snapshotId": snapshot_id,
            "snapshotPath": paths::internal_state_reference(policy::SNAPSHOT_DIR, &snapshot_path),
            "sourcePath": paths::redacted_local_path(),
            "preRestoreSnapshotId": pre_restore.snapshot_id,
            "preRestoreSnapshotPath": paths::internal_state_reference(
                policy::SNAPSHOT_DIR,
                &pre_restore.snapshot_path
            ),
            "redactionApplied": redaction_applied
        }))
    }
}

fn validate_target(target: &str) -> Result<()> {
    ensure!(
        !target.is_empty()
            && target.len() <= 128
            && target.bytes().all(|byte| {
                byte.is_ascii_alphanumeric() || byte == b'.' || byte == b'-' || byte == b'_'
            }),
        "snapshot target is invalid"
    );
    Ok(())
}

fn matches_snapshot_filter(snapshot: &Value, filter: &Value) -> bool {
    filter
        .get("target")
        .and_then(Value::as_str)
        .map(|expected| snapshot.get("target").and_then(Value::as_str) == Some(expected))
        .unwrap_or(true)
}

fn snapshot_summary(snapshot: &Value, path: PathBuf) -> Value {
    json!({
        "schemaVersion": snapshot
            .get("schemaVersion")
            .cloned()
            .unwrap_or_else(|| json!(policy::STATE_SCHEMA_VERSION)),
        "snapshotId": snapshot.get("snapshotId").cloned().unwrap_or_else(|| json!("")),
        "target": snapshot.get("target").cloned().unwrap_or_else(|| json!("")),
        "sourcePath": paths::redacted_local_path(),
        "capturedAt": snapshot.get("capturedAt").cloned().unwrap_or_else(|| json!("")),
        "existed": snapshot.get("existed").cloned().unwrap_or_else(|| json!(false)),
        "size": snapshot.get("size").cloned().unwrap_or_else(|| json!(0)),
        "hash": snapshot.get("hash").cloned().unwrap_or_else(|| json!("")),
        "redaction": snapshot.get("redaction").cloned().unwrap_or_else(|| json!({
            "policy": "known-credential-fields",
            "applied": false,
            "paths": []
        })),
        "snapshotPath": paths::internal_state_reference(policy::SNAPSHOT_DIR, &path)
    })
}
