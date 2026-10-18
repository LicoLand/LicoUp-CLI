//! The durable record of one package-owned conversion.
//!
//! This is the tool's existing run record, applied to the other kind of run it can
//! drive. It lives in the caller's working root rather than in the data root,
//! because the standalone conversion promises the data root is only ever read:
//! the journal is the tool's own state, and the source is the operator's.
//!
//! What it records is deliberately narrow. It names the package, its version, the
//! entry and the format pair the run declared, the digest of the installed payload
//! it was admitted under, and the digest of the source root as the run read it.
//! A later resume compares all of those against what it is asked to continue, so a
//! journal can never be used to run a different converter over a different root
//! and call it the same migration.
//!
//! Two steps are recorded, in order:
//!
//! * `stageSource` — the source root copied into the working root, recorded with
//!   the digest of the copy, so a resume reuses an intact copy instead of staging
//!   again and a copy interrupted part way is never trusted.
//! * `runConverter` — the converter invocation. It is recorded *before* the
//!   process starts and rewritten after it settles, so at every instant the
//!   document says either "not attempted", "attempted and may have stopped inside"
//!   or "committed". A process that stops inside the converter leaves the step in
//!   `running`, which is exactly what makes the next run a resume rather than a
//!   restart.
//!
//! Nothing here decides what a conversion means. Completion is only ever written
//! after the converter's own documented result document has been read and checked,
//! and a run that is not complete is never written as complete.

use crate::error::{COMMIT_UNSUPPORTED, PACKAGE_CONVERSION_MISMATCHED, ToolResult, marker_invalid};
use crate::journal::markers;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// The journal's schema identity.
pub const PACKAGE_JOURNAL_SCHEMA: &str = "v0.0.1:package-conversion-journal-1";

/// The step that stages the source root into the working root.
pub const STAGE_SOURCE: &str = "stageSource";
/// The step that runs the package's own converter.
pub const RUN_CONVERTER: &str = "runConverter";

/// Every step the run declares, in the order they are taken.
pub const STEPS: [&str; 2] = [STAGE_SOURCE, RUN_CONVERTER];

/// One step's durable state.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum StepStatus {
    /// Declared, not attempted.
    Pending,
    /// Attempted; the process may have stopped inside it.
    Running,
    /// Settled.
    Committed,
}

impl StepStatus {
    pub const fn is_settled(self) -> bool {
        matches!(self, Self::Committed)
    }
}

/// One step's durable entry.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StepEntry {
    pub status: StepStatus,
    /// How many times this step has been attempted.
    pub attempt: u32,
    /// Why an unsettled step is unsettled, in the tool's own vocabulary.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pending_reason: Option<String>,
}

/// The facts a run declares before it does anything.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RunPlan {
    pub package_id: String,
    pub package_version: String,
    pub entry: String,
    pub source_format: String,
    pub target_format: String,
    /// The digest of the source root as this run read it.
    pub source_digest: String,
    /// The digest the host recorded for the installed payload.
    pub record_digest: String,
    /// Whether the run was admitted against a verified signed index.
    pub index_verified: bool,
}

/// The durable journal document.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Journal {
    pub schema_version: String,
    pub status: String,
    pub started_at: String,
    pub updated_at: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub completed_at: Option<String>,
    pub package_id: String,
    pub package_version: String,
    pub entry: String,
    pub source_format: String,
    pub target_format: String,
    pub source_digest: String,
    pub record_digest: String,
    pub index_verified: bool,
    /// The digest of the staged source copy, once one is whole.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub staged_source_digest: Option<String>,
    /// The digest of the produced target, once a converter reported it complete.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target_digest: Option<String>,
    /// How many bytes the produced target holds.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target_bytes: Option<u64>,
    /// How many records the converter reported converting.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub converted_records: Option<u64>,
    /// How many converter invocations this run has attempted.
    pub attempts: u32,
    pub steps: BTreeMap<String, StepEntry>,
}

impl Journal {
    /// One step's entry.
    pub fn step(&self, step: &str) -> Option<&StepEntry> {
        self.steps.get(step)
    }

    /// Whether the step settled.
    pub fn settled(&self, step: &str) -> bool {
        self.step(step)
            .is_some_and(|entry| entry.status.is_settled())
    }

    /// Whether a converter invocation is already recorded, so the next one is a resume.
    pub fn converter_attempted(&self) -> bool {
        self.attempts > 0
    }

    /// Whether this journal is the record of a completed conversion.
    pub fn is_complete(&self) -> bool {
        self.status == "complete" && STEPS.iter().all(|step| self.settled(step))
    }

    /// Every step that still owes work.
    pub fn outstanding_steps(&self) -> Vec<&str> {
        STEPS
            .iter()
            .copied()
            .filter(|step| !self.settled(step))
            .collect()
    }

    /// Whether this journal is the record of the run `plan` declares.
    ///
    /// A resume continues one declared migration: another package, another entry,
    /// another pair or another source root is a different run, and acting on it
    /// would be the restart the resume exists to prevent.
    pub fn matches(&self, plan: &RunPlan) -> bool {
        self.package_id == plan.package_id
            && self.package_version == plan.package_version
            && self.entry == plan.entry
            && self.source_format == plan.source_format
            && self.target_format == plan.target_format
            && self.source_digest == plan.source_digest
            && self.record_digest == plan.record_digest
    }
}

/// The journal path inside one working root.
pub fn journal_path(work_root: &Path) -> PathBuf {
    work_root.join("journal").join("package-conversion.json")
}

/// Read the journal recorded in one working root.
pub fn open(work_root: &Path) -> ToolResult<Option<Journal>> {
    let path = journal_path(work_root);
    let Some(value) = markers::read_json::<serde_json::Value>(&path)? else {
        return Ok(None);
    };
    let document: Journal =
        serde_json::from_value(value).map_err(|_| marker_invalid("package journal"))?;
    if document.schema_version != PACKAGE_JOURNAL_SCHEMA {
        return Err(PACKAGE_CONVERSION_MISMATCHED);
    }
    if document.package_id.is_empty() || document.entry.is_empty() {
        return Err(marker_invalid("package journal"));
    }
    Ok(Some(document))
}

/// Write the journal for a run that has done nothing yet.
pub fn initialize(work_root: &Path, plan: &RunPlan) -> ToolResult<Journal> {
    let now = timestamp();
    let mut steps = BTreeMap::new();
    for step in STEPS {
        steps.insert(
            step.to_string(),
            StepEntry {
                status: StepStatus::Pending,
                attempt: 0,
                pending_reason: None,
            },
        );
    }
    let document = Journal {
        schema_version: PACKAGE_JOURNAL_SCHEMA.to_string(),
        status: "inProgress".to_string(),
        started_at: now.clone(),
        updated_at: now,
        completed_at: None,
        package_id: plan.package_id.clone(),
        package_version: plan.package_version.clone(),
        entry: plan.entry.clone(),
        source_format: plan.source_format.clone(),
        target_format: plan.target_format.clone(),
        source_digest: plan.source_digest.clone(),
        record_digest: plan.record_digest.clone(),
        index_verified: plan.index_verified,
        staged_source_digest: None,
        target_digest: None,
        target_bytes: None,
        converted_records: None,
        attempts: 0,
        steps,
    };
    markers::write_json(&journal_path(work_root), &document)?;
    Ok(document)
}

/// Record that a step is about to be attempted.
pub fn mark_running(work_root: &Path, step: &str) -> ToolResult<Journal> {
    mutate(work_root, |document| {
        let entry = document
            .steps
            .get_mut(step)
            .ok_or_else(|| marker_invalid("package journal"))?;
        entry.status = StepStatus::Running;
        entry.attempt += 1;
        entry.pending_reason = None;
        if step == RUN_CONVERTER {
            document.attempts += 1;
        }
        Ok(())
    })
}

/// Record that a step settled, with the fact that settled it.
pub fn mark_committed(work_root: &Path, step: &str, fact: StepFact) -> ToolResult<Journal> {
    mutate(work_root, |document| {
        let entry = document
            .steps
            .get_mut(step)
            .ok_or_else(|| marker_invalid("package journal"))?;
        entry.status = StepStatus::Committed;
        entry.pending_reason = None;
        match fact {
            StepFact::StagedSource { digest } => document.staged_source_digest = Some(digest),
            StepFact::Target {
                digest,
                bytes,
                converted_records,
            } => {
                document.target_digest = Some(digest);
                document.target_bytes = Some(bytes);
                document.converted_records = converted_records;
            }
            StepFact::None => {}
        }
        Ok(())
    })
}

/// Record that a step is owed again, and why.
pub fn mark_pending(work_root: &Path, step: &str, reason: &str) -> ToolResult<Journal> {
    mutate(work_root, |document| {
        let entry = document
            .steps
            .get_mut(step)
            .ok_or_else(|| marker_invalid("package journal"))?;
        entry.status = StepStatus::Pending;
        entry.pending_reason = Some(reason.to_string());
        Ok(())
    })
}

/// Close the journal: every step settled, the run over.
pub fn finish(work_root: &Path) -> ToolResult<Journal> {
    mutate(work_root, |document| {
        if !STEPS.iter().all(|step| document.settled(step)) {
            return Err(COMMIT_UNSUPPORTED);
        }
        document.status = "complete".to_string();
        document.completed_at = Some(timestamp());
        Ok(())
    })
}

/// The fact a settled step contributes to the record.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum StepFact {
    StagedSource {
        digest: String,
    },
    Target {
        digest: String,
        bytes: u64,
        converted_records: Option<u64>,
    },
    None,
}

fn mutate(
    work_root: &Path,
    edit: impl FnOnce(&mut Journal) -> ToolResult<()>,
) -> ToolResult<Journal> {
    let path = journal_path(work_root);
    let Some(mut document) = open(work_root)? else {
        return Err(marker_invalid("package journal"));
    };
    edit(&mut document)?;
    document.updated_at = timestamp();
    markers::write_json(&path, &document)?;
    Ok(document)
}

/// One instant, rendered so two entries written in the same run still order.
fn timestamp() -> String {
    match std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH) {
        Ok(elapsed) => format!("{}.{:09}", elapsed.as_secs(), elapsed.subsec_nanos()),
        Err(_) => "0.000000000".to_string(),
    }
}

/// The marker directory this journal belongs to, for a caller that reports it.
#[cfg(test)]
mod tests {
    use super::*;
    use crate::journal::test_support::scratch;

    fn plan() -> RunPlan {
        RunPlan {
            package_id: "org.licoland.fixture.converter".to_string(),
            package_version: "1.4.0".to_string(),
            entry: "bin/converter".to_string(),
            source_format: "licoup-state-0.1.1".to_string(),
            target_format: "licoup-state-0.3.0".to_string(),
            source_digest: "sha256:source".to_string(),
            record_digest: "sha256:record".to_string(),
            index_verified: false,
        }
    }

    #[test]
    fn a_run_is_recorded_step_by_step_and_matches_only_its_own_declaration() {
        let root = scratch("package-journal");
        let journal = initialize(&root, &plan()).expect("initialize");
        assert!(!journal.is_complete());
        assert_eq!(journal.outstanding_steps(), STEPS.to_vec());
        assert!(!journal.converter_attempted());

        let journal = mark_running(&root, STAGE_SOURCE).expect("running");
        assert_eq!(journal.step(STAGE_SOURCE).expect("entry").attempt, 1);
        // A step interrupted while running is unsettled, and reopening the document
        // reports exactly that rather than a commit.
        assert!(
            !open(&root)
                .expect("open")
                .expect("present")
                .settled(STAGE_SOURCE)
        );

        let journal = mark_committed(
            &root,
            STAGE_SOURCE,
            StepFact::StagedSource {
                digest: "sha256:staged".to_string(),
            },
        )
        .expect("committed");
        assert_eq!(
            journal.staged_source_digest.as_deref(),
            Some("sha256:staged")
        );
        assert!(journal.settled(STAGE_SOURCE));

        mark_running(&root, RUN_CONVERTER).expect("converter running");
        let journal = open(&root).expect("open").expect("present");
        assert!(journal.converter_attempted());
        assert_eq!(journal.attempts, 1);
        assert!(!journal.settled(RUN_CONVERTER));

        mark_committed(
            &root,
            RUN_CONVERTER,
            StepFact::Target {
                digest: "sha256:target".to_string(),
                bytes: 3,
                converted_records: Some(2),
            },
        )
        .expect("converter committed");
        let finished = finish(&root).expect("finish");
        assert!(finished.is_complete());
        assert_eq!(finished.target_bytes, Some(3));
        assert_eq!(finished.converted_records, Some(2));

        // The same document describes one run: another package, pair or source is not it.
        assert!(finished.matches(&plan()));
        let mut other = plan();
        other.package_id = "org.licoland.fixture.other".to_string();
        assert!(!finished.matches(&other));
        other = plan();
        other.source_digest = "sha256:changed".to_string();
        assert!(!finished.matches(&other));
    }

    #[test]
    fn a_journal_that_is_not_this_schema_is_refused() {
        let root = scratch("package-journal-schema");
        initialize(&root, &plan()).expect("initialize");
        let path = journal_path(&root);
        let mut document: serde_json::Value =
            markers::read_json(&path).expect("read").expect("present");
        document["schemaVersion"] = serde_json::json!("v9");
        markers::write_json(&path, &document).expect("write");
        assert_eq!(
            open(&root).expect_err("another schema").code(),
            "package_conversion_mismatched"
        );
    }
}
