//! The durable catalogue record: generation references and the active pointer.
//!
//! C09 §6 requires the generation reference and the active pointer to survive a
//! restart. This module is the seam the host uses for that, and it owns a record
//! beside the install journal inside the *same* managed root — not a second
//! ledger for the same facts. The install journal keeps owning the package
//! bytes and the installed versions; this record keeps only what the runtime
//! host alone knows: which generation of which package was consumed, and which
//! instance was active.
//!
//! Three things are recorded:
//!
//! - **Allocations.** A generation is consumed in `prepare`, before any carrier
//!   starts. Recording the allocation means a crash between preparation and
//!   activation burns the generation instead of handing the same label to real
//!   work later; the promise is therefore about *consumed* generations, not
//!   only published ones.
//! - **Activations.** Every commit records the
//!   `(package, permission scope, instance, generation, registry epoch)` pointer
//!   before its epoch is published. A restarted host seeds its epoch and
//!   per-package generation watermark from the record, so a new instance never
//!   reuses a generation a previous run handed out.
//! - **Stops.** A stop clears the active pointer and names its reason — but only
//!   when it matches the recorded pointer *in full* and only when the reason is
//!   evidence the owner is gone ([`stop_clears_active`]). A stop for another
//!   generation never clears a newer pointer, and a stop whose clearing cannot
//!   be written is reported as an anomaly instead of being hidden.
//!
//! Three rules make the record safe to trust rather than merely present:
//!
//! 1. **One root, one writer.** Two mechanisms, because two different mistakes
//!    are possible. [`RuntimeCatalogJournal::open`] takes an exclusive file
//!    lease on the root's `journal/` directory (a locked lock file; the kernel
//!    releases it when the holder exits), so a second *object* or a second
//!    *process* is refused with `runtime_catalog_writer_busy`. That lease is
//!    taken once per journal, though, and says nothing about how many
//!    [`super::host::ExtensionHost`]s share one journal object — so
//!    [`CatalogJournal::claim_writer`] adds the second half: a host claims the
//!    journal's single writer slot at construction and holds it for its
//!    lifetime. A second host over the same journal is refused *there*, before it
//!    prepares anything or starts a carrier, and the slot is released when the
//!    first host drops so a later host can take over. Readers are unrestricted:
//!    any number of surfaces may share the host and its snapshots, and a
//!    read-only journal reference is fine. Without this, two hosts both read
//!    watermark zero and both publish generation one: a split catalogue that
//!    file locking cannot see, because both write through the same lock.
//!    [`super::identity::HostIncarnation`] only refuses *handles* from another
//!    run; it cannot stop a second owner from writing.
//! 2. **Complete lines are facts; they must parse.** A complete line that is not
//!    this record's shape — an unknown version, a damaged middle line — fails
//!    closed with `runtime_catalog_corrupt` instead of being skipped. Skipping
//!    would forget active pointers and quietly allow a takeover.
//! 3. **Only the unterminated tail may be unreadable, and it is repaired before
//!    the next append.** The write protocol is line-terminated, so everything
//!    after the last newline is the one part a crash can leave unfinished. A
//!    tail that still parses is a complete fact whose terminator was lost: it is
//!    kept and terminated. A tail that does not parse is provably uncommitted and
//!    is truncated — never merged into the next record, which would return `Ok`
//!    from the append while the reopened file skipped the whole line.
//!
//! **Identity is not here.** A binding carries a
//! [`super::identity::HostIncarnation`], which is random per run, so no durable
//! record can make an old handle valid again; the record makes *generations and
//! epochs* monotonic, the incarnation makes *handles* unforgeable. Both are
//! needed, and neither replaces the other.
//!
//! **What a restarted host does with the active pointers it finds.** They are
//! *not* dropped and not adopted: each one becomes a predecessor whose owner has
//! not been confirmed. Adopting would claim a process this run never started;
//! dropping would let a new instance of the same package and permission scope
//! take over silently while the previous owner may still be running. A
//! predecessor therefore blocks new activation for that scope until its owner is
//! confirmed stopped, and it stays visible in every catalogue snapshot until
//! then.
//!
//! Two implementations exist and they are honestly different:
//! [`RuntimeCatalogJournal`] writes real files under a managed root and reports
//! [`JournalDurability::Durable`]; [`MemoryCatalogJournal`] is an in-memory
//! fixture that reports [`JournalDurability::ProcessLocal`] and is never counted
//! as durable, whatever it is wired into.

use std::collections::BTreeMap;
use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use licoup_application::{ApplicationFailure, RecoveryAction};
use serde::{Deserialize, Serialize};

use crate::platform::extension_packages::{
    append_journal_line, ensure_private_directory, now_unix_ms,
};

use super::refusal;

/// How much of the record a host will read. Bounded like the install journal: a
/// record that grew past this is refused rather than silently truncated.
const MAX_RECORD_BYTES: usize = 8 * 1024 * 1024;

/// The record's file name inside the managed `journal/` directory.
const RECORD_FILE: &str = "runtime-catalog.jsonl";

/// The lock file that carries the root's single-writer lease.
const LOCK_FILE: &str = "runtime-catalog.lock";

const JOURNAL_STAGE: &str = "extension/catalog-journal";

/// What a journal can promise about surviving a restart.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum JournalDurability {
    /// Real files under a managed root. A record written here is found again by
    /// the next run, and only this mode counts as durable identity.
    Durable {
        /// The managed root the record lives under. A local path, never
        /// published through a catalogue document.
        root: PathBuf,
    },
    /// The record lives in memory only. Fixtures and compositions without a
    /// store; never reported as durable.
    ProcessLocal,
}

impl JournalDurability {
    pub const fn is_durable(&self) -> bool {
        matches!(self, Self::Durable { .. })
    }
}

/// A held claim on a journal's single writer slot.
#[derive(Debug)]
struct WriterClaimLease;

/// The single writer slot of one journal.
///
/// The journal keeps only a weak reference, so the slot frees as soon as the
/// permit holding the strong reference drops.
#[derive(Debug, Default)]
struct WriterClaim {
    holder: Mutex<Option<std::sync::Weak<WriterClaimLease>>>,
}

impl WriterClaim {
    fn claim(&self) -> Result<CatalogWriterPermit, ApplicationFailure> {
        let mut holder = match self.holder.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        };
        if holder.as_ref().and_then(std::sync::Weak::upgrade).is_some() {
            return Err(writer_busy());
        }
        let lease = Arc::new(WriterClaimLease);
        *holder = Some(Arc::downgrade(&lease));
        Ok(CatalogWriterPermit { _lease: lease })
    }
}

/// The right to be one journal's catalogue writer, held for a host's lifetime.
///
/// One managed root has one *writing* host. The file lease in
/// [`RuntimeCatalogJournal::open`] stops a second journal object or process; it
/// cannot stop a second host from sharing the same journal object, where both
/// would read watermark zero and each publish its own generation one. This
/// permit makes that uniqueness explicit: a host claims it at construction, a
/// second host is refused there — before it prepares or starts a carrier — and
/// the claim is released when the first host drops, so a later host takes over
/// and inherits the previous run's unconfirmed owners.
///
/// It is not `Clone`: one journal has exactly one writer. Readers are not
/// restricted by it; any number of surfaces may share the host and its
/// snapshots, and a read-only journal reference needs no permit.
#[derive(Debug)]
pub struct CatalogWriterPermit {
    _lease: Arc<WriterClaimLease>,
}

/// The refusal for a second writer of one journal.
fn writer_busy() -> ApplicationFailure {
    refusal("runtime_catalog_writer_busy", JOURNAL_STAGE)
        .with_field("journal")
        .with_recovery(RecoveryAction::RetryAfterRecovery)
}

/// The stable facts of one active instance.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ActivePointer {
    pub package_id: String,
    pub package_version: String,
    /// The approved permission scope, canonical (sorted, deduplicated) so two
    /// records of the same scope compare equal.
    pub permission_scope: Vec<String>,
    pub instance_id: String,
    pub generation: u64,
    pub registry_epoch: u64,
}

/// Why an instance stopped being active.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum StopReason {
    /// The generation was replaced by a newer one and drained.
    Drained,
    /// Authority was withdrawn.
    Revoked,
    /// The carrier faulted and the instance was isolated.
    Faulted,
    /// A restart found the instance missing.
    ReconciledAfterRestart,
    /// The release itself failed; the process may still be running.
    ReleaseFailed,
    /// The original owner confirmed a predecessor from an earlier run.
    PredecessorConfirmed,
}

/// Whether a stop is evidence that the owner is gone.
///
/// A release that failed and a reconcile that never observed the process are
/// *not*: their pointers stay in the record, so the next run keeps the
/// unverified owner instead of inheriting a clean-looking slot. Only a confirmed
/// release clears the slot.
pub const fn stop_clears_active(reason: StopReason) -> bool {
    !matches!(
        reason,
        StopReason::ReleaseFailed | StopReason::ReconciledAfterRestart
    )
}

/// The durable watermark a restarted host must not go below.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CatalogWatermark {
    /// The highest epoch ever published: commits, and the catalogue changes a
    /// stop or a predecessor resolution caused.
    pub epoch: u64,
    /// The highest generation ever *consumed* per package: allocations count,
    /// not only published activations, so a preparation that crashed before
    /// committing cannot hand its label to a later run.
    #[serde(default)]
    pub generations: BTreeMap<String, u64>,
    /// The pointers recorded active and not yet stopped. A restarted host keeps
    /// these as unconfirmed predecessors; it neither adopts nor drops them.
    #[serde(default)]
    pub active: Vec<ActivePointer>,
}

/// One activation to record before its epoch is published.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ActivationRecord {
    pub pointer: ActivePointer,
}

/// One stop to record.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StopRecord {
    pub pointer: ActivePointer,
    pub reason: StopReason,
    /// The catalogue epoch the stop published. A stop advances the catalogue,
    /// so the epoch watermark must not be seeded below it after a restart.
    #[serde(default)]
    pub epoch: u64,
}

/// The durable record one host run reads at start and writes as it works.
///
/// A record that cannot be written must fail the operation that depends on it:
/// an activation whose pointer was not recorded, a preparation whose allocation
/// was not, or a stop whose clearing was not — those are exactly the states this
/// seam exists to prevent. A journal that cannot say it is durable must not be
/// counted as such.
pub trait CatalogJournal: Send + Sync {
    /// What this journal can promise about surviving a restart.
    fn durability(&self) -> JournalDurability;

    /// Claim the right to be this journal's catalogue writer.
    ///
    /// A host calls this at construction and holds the permit for its lifetime;
    /// a second host over the same journal is refused with
    /// `runtime_catalog_writer_busy` before it prepares anything.
    fn claim_writer(&self) -> Result<CatalogWriterPermit, ApplicationFailure>;

    /// What a previous run left behind.
    fn watermark(&self) -> Result<CatalogWatermark, ApplicationFailure>;

    /// Record a generation consumed by `prepare`, before any carrier starts.
    fn record_allocation(
        &self,
        package_id: &str,
        generation: u64,
    ) -> Result<(), ApplicationFailure>;

    /// Record an activation before its epoch is published.
    fn record_activation(&self, record: &ActivationRecord) -> Result<(), ApplicationFailure>;

    /// Record that an instance is no longer active.
    fn record_stop(&self, record: &StopRecord) -> Result<(), ApplicationFailure>;
}

/// An in-memory journal for fixtures.
///
/// It keeps the same facts a durable one would and the same failure surface, and
/// it reports [`JournalDurability::ProcessLocal`]: a host wired to this is *not*
/// restart-safe, and [`super::host::ExtensionHost::identity_is_durable`] says so
/// even though a journal is present.
pub struct MemoryCatalogJournal {
    state: Mutex<MemoryState>,
    claim: WriterClaim,
    fail_allocation: AtomicBool,
    fail_activation: AtomicBool,
    fail_stop: AtomicBool,
}

#[derive(Default)]
struct MemoryState {
    watermark: CatalogWatermark,
    stops: Vec<StopRecord>,
}

impl Default for MemoryCatalogJournal {
    fn default() -> Self {
        Self::new()
    }
}

impl MemoryCatalogJournal {
    pub fn new() -> Self {
        Self {
            state: Mutex::new(MemoryState::default()),
            claim: WriterClaim::default(),
            fail_allocation: AtomicBool::new(false),
            fail_activation: AtomicBool::new(false),
            fail_stop: AtomicBool::new(false),
        }
    }

    /// Make the next allocation record fail.
    pub fn fail_next_allocation(&self) {
        self.fail_allocation.store(true, Ordering::SeqCst);
    }

    /// Make the next activation record fail, to exercise the rule that an
    /// unrecorded activation must not commit.
    pub fn fail_next_activation(&self) {
        self.fail_activation.store(true, Ordering::SeqCst);
    }

    /// Make the next stop record fail, to exercise the anomaly surface.
    pub fn fail_next_stop(&self) {
        self.fail_stop.store(true, Ordering::SeqCst);
    }

    /// The active pointers as recorded, for tests and diagnostics.
    pub fn active(&self) -> Vec<ActivePointer> {
        self.state
            .lock()
            .expect("journal state")
            .watermark
            .active
            .clone()
    }

    /// The stops recorded, for tests.
    pub fn stops(&self) -> Vec<StopRecord> {
        self.state.lock().expect("journal state").stops.clone()
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, MemoryState> {
        match self.state.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        }
    }

    fn record_failure(&self) -> ApplicationFailure {
        refusal("extension_catalog_journal_failed", JOURNAL_STAGE).with_field("instanceId")
    }
}

impl CatalogJournal for MemoryCatalogJournal {
    fn durability(&self) -> JournalDurability {
        JournalDurability::ProcessLocal
    }

    fn claim_writer(&self) -> Result<CatalogWriterPermit, ApplicationFailure> {
        self.claim.claim()
    }

    fn watermark(&self) -> Result<CatalogWatermark, ApplicationFailure> {
        Ok(self.lock().watermark.clone())
    }

    fn record_allocation(
        &self,
        package_id: &str,
        generation: u64,
    ) -> Result<(), ApplicationFailure> {
        if self.fail_allocation.swap(false, Ordering::SeqCst) {
            return Err(self.record_failure());
        }
        let mut state = self.lock();
        let recorded = state
            .watermark
            .generations
            .entry(package_id.to_owned())
            .or_insert(0);
        *recorded = (*recorded).max(generation);
        Ok(())
    }

    fn record_activation(&self, record: &ActivationRecord) -> Result<(), ApplicationFailure> {
        if self.fail_activation.swap(false, Ordering::SeqCst) {
            return Err(self.record_failure());
        }
        let mut state = self.lock();
        let pointer = &record.pointer;
        state.watermark.epoch = state.watermark.epoch.max(pointer.registry_epoch);
        let generation = state
            .watermark
            .generations
            .entry(pointer.package_id.clone())
            .or_insert(0);
        *generation = (*generation).max(pointer.generation);
        state
            .watermark
            .active
            .retain(|active| active.instance_id != pointer.instance_id);
        state.watermark.active.push(pointer.clone());
        Ok(())
    }

    fn record_stop(&self, record: &StopRecord) -> Result<(), ApplicationFailure> {
        if self.fail_stop.swap(false, Ordering::SeqCst) {
            return Err(self.record_failure());
        }
        let mut state = self.lock();
        if stop_clears_active(record.reason) {
            clear_matching_pointer(&mut state.watermark.active, &record.pointer);
        }
        state.watermark.epoch = state.watermark.epoch.max(record.epoch);
        // A stop is keyed by instance: confirming a stop that was already
        // recorded replaces it instead of pretending two stops happened.
        state
            .stops
            .retain(|stop| stop.pointer.instance_id != record.pointer.instance_id);
        state.stops.push(record.clone());
        Ok(())
    }
}

/// Clear the recorded pointer only when the stop names the *same* pointer.
///
/// A stop for another generation of the same instance id is not evidence about
/// this pointer, and clearing it would forget a live owner.
fn clear_matching_pointer(active: &mut Vec<ActivePointer>, stopped: &ActivePointer) {
    active.retain(|recorded| {
        recorded.instance_id != stopped.instance_id
            || recorded.package_id != stopped.package_id
            || recorded.generation != stopped.generation
            || recorded.registry_epoch != stopped.registry_epoch
    });
}

/// One line of the runtime catalogue record.
///
/// The record is append-only and line-per-fact, like the install journal: a
/// reader folds it, and only the final unterminated line may be unreadable.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(
    tag = "kind",
    rename_all = "kebab-case",
    rename_all_fields = "camelCase"
)]
enum RuntimeRecord {
    Allocation {
        at_unix_ms: i64,
        package_id: String,
        generation: u64,
    },
    Activation {
        at_unix_ms: i64,
        pointer: ActivePointer,
    },
    Stop {
        at_unix_ms: i64,
        pointer: ActivePointer,
        reason: StopReason,
        #[serde(default)]
        epoch: u64,
    },
}

/// The root's single-writer lease.
///
/// The lock file is opened once and locked exclusively for as long as the
/// journal lives; the kernel drops the lock when the holder exits, so a crashed
/// writer does not leave a lease nobody can take. A second opener — another
/// object in this process or another process — gets
/// `runtime_catalog_writer_busy`.
#[derive(Debug)]
struct WriterLease {
    _file: File,
}

impl WriterLease {
    fn acquire(directory: &Path) -> Result<Self, ApplicationFailure> {
        let path = directory.join(LOCK_FILE);
        let file = OpenOptions::new()
            .create(true)
            // The lock file carries no content; truncation would be a no-op and
            // saying so keeps the intent explicit.
            .truncate(false)
            .read(true)
            .write(true)
            .open(&path)
            .map_err(|_| unavailable("lock"))?;
        match file.try_lock() {
            Ok(()) => Ok(Self { _file: file }),
            Err(std::fs::TryLockError::WouldBlock) => {
                Err(refusal("runtime_catalog_writer_busy", JOURNAL_STAGE)
                    .with_field("root")
                    .with_recovery(RecoveryAction::RetryAfterRecovery))
            }
            Err(std::fs::TryLockError::Error(_)) => Err(unavailable("lock")),
        }
    }
}

/// The real record: `journal/runtime-catalog.jsonl` under a managed root,
/// written with the same bounded, fsynced, append-only rules as the install
/// journal.
///
/// It records runtime facts only. Installed versions and package bytes stay
/// [`crate::platform::extension_packages::InstallJournal`]'s, so this is not a
/// second copy of anything the store already owns. It is not `Clone`: one root
/// has one writer, and a second [`RuntimeCatalogJournal::open`] on the same root
/// is refused while this one lives.
#[derive(Debug)]
pub struct RuntimeCatalogJournal {
    root: PathBuf,
    path: PathBuf,
    _lease: WriterLease,
    /// Serializes one process's own appends (repair plus write) even though the
    /// lease already excludes other writers.
    append: Mutex<()>,
    /// The single host writer slot; see [`CatalogWriterPermit`].
    claim: WriterClaim,
}

impl RuntimeCatalogJournal {
    /// Open (creating when absent) the runtime catalogue record under a managed
    /// root, taking the root's single-writer lease.
    ///
    /// The `journal/` directory is shared with the install journal; the record
    /// file is created on the first record. A second opener on the same root is
    /// refused with `runtime_catalog_writer_busy`.
    pub fn open(root: &Path) -> Result<Self, ApplicationFailure> {
        let directory = root.join("journal");
        ensure_private_directory(&directory)?;
        let lease = WriterLease::acquire(&directory)?;
        Ok(Self {
            root: root.to_path_buf(),
            path: directory.join(RECORD_FILE),
            _lease: lease,
            append: Mutex::new(()),
            claim: WriterClaim::default(),
        })
    }

    /// The managed root this record belongs to.
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// The record file.
    pub fn path(&self) -> &Path {
        &self.path
    }

    fn lock_append(&self) -> std::sync::MutexGuard<'_, ()> {
        match self.append.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        }
    }

    fn append(&self, record: &RuntimeRecord) -> Result<(), ApplicationFailure> {
        let _guard = self.lock_append();
        // The tail is repaired under this writer's exclusive boundary before
        // anything is appended to it: a half-written line must never be merged
        // into the next record.
        self.repair_tail()?;
        let line = serde_json::to_string(record)
            .map_err(|_| refusal("runtime_catalog_record_invalid", JOURNAL_STAGE))?;
        append_journal_line(&self.path, &line).map_err(|failure| {
            // The line is ours; the failure is the shared writer's. Keep the
            // chain readable by naming our record and the cause.
            unavailable_cause(&failure.code)
        })
    }

    /// Repair an unterminated tail under the writer's exclusive boundary.
    ///
    /// A tail that parses is a complete fact whose terminator was lost: it is
    /// terminated, not dropped. A tail that does not parse is provably
    /// uncommitted and is truncated to the last newline. Nothing before that
    /// boundary is touched.
    fn repair_tail(&self) -> Result<(), ApplicationFailure> {
        let bytes = match std::fs::read(&self.path) {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(_) => return Err(unavailable("read")),
        };
        if bytes.is_empty() || bytes.ends_with(b"\n") {
            return Ok(());
        }
        if bytes.len() > MAX_RECORD_BYTES {
            return Err(too_large());
        }
        let tail_start = bytes
            .iter()
            .rposition(|byte| *byte == b'\n')
            .map(|last| last + 1)
            .unwrap_or(0);
        if tail_is_a_complete_fact(&bytes[tail_start..]) {
            let mut file = OpenOptions::new()
                .append(true)
                .open(&self.path)
                .map_err(|_| unavailable("append"))?;
            file.write_all(b"\n")
                .and_then(|()| file.sync_all())
                .map_err(|_| unavailable("append"))?;
            return Ok(());
        }
        let file = OpenOptions::new()
            .write(true)
            .open(&self.path)
            .map_err(|_| unavailable("repair"))?;
        file.set_len(tail_start as u64)
            .and_then(|()| file.sync_all())
            .map_err(|_| unavailable("repair"))
    }
}

impl CatalogJournal for RuntimeCatalogJournal {
    fn durability(&self) -> JournalDurability {
        JournalDurability::Durable {
            root: self.root.clone(),
        }
    }

    fn claim_writer(&self) -> Result<CatalogWriterPermit, ApplicationFailure> {
        self.claim.claim()
    }

    fn watermark(&self) -> Result<CatalogWatermark, ApplicationFailure> {
        let bytes = match std::fs::read(&self.path) {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Ok(CatalogWatermark::default());
            }
            Err(_) => return Err(unavailable("read")),
        };
        if bytes.len() > MAX_RECORD_BYTES {
            return Err(too_large());
        }
        fold_record(&bytes)
    }

    fn record_allocation(
        &self,
        package_id: &str,
        generation: u64,
    ) -> Result<(), ApplicationFailure> {
        self.append(&RuntimeRecord::Allocation {
            at_unix_ms: now_unix_ms(),
            package_id: package_id.to_owned(),
            generation,
        })
    }

    fn record_activation(&self, record: &ActivationRecord) -> Result<(), ApplicationFailure> {
        self.append(&RuntimeRecord::Activation {
            at_unix_ms: now_unix_ms(),
            pointer: record.pointer.clone(),
        })
    }

    fn record_stop(&self, record: &StopRecord) -> Result<(), ApplicationFailure> {
        self.append(&RuntimeRecord::Stop {
            at_unix_ms: now_unix_ms(),
            pointer: record.pointer.clone(),
            reason: record.reason,
            epoch: record.epoch,
        })
    }
}

/// Whether an unterminated tail is a complete record missing only its newline.
fn tail_is_a_complete_fact(tail: &[u8]) -> bool {
    let Ok(text) = std::str::from_utf8(tail) else {
        return false;
    };
    if text.trim().is_empty() {
        return false;
    }
    serde_json::from_str::<RuntimeRecord>(text).is_ok()
}

/// Fold the record's bytes into a watermark, fail-closed.
///
/// Everything up to and including the last newline is complete and must parse;
/// only the unterminated tail may be unreadable, and it still counts when it is
/// a complete fact whose terminator was lost.
fn fold_record(bytes: &[u8]) -> Result<CatalogWatermark, ApplicationFailure> {
    let (complete, tail) = match bytes.iter().rposition(|byte| *byte == b'\n') {
        Some(last) => (&bytes[..=last], &bytes[last + 1..]),
        None => (&bytes[..0], bytes),
    };
    let complete = std::str::from_utf8(complete)
        .map_err(|_| refusal("runtime_catalog_corrupt", JOURNAL_STAGE).with_field("record"))?;
    let mut watermark = CatalogWatermark::default();
    let mut active: BTreeMap<String, ActivePointer> = BTreeMap::new();
    for (index, line) in complete.split_terminator('\n').enumerate() {
        if line.trim().is_empty() {
            continue;
        }
        let record: RuntimeRecord =
            serde_json::from_str(line).map_err(|_| corrupt_line(index + 1))?;
        fold_one(&mut watermark, &mut active, record);
    }
    if !tail.is_empty()
        && let Ok(text) = std::str::from_utf8(tail)
        && !text.trim().is_empty()
        && let Ok(record) = serde_json::from_str::<RuntimeRecord>(text)
    {
        fold_one(&mut watermark, &mut active, record);
    }
    watermark.active = active.into_values().collect();
    Ok(watermark)
}

fn fold_one(
    watermark: &mut CatalogWatermark,
    active: &mut BTreeMap<String, ActivePointer>,
    record: RuntimeRecord,
) {
    match record {
        RuntimeRecord::Allocation {
            package_id,
            generation,
            ..
        } => {
            let recorded = watermark.generations.entry(package_id).or_insert(0);
            *recorded = (*recorded).max(generation);
        }
        RuntimeRecord::Activation { pointer, .. } => {
            watermark.epoch = watermark.epoch.max(pointer.registry_epoch);
            let recorded = watermark
                .generations
                .entry(pointer.package_id.clone())
                .or_insert(0);
            *recorded = (*recorded).max(pointer.generation);
            active.insert(pointer.instance_id.clone(), pointer);
        }
        RuntimeRecord::Stop {
            pointer,
            reason,
            epoch,
            ..
        } => {
            watermark.epoch = watermark.epoch.max(epoch);
            if stop_clears_active(reason) {
                let matches = active.get(&pointer.instance_id).is_some_and(|recorded| {
                    recorded.package_id == pointer.package_id
                        && recorded.generation == pointer.generation
                        && recorded.registry_epoch == pointer.registry_epoch
                });
                if matches {
                    active.remove(&pointer.instance_id);
                }
            }
        }
    }
}

fn unavailable(action: &str) -> ApplicationFailure {
    refusal("runtime_catalog_unavailable", JOURNAL_STAGE).with_presentation_arg("action", action)
}

fn unavailable_cause(cause: &str) -> ApplicationFailure {
    refusal("runtime_catalog_unavailable", JOURNAL_STAGE).with_presentation_arg("cause", cause)
}

fn too_large() -> ApplicationFailure {
    refusal("runtime_catalog_too_large", JOURNAL_STAGE).with_field("record")
}

fn corrupt_line(line: usize) -> ApplicationFailure {
    refusal("runtime_catalog_corrupt", JOURNAL_STAGE)
        .with_field("line")
        .with_presentation_arg("line", &line.to_string())
}
