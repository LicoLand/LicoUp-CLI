//! The file stage of a cleanup.
//!
//! The file stage is the only stage that removes application files. It runs
//! exactly once per approved cleanup, is resumable at every point, and reports
//! a partial result that later stages must complete.
//!
//! Restart safety comes from three rules:
//!
//! 1. Admission is closed before the first removal and re-established on every
//!    resume, so a restarted stage never races a writer it did not stop.
//! 2. Each entry's observed outcome is persisted before the next entry is
//!    touched, so a crash re-observes rather than re-derives.
//! 3. A recorded removal that is no longer true is a divergence: the stage
//!    records it pending instead of advancing, so progress is never claimed
//!    over a fact that stopped holding.
//!
//! The stage stops at [`CleanupStage::FilesSettled`]. It has no path to
//! credential deletion, terminal settlement or completion, and the receipt it
//! returns says so.

use anyhow::Result;

use super::file_owner::{CleanupFileOwner, FileStageOutcome, WriterQuiescence};
use super::journal::{
    CleanupJournalStore, CleanupStage, EntryOutcome, RecordedEntry, StageEntryClass,
};
use super::receipt::FileStageReceipt;
use super::target::{CleanupInventory, CleanupTarget};

/// Why one frozen entry is still pending.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PendingEntry {
    path: String,
    reason: String,
}

impl PendingEntry {
    pub fn path(&self) -> &str {
        &self.path
    }

    pub fn reason(&self) -> &str {
        &self.reason
    }
}

/// The truthful state of a file stage that has not settled every entry.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FileStageReport {
    target: CleanupTarget,
    inventory_digest: String,
    stage: CleanupStage,
    removed_count: usize,
    already_absent_count: usize,
    removed_file_bytes: u64,
    pending: Vec<PendingEntry>,
    journal_revision: u64,
}

impl FileStageReport {
    pub fn target(&self) -> &CleanupTarget {
        &self.target
    }

    pub fn inventory_digest(&self) -> &str {
        &self.inventory_digest
    }

    pub fn stage(&self) -> CleanupStage {
        self.stage
    }

    pub fn removed_count(&self) -> usize {
        self.removed_count
    }

    pub fn already_absent_count(&self) -> usize {
        self.already_absent_count
    }

    pub fn removed_file_bytes(&self) -> u64 {
        self.removed_file_bytes
    }

    pub fn pending(&self) -> &[PendingEntry] {
        &self.pending
    }

    pub fn journal_revision(&self) -> u64 {
        self.journal_revision
    }

    /// Always false: an unsettled file stage is never a finished cleanup.
    pub const fn complete(&self) -> bool {
        false
    }

    /// Stages still pending, including the entries this report holds back.
    pub fn outstanding_stages(&self) -> Vec<CleanupStage> {
        self.stage.outstanding()
    }
}

/// What one run of the file stage observed.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum FileStageProgress {
    /// Every frozen entry settled. The cleanup is still partial.
    Settled(Box<FileStageReceipt>),
    /// At least one entry is still pending. Nothing about it is reported as
    /// done, and the cleanup stays at its current stage.
    Pending(Box<FileStageReport>),
}

impl FileStageProgress {
    pub fn is_settled(&self) -> bool {
        matches!(self, Self::Settled(_))
    }

    pub fn receipt(&self) -> Option<&FileStageReceipt> {
        match self {
            Self::Settled(receipt) => Some(receipt),
            Self::Pending(_) => None,
        }
    }
}

/// Runs the file stage for one frozen inventory.
pub struct FileStage<'a> {
    owner: &'a dyn CleanupFileOwner,
    store: CleanupJournalStore,
}

impl<'a> FileStage<'a> {
    pub fn new(owner: &'a dyn CleanupFileOwner, store: CleanupJournalStore) -> Self {
        Self { owner, store }
    }

    pub fn owner_backend(&self) -> &'static str {
        self.owner.backend()
    }

    pub fn store(&self) -> &CleanupJournalStore {
        &self.store
    }

    /// Run, or resume, the file stage.
    ///
    /// Calling this twice for the same inventory is safe: the second call
    /// observes the settled entries and returns the same receipt shape without
    /// removing anything again.
    pub fn run(&self, inventory: &CleanupInventory) -> Result<FileStageProgress> {
        let mut journal = self.store.resume(inventory)?;
        if journal.stage() >= CleanupStage::FilesSettled {
            return Ok(FileStageProgress::Settled(Box::new(
                self.receipt_from_journal(&journal, inventory),
            )));
        }
        if journal.stage() == CleanupStage::Admitted {
            journal.advance(CleanupStage::WritersQuiesced)?;
            self.store.save(&mut journal)?;
        }

        // Admission is closed for the whole removal loop, including a resume,
        // so a writer admitted between two runs is refused instead of ignored.
        let _quiescence: WriterQuiescence = self.owner.quiesce_writers(inventory.target())?;

        for entry in inventory.entries() {
            let recorded = journal
                .entries(StageEntryClass::Files)
                .get(entry.path())
                .cloned();
            if let Some(recorded) = &recorded
                && recorded.outcome().is_settled()
            {
                // Confirmed progress, not assumed progress: a recorded removal
                // that is no longer true is a divergence this stage refuses to
                // advance over.
                if !self.owner.observe_absent(entry)? {
                    journal.entries_mut(StageEntryClass::Files).insert(
                        entry.path().to_string(),
                        RecordedEntry::pending(Some("cleanup_resume_diverged")),
                    );
                    self.store.save(&mut journal)?;
                }
                continue;
            }
            let record = match self.owner.remove_owned_entry(entry) {
                Ok(FileStageOutcome::Removed { .. }) => {
                    RecordedEntry::settled(EntryOutcome::Removed)
                }
                Ok(FileStageOutcome::AlreadyAbsent) => {
                    RecordedEntry::settled(EntryOutcome::AlreadyAbsent)
                }
                // One entry's refusal stops that entry, never the whole
                // cleanup: the others are independent and the completed
                // removals are not rolled back.
                Err(error) => RecordedEntry::pending(Some(&reason_code(&error.to_string()))),
            };
            journal
                .entries_mut(StageEntryClass::Files)
                .insert(entry.path().to_string(), record);
            self.store.save(&mut journal)?;
        }

        let report = self.report_from_journal(&journal, inventory);
        if report.pending.is_empty() {
            journal.advance(CleanupStage::FilesSettled)?;
            self.store.save(&mut journal)?;
            let report = self.report_from_journal(&journal, inventory);
            return Ok(FileStageProgress::Settled(Box::new(
                self.receipt_from_report(&report),
            )));
        }
        Ok(FileStageProgress::Pending(Box::new(report)))
    }

    fn report_from_journal(
        &self,
        journal: &super::journal::CleanupJournal,
        inventory: &CleanupInventory,
    ) -> FileStageReport {
        let mut removed_count = 0;
        let mut already_absent_count = 0;
        let mut removed_file_bytes = 0_u64;
        let mut pending = Vec::new();
        for entry in inventory.entries() {
            match journal.entries(StageEntryClass::Files).get(entry.path()) {
                Some(recorded) if recorded.outcome() == EntryOutcome::Removed => {
                    removed_count += 1;
                    removed_file_bytes = removed_file_bytes.saturating_add(entry.size());
                }
                Some(recorded) if recorded.outcome() == EntryOutcome::AlreadyAbsent => {
                    already_absent_count += 1;
                }
                Some(recorded) => pending.push(PendingEntry {
                    path: entry.path().to_string(),
                    reason: recorded
                        .reason()
                        .unwrap_or("cleanup_entry_pending")
                        .to_string(),
                }),
                None => pending.push(PendingEntry {
                    path: entry.path().to_string(),
                    reason: "cleanup_entry_unvisited".to_string(),
                }),
            }
        }
        FileStageReport {
            target: inventory.target().clone(),
            inventory_digest: inventory.digest().to_string(),
            stage: journal.stage(),
            removed_count,
            already_absent_count,
            removed_file_bytes,
            pending,
            journal_revision: journal.revision(),
        }
    }

    fn receipt_from_report(&self, report: &FileStageReport) -> FileStageReceipt {
        FileStageReceipt::new(
            &report.target,
            &report.inventory_digest,
            report.removed_count,
            report.already_absent_count,
            report.removed_file_bytes,
            report.journal_revision,
            report.outstanding_stages(),
        )
    }

    fn receipt_from_journal(
        &self,
        journal: &super::journal::CleanupJournal,
        inventory: &CleanupInventory,
    ) -> FileStageReceipt {
        self.receipt_from_report(&self.report_from_journal(journal, inventory))
    }
}

fn reason_code(error: &str) -> String {
    match error.split(':').next() {
        Some(code) if !code.trim().is_empty() => code.trim().to_string(),
        _ => "cleanup_entry_failed".to_string(),
    }
}
