//! Durable, replay-safe cleanup progress.
//!
//! The journal is the only thing that makes a cleanup restartable. It is
//! written beside the root's own state, atomically, with a monotonically
//! increasing revision: a writer that presents a revision this owner has
//! already passed is refused instead of rewinding progress. Every entry is
//! recorded by the exact name the frozen inventory used, so a resumed stage
//! continues from observed facts rather than from a caller's summary.

use anyhow::{Result, ensure};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use licoup_foundation::platform::file_security::{
    atomic_write_private_text_bounded, ensure_private_dir, read_private_text_bounded,
};

use super::target::{CleanupInventory, CleanupTarget};

/// The one layout tag a cleanup journal declares.
pub const CLEANUP_JOURNAL_SCHEMA: &str = "licoup.endpoint-cleanup-journal.v1";
/// Data-root-relative directory this owner keeps its progress and logs in.
pub const CLEANUP_STATE_DIRECTORY: &str = "cleanup";
/// Journal file name inside the state directory.
pub const CLEANUP_JOURNAL_FILE: &str = "journal.json";
/// Largest journal this owner reads back.
pub const MAX_CLEANUP_JOURNAL_BYTES: usize = 1024 * 1024;
/// Longest recorded refusal reason.
pub const MAX_CLEANUP_REASON_BYTES: usize = 96;

/// How far a cleanup has progressed.
///
/// The order is the cleanup's own: no stage may be skipped and none may be
/// re-entered. A file stage stops at [`CleanupStage::FilesSettled`]; credential
/// deletion and terminal settlement are later stages that decide their own
/// preconditions.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum CleanupStage {
    /// The cleanup was admitted and the inventory frozen. Nothing removed yet.
    Admitted,
    /// Admission is closed and no application writer holds the root.
    WritersQuiesced,
    /// Every frozen file entry is settled.
    FilesSettled,
    /// Every frozen credential entry is settled.
    CredentialsSettled,
    /// Journals, logs, temporary material and the admission lock are settled.
    TerminalSettlement,
    /// Every required stage was observed settled and the final receipt was
    /// delivered. Only the final stage may report this.
    Complete,
}

impl CleanupStage {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Admitted => "admitted",
            Self::WritersQuiesced => "writers-quiesced",
            Self::FilesSettled => "files-settled",
            Self::CredentialsSettled => "credentials-settled",
            Self::TerminalSettlement => "terminal-settlement",
            Self::Complete => "complete",
        }
    }

    const fn next(self) -> Option<Self> {
        match self {
            Self::Admitted => Some(Self::WritersQuiesced),
            Self::WritersQuiesced => Some(Self::FilesSettled),
            Self::FilesSettled => Some(Self::CredentialsSettled),
            Self::CredentialsSettled => Some(Self::TerminalSettlement),
            Self::TerminalSettlement => Some(Self::Complete),
            Self::Complete => None,
        }
    }

    /// Every stage from `self` (exclusive) to completion, in order.
    pub fn outstanding(self) -> Vec<Self> {
        let mut stages = Vec::new();
        let mut current = self;
        while let Some(next) = current.next() {
            stages.push(next);
            current = next;
        }
        stages
    }

    /// Whether this stage already means every effect is done.
    pub const fn is_complete(self) -> bool {
        matches!(self, Self::Complete)
    }
}

/// What one frozen entry's own stage observed.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum EntryOutcome {
    /// Not settled. The entry is untouched or the effect failed.
    Pending,
    /// The entry was present and is now gone.
    Removed,
    /// The entry was already gone when this stage looked.
    AlreadyAbsent,
}

impl EntryOutcome {
    pub const fn is_settled(self) -> bool {
        !matches!(self, Self::Pending)
    }
}

/// One recorded entry.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct RecordedEntry {
    outcome: EntryOutcome,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    reason: Option<String>,
}

impl RecordedEntry {
    pub fn pending(reason: Option<&str>) -> Self {
        Self {
            outcome: EntryOutcome::Pending,
            reason: reason.map(|reason| bounded_reason(reason)),
        }
    }

    pub fn settled(outcome: EntryOutcome) -> Self {
        Self {
            outcome,
            reason: None,
        }
    }

    pub fn outcome(&self) -> EntryOutcome {
        self.outcome
    }

    pub fn reason(&self) -> Option<&str> {
        self.reason.as_deref()
    }
}

/// The durable progress document for one approved cleanup.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CleanupJournal {
    schema: String,
    subject: String,
    device: String,
    operation: String,
    inventory_digest: String,
    revision: u64,
    stage: CleanupStage,
    #[serde(default)]
    files: BTreeMap<String, RecordedEntry>,
    #[serde(default)]
    credentials: BTreeMap<String, RecordedEntry>,
}

impl CleanupJournal {
    /// A cleanup that was admitted and has removed nothing yet.
    pub fn admitted(inventory: &CleanupInventory) -> Self {
        let target = inventory.target();
        Self {
            schema: CLEANUP_JOURNAL_SCHEMA.to_string(),
            subject: target.subject().as_str().to_string(),
            device: target.device().as_str().to_string(),
            operation: target.operation().as_str().to_string(),
            inventory_digest: inventory.digest().to_string(),
            revision: 0,
            stage: CleanupStage::Admitted,
            files: BTreeMap::new(),
            credentials: BTreeMap::new(),
        }
    }

    pub fn schema(&self) -> &str {
        &self.schema
    }

    pub fn stage(&self) -> CleanupStage {
        self.stage
    }

    /// Monotonic revision. `0` means never persisted.
    pub fn revision(&self) -> u64 {
        self.revision
    }

    pub fn inventory_digest(&self) -> &str {
        &self.inventory_digest
    }

    pub fn binding(&self) -> String {
        format!("{}|{}|{}", self.subject, self.device, self.operation)
    }

    pub fn files(&self) -> &BTreeMap<String, RecordedEntry> {
        &self.files
    }

    pub fn credentials(&self) -> &BTreeMap<String, RecordedEntry> {
        &self.credentials
    }

    /// The two entry maps a stage writes into.
    pub fn entries_mut(&mut self, class: StageEntryClass) -> &mut BTreeMap<String, RecordedEntry> {
        match class {
            StageEntryClass::Files => &mut self.files,
            StageEntryClass::Credentials => &mut self.credentials,
        }
    }

    pub fn entries(&self, class: StageEntryClass) -> &BTreeMap<String, RecordedEntry> {
        match class {
            StageEntryClass::Files => &self.files,
            StageEntryClass::Credentials => &self.credentials,
        }
    }

    /// Whether this journal describes exactly this target and this inventory.
    pub fn require_inventory(&self, inventory: &CleanupInventory) -> Result<()> {
        ensure!(
            self.schema == CLEANUP_JOURNAL_SCHEMA,
            "cleanup_journal_schema_unsupported"
        );
        ensure!(
            self.binding() == inventory.target().binding()
                && self.inventory_digest == inventory.digest(),
            "cleanup_journal_target_mismatch"
        );
        Ok(())
    }

    /// Advance exactly one stage. Skipping, rewinding and re-entering are all
    /// refused, so a resumed stage can never leap to completion.
    pub fn advance(&mut self, stage: CleanupStage) -> Result<()> {
        ensure!(
            self.stage.next() == Some(stage),
            "cleanup_stage_transition_refused"
        );
        self.stage = stage;
        Ok(())
    }

    /// The target this journal is bound to.
    pub fn target_bound_to(&self, target: &CleanupTarget) -> bool {
        self.binding() == target.binding()
    }
}

/// Which frozen class a stage records into.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StageEntryClass {
    Files,
    Credentials,
}

fn bounded_reason(reason: &str) -> String {
    let mut bounded: String = reason
        .chars()
        .filter(|character| character.is_ascii_alphanumeric() || matches!(character, '_' | '-'))
        .take(MAX_CLEANUP_REASON_BYTES)
        .collect();
    if bounded.is_empty() {
        bounded.push_str("cleanup_stage_failed");
    }
    bounded
}

/// The durable store for one data root's cleanup journal.
#[derive(Clone, Debug)]
pub struct CleanupJournalStore {
    root: PathBuf,
}

impl CleanupJournalStore {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn state_directory(&self) -> PathBuf {
        self.root.join(CLEANUP_STATE_DIRECTORY)
    }

    pub fn journal_path(&self) -> PathBuf {
        self.state_directory().join(CLEANUP_JOURNAL_FILE)
    }

    /// Read back the persisted journal, if this root has one.
    ///
    /// A journal that does not parse, declares another schema, or names another
    /// target is refused rather than replaced: silently starting a second
    /// cleanup over a first one's progress is exactly the mistake this file
    /// exists to prevent.
    pub fn load(&self) -> Result<Option<CleanupJournal>> {
        let Some(text) =
            read_private_text_bounded(&self.journal_path(), MAX_CLEANUP_JOURNAL_BYTES)?
        else {
            return Ok(None);
        };
        let journal: CleanupJournal =
            serde_json::from_str(&text).map_err(|_| anyhow::anyhow!("cleanup_journal_invalid"))?;
        ensure!(
            journal.schema == CLEANUP_JOURNAL_SCHEMA,
            "cleanup_journal_schema_unsupported"
        );
        Ok(Some(journal))
    }

    /// Resume this root's cleanup for exactly this inventory.
    pub fn resume(&self, inventory: &CleanupInventory) -> Result<CleanupJournal> {
        match self.load()? {
            Some(journal) => {
                journal.require_inventory(inventory)?;
                Ok(journal)
            }
            None => Ok(CleanupJournal::admitted(inventory)),
        }
    }

    /// Persist the next revision of this journal.
    ///
    /// The revision must be exactly one past what is on disk. A stale writer —
    /// a resumed process that read an older revision, or a replayed request —
    /// is refused, so progress cannot move backwards.
    pub fn save(&self, journal: &mut CleanupJournal) -> Result<()> {
        ensure!(
            journal.schema == CLEANUP_JOURNAL_SCHEMA,
            "cleanup_journal_schema_unsupported"
        );
        let persisted = self.load()?;
        let expected = persisted
            .as_ref()
            .map(|persisted| persisted.revision)
            .unwrap_or(0);
        ensure!(
            journal.revision == expected,
            "cleanup_journal_revision_stale"
        );
        if let Some(persisted) = &persisted {
            ensure!(
                persisted.binding() == journal.binding()
                    && persisted.inventory_digest == journal.inventory_digest,
                "cleanup_journal_target_mismatch"
            );
            ensure!(
                persisted.stage <= journal.stage,
                "cleanup_journal_stage_regression"
            );
        }
        journal.revision = expected + 1;
        ensure_private_dir(&self.state_directory())?;
        let text = serde_json::to_string(journal)
            .map_err(|_| anyhow::anyhow!("cleanup_journal_unencodable"))?;
        atomic_write_private_text_bounded(&self.journal_path(), &text, MAX_CLEANUP_JOURNAL_BYTES)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_stage_may_only_advance_one_step_and_never_rewind() {
        let mut journal = CleanupJournal {
            schema: CLEANUP_JOURNAL_SCHEMA.to_string(),
            subject: "s".into(),
            device: "d".into(),
            operation: "o".into(),
            inventory_digest: "digest".into(),
            revision: 0,
            stage: CleanupStage::Admitted,
            files: BTreeMap::new(),
            credentials: BTreeMap::new(),
        };
        assert!(
            journal
                .advance(CleanupStage::FilesSettled)
                .unwrap_err()
                .to_string()
                .contains("transition_refused")
        );
        journal.advance(CleanupStage::WritersQuiesced).unwrap();
        journal.advance(CleanupStage::FilesSettled).unwrap();
        assert!(
            journal
                .advance(CleanupStage::FilesSettled)
                .unwrap_err()
                .to_string()
                .contains("transition_refused")
        );
    }

    #[test]
    fn outstanding_stages_name_everything_left_after_the_file_stage() {
        assert_eq!(
            CleanupStage::FilesSettled.outstanding(),
            vec![
                CleanupStage::CredentialsSettled,
                CleanupStage::TerminalSettlement,
                CleanupStage::Complete
            ]
        );
        assert!(!CleanupStage::FilesSettled.is_complete());
        assert!(CleanupStage::Complete.outstanding().is_empty());
    }

    #[test]
    fn a_reason_is_bounded_to_a_code() {
        let recorded = RecordedEntry::pending(Some(&"x".repeat(4_000)));
        assert_eq!(recorded.reason().unwrap().len(), MAX_CLEANUP_REASON_BYTES);
        let recorded = RecordedEntry::pending(Some("!!"));
        assert_eq!(recorded.reason(), Some("cleanup_stage_failed"));
    }
}
