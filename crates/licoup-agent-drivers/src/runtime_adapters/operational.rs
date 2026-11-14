//! Operational evidence: the verdict of the last real conversation turn this
//! host actually ran for each Agent.
//!
//! A completed real turn is itself the verification a lane can carry — one
//! genuine end-to-end success proves the path works, and a genuine failure is
//! fresher evidence than any packaged claim. The dedicated release-evidence
//! document remains a separate diagnostic record; it is one evidence source,
//! never the definition of whether a lane works, and never an admission gate.
//!
//! The record is bounded, carries no message content or local paths, and is
//! written only where a lane was actually engaged and returned a verdict.
//! Pre-invocation rejections (missing executable, unsatisfied requirements,
//! absent lane) leave no trace here because they say nothing about whether
//! the lane itself works.

use licoup_client_state::paths::{portable_state_root, portable_state_root_read_only};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

const OPERATIONS_FILE: &str = "agent-conversation-operations.json";
const OPERATIONS_SCHEMA: &str = "v0.0.1:client-agent-conversation-operations-1";
const MAX_OPERATIONS_BYTES: u64 = 256 * 1024;
const MAX_AGENTS: usize = 64;
const OUTCOME_SUCCESS: &str = "success";

#[derive(Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct OperationsDocument {
    #[serde(default)]
    schema_version: String,
    #[serde(default)]
    agents: BTreeMap<String, AgentOperations>,
}

#[derive(Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct AgentOperations {
    /// Verdict of the most recent engaged turn: `success` or `failure`.
    #[serde(default)]
    last_outcome: String,
    #[serde(default)]
    success_count: u64,
    /// Wall-clock time of the most recent engaged turn, milliseconds since epoch.
    #[serde(default)]
    last_unix_ms: u64,
}

/// Whether the running build may touch the real data home. Test binaries
/// never read or write operational evidence in a real data home; a suite
/// that exercises the record binds a synthetic root through
/// `set_portable_data_dir_override`, exactly the way every other
/// owned-state seam isolates itself.
#[cfg(any(test, feature = "test-support"))]
fn evidence_binding_present() -> bool {
    matches!(
        licoup_foundation::platform::paths::selected_data_home().map(|selection| selection.source),
        Ok(licoup_foundation::platform::paths::DataHomeSource::TestOverride)
    )
}

/// A read resolves the root lexically and never creates a data home just to
/// answer "no recorded verdict".
fn readable_state_root() -> Option<PathBuf> {
    #[cfg(any(test, feature = "test-support"))]
    if !evidence_binding_present() {
        return None;
    }
    portable_state_root_read_only().ok()
}

fn writable_state_root() -> Option<PathBuf> {
    #[cfg(any(test, feature = "test-support"))]
    if !evidence_binding_present() {
        return None;
    }
    portable_state_root().ok()
}

fn read_operations_document(root: &Path) -> Option<OperationsDocument> {
    let path = root.join(OPERATIONS_FILE);
    let metadata = std::fs::metadata(&path).ok()?;
    if metadata.len() > MAX_OPERATIONS_BYTES {
        return None;
    }
    let bytes = std::fs::read(&path).ok()?;
    let document: OperationsDocument = serde_json::from_slice(&bytes).ok()?;
    if document.schema_version != OPERATIONS_SCHEMA {
        return None;
    }
    Some(document)
}

/// The verdict of the last real turn the lane was engaged for: `Some(true)`
/// when it succeeded, `Some(false)` when it failed, `None` when this host has
/// no recorded verdict for the Agent.
pub fn last_turn_verdict(agent_id: &str) -> Option<bool> {
    let root = readable_state_root()?;
    read_operations_document(&root)?
        .agents
        .get(agent_id)
        .map(|entry| entry.last_outcome == OUTCOME_SUCCESS)
}

/// Record the verdict of one engaged turn. Called only where the lane was
/// actually invoked; failures to reach the lane record nothing. A write
/// failure degrades the evidence record only — it never fails the turn that
/// produced the verdict.
pub fn record_turn_verdict(agent_id: &str, succeeded: bool) {
    let Some(root) = writable_state_root() else {
        return;
    };
    let mut document = read_operations_document(&root).unwrap_or_else(|| OperationsDocument {
        schema_version: OPERATIONS_SCHEMA.to_owned(),
        agents: BTreeMap::new(),
    });
    if !document.agents.contains_key(agent_id) && document.agents.len() >= MAX_AGENTS {
        return;
    }
    let entry = document.agents.entry(agent_id.to_owned()).or_default();
    entry.last_outcome = if succeeded {
        OUTCOME_SUCCESS
    } else {
        "failure"
    }
    .to_owned();
    if succeeded {
        entry.success_count = entry.success_count.saturating_add(1);
    }
    entry.last_unix_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_millis() as u64)
        .unwrap_or(0);

    if std::fs::create_dir_all(&root).is_err() {
        return;
    }
    let path = root.join(OPERATIONS_FILE);
    let Ok(serialized) = serde_json::to_vec(&document) else {
        return;
    };
    let staging = path.with_extension("tmp");
    if std::fs::write(&staging, serialized).is_ok() {
        let _ = std::fs::rename(&staging, &path);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use licoup_foundation::platform::paths::set_portable_data_dir_override;
    use std::path::PathBuf;

    // The data-dir override is thread-local, so parallel tests never share a
    // root; the guard restores the caller's previous selection and drops the
    // synthetic root.
    struct DataDirGuard {
        previous: Option<PathBuf>,
        root: PathBuf,
    }

    impl DataDirGuard {
        fn new() -> Self {
            let root = std::env::temp_dir().join(format!(
                "licoup-operations-{}-{}",
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_nanos()
            ));
            std::fs::create_dir_all(&root).expect("temp root");
            let previous = set_portable_data_dir_override(Some(root.clone()));
            Self { previous, root }
        }
    }

    impl Drop for DataDirGuard {
        fn drop(&mut self) {
            set_portable_data_dir_override(self.previous.take());
            let _ = std::fs::remove_dir_all(&self.root);
        }
    }

    #[test]
    fn verdict_is_unknown_before_any_turn() {
        let _guard = DataDirGuard::new();
        assert_eq!(last_turn_verdict("codex"), None);
    }

    #[test]
    fn successful_turn_marks_lane_verified() {
        let _guard = DataDirGuard::new();
        record_turn_verdict("codex", true);
        assert_eq!(last_turn_verdict("codex"), Some(true));
    }

    #[test]
    fn latest_verdict_wins_in_both_directions() {
        let _guard = DataDirGuard::new();
        record_turn_verdict("codex", true);
        record_turn_verdict("codex", false);
        assert_eq!(last_turn_verdict("codex"), Some(false));
        record_turn_verdict("codex", true);
        assert_eq!(last_turn_verdict("codex"), Some(true));
    }

    #[test]
    fn verdicts_are_per_agent() {
        let _guard = DataDirGuard::new();
        record_turn_verdict("codex", true);
        assert_eq!(last_turn_verdict("claude-code"), None);
    }
}
