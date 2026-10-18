//! Synthetic fixtures and the stage-restart proofs.
//!
//! Every fixture here is synthetic: an in-memory file owner, a recording
//! receipt path, and disposable roots under the platform temporary directory.
//! No test reads, writes or removes a real data root, a real credential or an
//! installed application.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use anyhow::{Result, anyhow};

use super::file_owner::{
    CleanupFileOwner, FileStageOutcome, PrivateDataRootFileOwner, WriterQuiescence,
};
use super::journal::{
    CLEANUP_JOURNAL_SCHEMA, CLEANUP_STATE_DIRECTORY, CleanupJournal, CleanupJournalStore,
    CleanupStage, EntryOutcome, StageEntryClass,
};
use super::receipt::{
    CleanupReceiptPath, FileStageReceipt, ReceiptDelivery, RestrictedReceiptEnvelope,
};
use super::stage::{FileStage, FileStageProgress};
use super::target::{
    CleanupInventory, CleanupInventoryEntry, CleanupSubject, CleanupTarget, DeviceId, OperationId,
};

const FIXTURE_ADMISSION_LOCK: &str = "client-state/migrations/admission.lock";

/// One disposable root under the temporary directory.
struct SyntheticRoot {
    path: PathBuf,
}

impl SyntheticRoot {
    fn new(label: &str) -> Self {
        let path = std::env::temp_dir().join(format!(
            "licoup-endpoint-cleanup-{label}-{}",
            uuid::Uuid::new_v4()
        ));
        fs::create_dir_all(&path).unwrap();
        Self { path }
    }

    fn path(&self) -> &Path {
        &self.path
    }

    fn write(&self, relative: &str, bytes: &[u8]) -> PathBuf {
        let path = self.path.join(relative);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, bytes).unwrap();
        path
    }
}

impl Drop for SyntheticRoot {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.path);
    }
}

/// An in-memory file owner with injectable refusals.
#[derive(Default)]
struct FixtureFileOwner {
    present: Mutex<BTreeMap<String, u64>>,
    refusing: Mutex<BTreeSet<String>>,
    removals: Mutex<Vec<String>>,
}

impl FixtureFileOwner {
    fn with_entries(entries: &[(&str, u64)]) -> Self {
        let owner = Self::default();
        {
            let mut present = owner.present.lock().unwrap();
            for (path, size) in entries {
                present.insert((*path).to_string(), *size);
            }
        }
        owner
    }

    fn refuse(&self, path: &str) {
        self.refusing.lock().unwrap().insert(path.to_string());
    }

    fn allow(&self, path: &str) {
        self.refusing.lock().unwrap().remove(path);
    }

    fn restore(&self, path: &str, size: u64) {
        self.present.lock().unwrap().insert(path.to_string(), size);
    }

    fn present_paths(&self) -> Vec<String> {
        self.present.lock().unwrap().keys().cloned().collect()
    }

    fn removal_attempts(&self) -> Vec<String> {
        self.removals.lock().unwrap().clone()
    }
}

impl CleanupFileOwner for FixtureFileOwner {
    fn backend(&self) -> &'static str {
        "fixture-in-memory-files"
    }

    fn quiesce_writers(&self, _target: &CleanupTarget) -> Result<WriterQuiescence> {
        Ok(WriterQuiescence::fixture("fixture-admission"))
    }

    fn remove_owned_entry(&self, entry: &CleanupInventoryEntry) -> Result<FileStageOutcome> {
        self.removals.lock().unwrap().push(entry.path().to_string());
        if self.refusing.lock().unwrap().contains(entry.path()) {
            return Err(anyhow!("cleanup_entry_removal_failed"));
        }
        match self.present.lock().unwrap().remove(entry.path()) {
            Some(bytes) => Ok(FileStageOutcome::Removed { bytes }),
            None => Ok(FileStageOutcome::AlreadyAbsent),
        }
    }

    fn observe_absent(&self, entry: &CleanupInventoryEntry) -> Result<bool> {
        Ok(!self.present.lock().unwrap().contains_key(entry.path()))
    }
}

/// A restricted receipt path that records instead of transmitting.
#[derive(Default)]
struct RecordingReceiptPath {
    delivered: Mutex<Vec<RestrictedReceiptEnvelope>>,
    unavailable: bool,
}

impl RecordingReceiptPath {
    fn unavailable() -> Self {
        Self {
            delivered: Mutex::new(Vec::new()),
            unavailable: true,
        }
    }

    fn envelopes(&self) -> Vec<RestrictedReceiptEnvelope> {
        self.delivered.lock().unwrap().clone()
    }
}

impl CleanupReceiptPath for RecordingReceiptPath {
    fn backend(&self) -> &'static str {
        "fixture-restricted-control-path"
    }

    fn deliver(&self, envelope: &RestrictedReceiptEnvelope) -> Result<ReceiptDelivery> {
        if self.unavailable {
            return Ok(ReceiptDelivery::unavailable("fixture_path_unavailable"));
        }
        self.delivered.lock().unwrap().push(envelope.clone());
        Ok(ReceiptDelivery::Delivered)
    }
}

struct Fixture {
    root: SyntheticRoot,
    store: CleanupJournalStore,
    inventory: CleanupInventory,
}

impl Fixture {
    fn new(label: &str, entries: &[(&str, u64)]) -> Self {
        let root = SyntheticRoot::new(label);
        let store = CleanupJournalStore::new(root.path());
        let target = CleanupTarget::new(
            CleanupSubject::new("subject-fixture").unwrap(),
            DeviceId::new("device-fixture").unwrap(),
            OperationId::generate(),
        );
        let inventory = CleanupInventory::freeze(
            target,
            entries
                .iter()
                .map(|(path, size)| CleanupInventoryEntry::file(*path, *size).unwrap()),
        )
        .unwrap();
        Self {
            root,
            store,
            inventory,
        }
    }

    fn stage<'a>(&'a self, owner: &'a FixtureFileOwner) -> FileStage<'a> {
        FileStage::new(owner, self.store.clone())
    }
}

#[test]
fn the_file_stage_settles_every_frozen_entry_and_leaves_unlisted_data_alone() {
    let fixture = Fixture::new("settles", &[("a.txt", 3), ("nested/b.bin", 4)]);
    let owner = FixtureFileOwner::with_entries(&[
        ("a.txt", 3),
        ("nested/b.bin", 4),
        ("external/keep.txt", 9),
    ]);

    let progress = fixture.stage(&owner).run(&fixture.inventory).unwrap();
    let receipt = progress.receipt().expect("the file stage settled");

    assert_eq!(receipt.removed_count(), 2);
    assert_eq!(receipt.already_absent_count(), 0);
    assert_eq!(receipt.removed_file_bytes(), 7);
    assert_eq!(owner.present_paths(), vec!["external/keep.txt".to_string()]);
    // The root holds nothing but this owner's own progress material: the stage
    // never walks the root, so nothing unlisted was reached.
    let mut left: Vec<String> = fs::read_dir(fixture.root.path())
        .unwrap()
        .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    left.sort();
    assert_eq!(left, vec![CLEANUP_STATE_DIRECTORY.to_string()]);
}

#[test]
fn a_restart_at_each_meaningful_boundary_resumes_without_removing_twice() {
    let fixture = Fixture::new("restart", &[("a.txt", 3), ("nested/b.bin", 4)]);
    let owner = FixtureFileOwner::with_entries(&[("a.txt", 3), ("nested/b.bin", 4)]);
    owner.refuse("nested/b.bin");

    let first = fixture.stage(&owner).run(&fixture.inventory).unwrap();
    match &first {
        FileStageProgress::Pending(report) => {
            assert_eq!(report.stage(), CleanupStage::WritersQuiesced);
            assert_eq!(report.pending().len(), 1);
            assert_eq!(report.pending()[0].path(), "nested/b.bin");
            assert_eq!(report.pending()[0].reason(), "cleanup_entry_removal_failed");
            assert!(!report.complete());
        }
        FileStageProgress::Settled(_) => panic!("a refused entry must not settle the stage"),
    }
    assert_eq!(owner.present_paths(), vec!["nested/b.bin".to_string()]);

    // Restart: the recorded removal is confirmed by observation, not repeated.
    let persisted = fixture.store.load().unwrap().unwrap();
    assert_eq!(persisted.stage(), CleanupStage::WritersQuiesced);
    assert_eq!(
        persisted.entries(StageEntryClass::Files)["a.txt"].outcome(),
        EntryOutcome::Removed
    );

    owner.allow("nested/b.bin");
    let attempts_before = owner.removal_attempts().len();
    let second = fixture.stage(&owner).run(&fixture.inventory).unwrap();
    let receipt = second.receipt().expect("the resumed stage settles");
    // The receipt describes the whole file stage across restarts, so it reports
    // both removals: the one the first run recorded and the one this run made.
    assert_eq!(receipt.removed_count(), 2);
    assert_eq!(receipt.already_absent_count(), 0);
    assert_eq!(owner.removal_attempts().len(), attempts_before + 1);
    assert!(owner.present_paths().is_empty());

    // A third run is idempotent and reports the same shape.
    let third = fixture.stage(&owner).run(&fixture.inventory).unwrap();
    let receipt = third.receipt().unwrap();
    assert_eq!(receipt.removed_count(), 2);
    assert_eq!(receipt.already_absent_count(), 0);
    assert_eq!(owner.removal_attempts().len(), attempts_before + 1);
}

#[test]
fn a_file_stage_receipt_stays_partial_and_cannot_report_completion() {
    let fixture = Fixture::new("partial", &[("a.txt", 3)]);
    let owner = FixtureFileOwner::with_entries(&[("a.txt", 3)]);
    let path = RecordingReceiptPath::default();

    let progress = fixture.stage(&owner).run(&fixture.inventory).unwrap();
    let receipt = progress.receipt().unwrap();

    assert!(!receipt.complete());
    assert!(!receipt.admission_restored());
    assert_eq!(
        receipt.outstanding_stages(),
        &[
            CleanupStage::CredentialsSettled,
            CleanupStage::TerminalSettlement,
            CleanupStage::Complete
        ]
    );

    let delivery = receipt.deliver(&path).unwrap();
    assert!(delivery.is_delivered());
    let envelopes = path.envelopes();
    assert_eq!(envelopes.len(), 1);
    assert!(!envelopes[0].complete());
    assert!(!envelopes[0].admission_restored());
    assert_eq!(envelopes[0].kind().as_str(), "file-stage");

    // The journal stopped at the file stage and can only move one declared step.
    let persisted = fixture.store.load().unwrap().unwrap();
    assert_eq!(persisted.stage(), CleanupStage::FilesSettled);
    let mut attempt = persisted.clone();
    assert!(
        attempt
            .advance(CleanupStage::Complete)
            .unwrap_err()
            .to_string()
            .contains("transition_refused")
    );
    assert_eq!(
        fixture.store.load().unwrap().unwrap().stage(),
        CleanupStage::FilesSettled
    );
}

#[test]
fn a_recorded_removal_that_no_longer_holds_stops_the_stage_instead_of_advancing() {
    let fixture = Fixture::new("diverged", &[("a.txt", 3), ("b.bin", 5)]);
    let owner = FixtureFileOwner::with_entries(&[("a.txt", 3), ("b.bin", 5)]);
    owner.refuse("b.bin");
    assert!(matches!(
        fixture.stage(&owner).run(&fixture.inventory).unwrap(),
        FileStageProgress::Pending(_)
    ));

    // Something recreated a file the journal recorded as removed.
    owner.restore("a.txt", 3);
    owner.allow("b.bin");

    let progress = fixture.stage(&owner).run(&fixture.inventory).unwrap();
    let report = match progress {
        FileStageProgress::Pending(report) => report,
        FileStageProgress::Settled(_) => {
            panic!("a diverged entry must keep the stage pending")
        }
    };
    assert_eq!(report.stage(), CleanupStage::WritersQuiesced);
    assert_eq!(report.pending().len(), 1);
    assert_eq!(report.pending()[0].path(), "a.txt");
    assert_eq!(report.pending()[0].reason(), "cleanup_resume_diverged");
}

#[test]
fn an_unavailable_restricted_path_is_reported_and_never_read_as_success() {
    let fixture = Fixture::new("unavailable", &[("a.txt", 3)]);
    let owner = FixtureFileOwner::with_entries(&[("a.txt", 3)]);
    let receipt = fixture
        .stage(&owner)
        .run(&fixture.inventory)
        .unwrap()
        .receipt()
        .cloned()
        .unwrap();

    let delivery = receipt
        .deliver(&RecordingReceiptPath::unavailable())
        .unwrap();
    assert!(!delivery.is_delivered());
    match delivery {
        ReceiptDelivery::Unavailable { code } => assert_eq!(code, "fixture_path_unavailable"),
        ReceiptDelivery::Delivered => unreachable!(),
    }
}

#[test]
fn the_journal_refuses_a_stale_revision_and_a_rewind() {
    let fixture = Fixture::new("revision", &[("a.txt", 3)]);
    let mut journal = fixture.store.resume(&fixture.inventory).unwrap();
    assert_eq!(journal.revision(), 0);
    fixture.store.save(&mut journal).unwrap();
    assert_eq!(journal.revision(), 1);

    let stale = journal.clone();
    journal.advance(CleanupStage::WritersQuiesced).unwrap();
    fixture.store.save(&mut journal).unwrap();
    assert_eq!(journal.revision(), 2);

    let mut replayed = stale;
    replayed.advance(CleanupStage::WritersQuiesced).unwrap();
    assert!(
        fixture
            .store
            .save(&mut replayed)
            .unwrap_err()
            .to_string()
            .contains("cleanup_journal_revision_stale")
    );

    let mut rewound = fixture.store.load().unwrap().unwrap();
    assert!(
        rewound
            .advance(CleanupStage::Admitted)
            .unwrap_err()
            .to_string()
            .contains("transition_refused")
    );
}

#[test]
fn the_journal_refuses_an_inventory_that_names_another_target_or_set() {
    let fixture = Fixture::new("mismatch", &[("a.txt", 3)]);
    let mut journal = fixture.store.resume(&fixture.inventory).unwrap();
    fixture.store.save(&mut journal).unwrap();

    let other = CleanupInventory::freeze(
        CleanupTarget::new(
            CleanupSubject::new("subject-other").unwrap(),
            DeviceId::new("device-fixture").unwrap(),
            OperationId::generate(),
        ),
        [CleanupInventoryEntry::file("a.txt", 3).unwrap()],
    )
    .unwrap();
    assert!(
        fixture
            .store
            .resume(&other)
            .unwrap_err()
            .to_string()
            .contains("cleanup_journal_target_mismatch")
    );
}

#[test]
fn the_private_data_root_owner_removes_only_frozen_entries_and_refuses_a_link() {
    let root = SyntheticRoot::new("root-owner");
    root.write("a.txt", b"aaa");
    root.write("nested/b.bin", b"bbbb");
    root.write("external/keep.txt", b"kept");
    let outside = std::env::temp_dir().join(format!("licoup-outside-{}", uuid::Uuid::new_v4()));
    fs::write(&outside, b"outside").unwrap();
    #[cfg(unix)]
    std::os::unix::fs::symlink(&outside, root.path().join("linked.txt")).unwrap();

    let store = CleanupJournalStore::new(root.path());
    let target = CleanupTarget::new(
        CleanupSubject::new("subject-fixture").unwrap(),
        DeviceId::new("device-fixture").unwrap(),
        OperationId::generate(),
    );
    let mut entries = vec![
        CleanupInventoryEntry::file("a.txt", 3).unwrap(),
        CleanupInventoryEntry::file("nested/b.bin", 4).unwrap(),
    ];
    #[cfg(unix)]
    entries.push(CleanupInventoryEntry::file("linked.txt", 7).unwrap());
    let inventory = CleanupInventory::freeze(target, entries).unwrap();

    let owner = PrivateDataRootFileOwner::new(root.path());
    let progress = FileStage::new(&owner, store.clone())
        .run(&inventory)
        .unwrap();
    // On a platform that can express the case, the refused link keeps its own
    // entry pending so the stage does not settle — but the entries it could
    // remove are gone and are never restored.
    #[cfg(unix)]
    {
        let report = match &progress {
            FileStageProgress::Pending(report) => report,
            FileStageProgress::Settled(_) => panic!("a refused link must keep the stage pending"),
        };
        assert_eq!(report.pending().len(), 1);
        assert_eq!(report.pending()[0].path(), "linked.txt");
        assert_eq!(
            report.stage(),
            CleanupStage::WritersQuiesced,
            "the stage stops before FilesSettled"
        );
    }
    #[cfg(not(unix))]
    {
        assert!(progress.is_settled());
    }

    assert!(!root.path().join("a.txt").exists());
    assert!(!root.path().join("nested/b.bin").exists());
    assert!(root.path().join("external/keep.txt").exists());
    // A link is refused by the owner, so the file it points at survives.
    #[cfg(unix)]
    {
        assert!(root.path().join("linked.txt").exists());
        assert!(outside.exists());
        let persisted = store.load().unwrap().unwrap();
        assert_eq!(
            persisted.entries(StageEntryClass::Files)["linked.txt"]
                .reason()
                .unwrap(),
            "cleanup_entry_symlink_refused"
        );
        let _ = fs::remove_file(&outside);
    }

    // Terminal material settlement removes the progress it wrote and the root
    // admission lock, and reports terminal only then.
    assert!(root.path().join(CLEANUP_STATE_DIRECTORY).exists());
    let settlement = owner
        .settle_cleanup_material(&store.state_directory())
        .unwrap();
    assert!(settlement.is_terminal());
    assert!(!store.state_directory().exists());
    assert!(store.load().unwrap().is_none());
    assert!(settlement.removed_file_count >= 1);
}

#[test]
fn the_private_data_root_owner_refuses_a_live_writer() {
    use fs2::FileExt;

    let root = SyntheticRoot::new("live-writer");
    root.write("a.txt", b"aaa");
    let admission = root.write(FIXTURE_ADMISSION_LOCK, b"");
    let held = fs::OpenOptions::new().read(true).open(&admission).unwrap();
    held.lock_shared().unwrap();

    let owner = PrivateDataRootFileOwner::new(root.path());
    let target = CleanupTarget::new(
        CleanupSubject::new("subject-fixture").unwrap(),
        DeviceId::new("device-fixture").unwrap(),
        OperationId::generate(),
    );
    assert!(
        owner
            .quiesce_writers(&target)
            .unwrap_err()
            .to_string()
            .contains("cleanup_writers_running")
    );

    FileExt::unlock(&held).unwrap();
    let quiescence = owner.quiesce_writers(&target).unwrap();
    assert!(quiescence.holds_exclusion());
    assert_eq!(quiescence.backend(), "data-root-admission-exclusive");
}

#[test]
fn a_frozen_inventory_is_the_only_thing_the_stage_can_reach() {
    let fixture = Fixture::new("reach", &[("a.txt", 3)]);
    let owner = FixtureFileOwner::with_entries(&[("a.txt", 3), ("secret.txt", 1)]);
    assert!(matches!(
        fixture.stage(&owner).run(&fixture.inventory).unwrap(),
        FileStageProgress::Settled(_)
    ));
    assert_eq!(owner.present_paths(), vec!["secret.txt".to_string()]);
    assert_eq!(owner.removal_attempts(), vec!["a.txt".to_string()]);
}

#[test]
fn the_journal_document_is_the_declared_schema() {
    let fixture = Fixture::new("schema", &[("a.txt", 3)]);
    let journal = CleanupJournal::admitted(&fixture.inventory);
    assert_eq!(journal.schema(), CLEANUP_JOURNAL_SCHEMA);
    assert_eq!(journal.stage(), CleanupStage::Admitted);
    assert_eq!(journal.entries(StageEntryClass::Credentials).len(), 0);
}
