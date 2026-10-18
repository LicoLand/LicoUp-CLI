//! One rehearsal entry: convert a released-format root and round-trip it, reporting
//! exactly which stages ran.
//!
//! The rehearsal is not a second migration and not a second archive. It sequences the
//! owners that already exist — the client's own admission owner for conversion, the native
//! full-data-root archive owner for capture and restore — and turns their verdicts into one
//! report. Nothing here decides what a domain is, what a container holds or what a target
//! version is.
//!
//! Three properties are what the entry is built around:
//!
//! 1. **The named root is only ever read.** Conversion and archiving happen against a
//!    disposable copy this module creates inside the caller's working directory, so a
//!    rehearsal on a machine with live state cannot change it. The copy is the subject, and
//!    a failed stage therefore cannot be confused with a converted or restored root.
//! 2. **Every stage is explicit.** The report carries the whole stage list on every run. A
//!    stage that did not run is reported as not run with the stage that blocked it, never
//!    omitted, so a partial rehearsal cannot read as a complete recovery.
//! 3. **The roots speak for themselves.** Every stage records the fingerprint of the root it
//!    produced, so an independent comparison can read what is on disk instead of trusting
//!    this report. A stage whose named root is absent fails its own oracle.
//!
//! A refusal that belongs to the run rather than to a stage — a source that is not at the
//! last published format, a missing source, a missing writer statement — is returned as a
//! typed [`ToolError`] before anything is written. A refusal that belongs to a stage — an
//! export the owner declines, a restore the owner rejects — is recorded on that stage and
//! leaves every later stage visibly not run.

use crate::archive;
use crate::error::{ToolError, ToolResult, WORK_ROOT_INSIDE_DATA_ROOT, WRITERS_RUNNING};
use crate::journal::ledger::LedgerSnapshot;
use licoup_native::core::full_data_root_archive::{ADMISSION_LOCK_PATH, RecoveryCoverage};
use serde::Serialize;
use std::fs;
use std::path::{Path, PathBuf};

/// The rehearsal refused because the named source is not a data root at all.
pub const SOURCE_ROOT_MISSING: ToolError = ToolError::new("source_root_missing");
/// The rehearsal refused because the named source is not a directory.
pub const SOURCE_ROOT_NOT_DIRECTORY: ToolError = ToolError::new("source_root_not_directory");
/// The rehearsal refused because the named source is empty, so it carries no released shape.
pub const SOURCE_ROOT_SHAPE_ABSENT: ToolError = ToolError::new("source_root_shape_absent");
/// The rehearsal refused because the named source already declares this binary's target
/// frontier, so it is not a root the last published release left.
pub const SOURCE_ROOT_ALREADY_AT_TARGET: ToolError =
    ToolError::new("source_root_already_at_target");

/// The stage list every report carries, in execution order.
pub const STAGES: &[&str] = &[
    "source-observed",
    "copied",
    "converted",
    "archive-zip",
    "restore-zip",
    "reopen-zip",
    "archive-tar-gz",
    "restore-tar-gz",
    "reopen-tar-gz",
];

/// What one stage of the rehearsal observed.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum StageOutcome {
    /// The stage ran and the owner it drives answered.
    Observed,
    /// The stage did not run; the reason names the stage that stopped it.
    NotRun,
    /// The stage ran and the owner refused it, carrying the owner's own code.
    Refused,
}

/// The fingerprint of one root, as an independent comparison must read it.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RootFingerprint {
    /// Every directory and regular file below the root except the archive owner's
    /// ephemeral admission-coordination file.
    pub entries: u64,
    pub bytes: u64,
    /// A stable digest over every included entry's relative name, kind and size.
    ///
    /// The same tree always folds to the same value, and any rewrite that changes a name, an
    /// entry's kind or a size changes it. It is not a cryptographic commitment and is never
    /// used as one: a caller that needs byte identity compares the roots themselves.
    pub digest: String,
}

/// One stage's observed result.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StageResult {
    pub stage: &'static str,
    pub outcome: StageOutcome,
    /// The root this stage produced, when it produced one.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub root: Option<String>,
    /// The fingerprint of that root once the stage finished.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub root_fingerprint: Option<RootFingerprint>,
    /// What the owner this stage drove reported, or why the stage did not run.
    pub observed: String,
    /// The owner's own refusal code, when it refused.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub refusal: Option<String>,
    /// Anything else this stage observed that its root does not state.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
}

impl StageResult {
    fn not_run(stage: &'static str, reason: String) -> Self {
        Self {
            stage,
            outcome: StageOutcome::NotRun,
            root: None,
            root_fingerprint: None,
            observed: reason,
            refusal: None,
            detail: None,
        }
    }
}

/// What one rehearsal concluded.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RehearsalReport {
    /// `complete` only when every stage ran; `incomplete` otherwise.
    pub status: &'static str,
    /// The product version this binary reports.
    pub running_product_version: String,
    /// The frontier this binary's client declares as its target.
    pub frontier_id: String,
    /// The target frontier the named source declared, when it declared one.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source_frontier_id: Option<String>,
    /// The product version the named source's own ledger recorded, when it recorded one.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source_admitted_product_version: Option<String>,
    /// The fingerprint of the named source before this run touched anything.
    pub source_fingerprint: RootFingerprint,
    /// The disposable working root this run created.
    pub work_root: String,
    /// Whether the working root still exists after the run.
    pub work_root_retained: bool,
    /// Every stage, in execution order; stages that did not run are named here.
    pub stages: Vec<StageResult>,
    /// The stages that did not run.
    pub not_run: Vec<&'static str>,
    /// Domains the client's owner did not account for; a non-empty list is unfinished work.
    pub still_owed: Vec<String>,
    /// Domains the owner left waiting for platform authority this tool does not hold.
    pub pending_authorization: Vec<String>,
    /// Domains the owner converted during this rehearsal.
    pub converted: Vec<String>,
    /// Whether this rehearsal observed a complete recovery.
    pub recovery_complete: bool,
}

impl RehearsalReport {
    pub fn is_complete(&self) -> bool {
        self.recovery_complete
    }

    pub fn stage(&self, name: &str) -> Option<&StageResult> {
        self.stages.iter().find(|stage| stage.stage == name)
    }
}

/// How a caller asked for the rehearsal.
#[derive(Clone, Debug)]
pub struct RehearsalRequest {
    /// The synthetic released-format root this rehearsal reads and never writes.
    pub data_root: PathBuf,
    /// The disposable directory the rehearsal stages, converts, archives and restores in.
    pub work_root: PathBuf,
    /// The operator's statement that no writer is running against the source root.
    pub writers_stopped: bool,
    /// Keep the working root after the run. A caller that compares the stage roots reads
    /// them after the report; a caller that only ran the rehearsal lets it be removed.
    pub keep_work_root: bool,
}

/// What the named source declares, read without writing anything.
///
/// A refusal names the shape it refused, because "not the last published format" is only
/// actionable with the format the root actually carries. The read is the same one the
/// rehearsal's first stage makes: the client's own ledger, and the tree's own fingerprint.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SourceShape {
    pub present: bool,
    pub directory: bool,
    /// The frontier the source's own ledger declares, when it declares one.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub frontier_id: Option<String>,
    /// The product version the source's own ledger recorded, when it recorded one.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub admitted_product_version: Option<String>,
    /// The frontier this binary's client declares as its target.
    pub target_frontier_id: String,
    pub entries: u64,
    pub digest: String,
}

/// Read the shape of one named source without creating, moving or removing anything.
pub fn source_shape(data_root: &Path) -> ToolResult<SourceShape> {
    let present = data_root.exists();
    let directory = data_root.is_dir();
    let target_frontier_id =
        licoup_native::domain::client_state_migration::frontier_projection_struct()
            .map_err(|_| ToolError::new("migration_frontier_unavailable"))?
            .frontier_id;
    let ledger = if directory {
        LedgerSnapshot::read(data_root)?.parse()?
    } else {
        None
    };
    let fingerprint = if directory {
        fingerprint(data_root)
    } else {
        RootFingerprint {
            entries: 0,
            bytes: 0,
            digest: "0".repeat(16),
        }
    };
    Ok(SourceShape {
        present,
        directory,
        frontier_id: ledger.as_ref().map(|ledger| ledger.frontier_id.clone()),
        admitted_product_version: ledger
            .as_ref()
            .map(|ledger| ledger.highest_admitted_product_version.clone()),
        target_frontier_id,
        entries: fingerprint.entries,
        digest: fingerprint.digest,
    })
}

/// Stage the source, convert it, round-trip both containers and reopen each restore.
pub fn rehearse(request: &RehearsalRequest) -> ToolResult<RehearsalReport> {
    let source = &request.data_root;
    if !source.exists() {
        return Err(SOURCE_ROOT_MISSING);
    }
    if !source.is_dir() {
        return Err(SOURCE_ROOT_NOT_DIRECTORY);
    }
    if !request.writers_stopped {
        return Err(WRITERS_RUNNING);
    }
    // The source is read-only for this run, so a working root inside it would be written
    // into the root the rehearsal promised not to change.
    if request.work_root.starts_with(source) {
        return Err(WORK_ROOT_INSIDE_DATA_ROOT);
    }

    let frontier = licoup_native::domain::client_state_migration::frontier_projection_struct()
        .map_err(|_| ToolError::new("migration_frontier_unavailable"))?;
    let source_fingerprint = fingerprint(source);

    // The released shape and the ledger are read before anything is created, so a refusal
    // here has written nothing at all: not the working root, not an archive, not a restore.
    let ledger: Option<crate::journal::ledger::ClientLedger> =
        LedgerSnapshot::read(source)?.parse()?;
    let source_frontier_id = ledger.as_ref().map(|ledger| ledger.frontier_id.clone());
    let source_admitted_product_version = ledger
        .as_ref()
        .map(|ledger| ledger.highest_admitted_product_version.clone());
    if source_frontier_id.as_deref() == Some(frontier.frontier_id.as_str()) {
        // The root already declares this binary's own target frontier. That is the format
        // this binary writes, not the format the last published release left, so there is no
        // fixed endpoint left to rehearse against it.
        return Err(SOURCE_ROOT_ALREADY_AT_TARGET);
    }
    if source_fingerprint.entries == 0 {
        // An empty directory carries no released shape to convert.
        return Err(SOURCE_ROOT_SHAPE_ABSENT);
    }

    let work_root = absolute(&request.work_root)?;
    let mut run = Run {
        source,
        work_root: work_root.clone(),
        frontier_id: frontier.frontier_id.clone(),
        domains: frontier
            .domains
            .iter()
            .map(|domain| domain.domain_id.clone())
            .collect(),
        converted: Vec::new(),
        still_owed: Vec::new(),
        pending_authorization: Vec::new(),
        coverage: Vec::new(),
        stages: Vec::with_capacity(STAGES.len()),
        stopped_by: None,
    };
    run.execute()?;

    let not_run: Vec<&'static str> = run
        .stages
        .iter()
        .filter(|stage| stage.outcome == StageOutcome::NotRun)
        .map(|stage| stage.stage)
        .collect();
    let complete_containers = run.coverage.iter().all(|coverage| *coverage);
    let recovery_complete = not_run.is_empty()
        && complete_containers
        && run.still_owed.is_empty()
        && run
            .stages
            .iter()
            .all(|stage| stage.outcome == StageOutcome::Observed);

    let work_root_retained = request.keep_work_root && work_root.exists();
    if !request.keep_work_root {
        // Only the directory this run created is removed: the caller's working directory is
        // not this tool's to delete, and the source was never inside it.
        let _ = fs::remove_dir_all(&work_root);
    }

    Ok(RehearsalReport {
        status: if recovery_complete {
            "complete"
        } else {
            "incomplete"
        },
        running_product_version:
            licoup_native::domain::client_state_migration::running_product_version()
                .unwrap_or_default()
                .to_string(),
        frontier_id: run.frontier_id,
        source_frontier_id,
        source_admitted_product_version,
        source_fingerprint,
        work_root: work_root.to_string_lossy().into_owned(),
        work_root_retained,
        stages: run.stages,
        not_run,
        still_owed: run.still_owed,
        pending_authorization: run.pending_authorization,
        converted: run.converted,
        recovery_complete,
    })
}

/// One rehearsal in progress, with the observable it accumulates.
struct Run<'a> {
    source: &'a Path,
    work_root: PathBuf,
    frontier_id: String,
    domains: Vec<String>,
    converted: Vec<String>,
    still_owed: Vec<String>,
    pending_authorization: Vec<String>,
    /// Whether each container's capture declared the owner's own recovery coverage: every
    /// archive is Limited because credential key material never travels, and the pending
    /// custody limitation must be visible instead of absent.
    coverage: Vec<bool>,
    stages: Vec<StageResult>,
    /// The stage whose refusal stopped the run, if one did.
    stopped_by: Option<&'static str>,
}

impl Run<'_> {
    /// Walk the fixed stage list once, recording every stage whether or not it runs.
    fn execute(&mut self) -> ToolResult<()> {
        fs::create_dir_all(&self.work_root)
            .map_err(|_| ToolError::new("rehearsal_work_root_unwritable"))?;
        let converted = self.work_root.join("converted");
        let restored = self.work_root.join("restored");

        self.observe_source();
        self.copy_source(&converted);
        self.convert(&converted);
        for (stage, archive_name, container) in [
            ("archive-zip", "converted.zip", "zip"),
            ("archive-tar-gz", "converted.tar.gz", "tar.gz"),
        ] {
            let destination = self.work_root.join(archive_name);
            if !self.archive(&converted, &destination, stage) {
                continue;
            }
            let target = restored.join(container);
            if !self.restore(&destination, &target, stage, container) {
                continue;
            }
            self.reopen(&target, stage, container);
        }
        Ok(())
    }

    /// Record what the named source declared, before anything is created.
    fn observe_source(&mut self) {
        self.push(StageResult {
            stage: "source-observed",
            outcome: StageOutcome::Observed,
            root: Some(self.source.to_string_lossy().into_owned()),
            root_fingerprint: Some(fingerprint(self.source)),
            observed: "source root read and fingerprinted; nothing was written".to_string(),
            refusal: None,
            detail: Some(format!(
                "the disposable working root is {}",
                self.work_root.to_string_lossy()
            )),
        });
    }

    /// Copy the released root into the disposable working root.
    ///
    /// The copy is the only thing conversion and archiving ever see, which is what makes the
    /// named source read-only by construction rather than by discipline.
    fn copy_source(&mut self, converted: &Path) {
        if !self.can_run("copied") {
            return;
        }
        let outcome = fs::create_dir_all(converted)
            .map_err(|_| ())
            .and_then(|()| copy_tree(self.source, converted).map_err(|_| ()));
        let copied = fingerprint(converted);
        let source_entries = fingerprint(self.source).entries;
        if outcome.is_err() {
            self.refuse(
                "copied",
                "rehearsal_copy_failed",
                "the released root could not be copied into the disposable working root",
                None,
            );
            return;
        }
        // A copy that dropped an entry would leave a root that no owner ever wrote, so the
        // stage compares the two trees before conversion is allowed to run.
        if copied.entries != source_entries {
            self.refuse(
                "copied",
                "rehearsal_copy_incomplete",
                "the disposable copy does not hold every entry the source holds",
                None,
            );
            return;
        }
        self.push(StageResult {
            stage: "copied",
            outcome: StageOutcome::Observed,
            root: Some(converted.to_string_lossy().into_owned()),
            root_fingerprint: Some(copied),
            observed: format!(
                "{} entries copied into the disposable working root",
                source_entries
            ),
            refusal: None,
            detail: None,
        });
    }

    /// Convert the disposable copy through the client's own admission owner.
    fn convert(&mut self, converted: &Path) {
        if !self.can_run("converted") {
            return;
        }
        match licoup_native::domain::client_state_migration::admit(converted) {
            Ok(admission) => {
                self.converted = admission.applied_domain_ids.clone();
                self.pending_authorization = admission.pending_authorization_domain_ids.clone();
                self.still_owed = self
                    .domains
                    .iter()
                    .filter(|domain| !self.accounted(domain, &admission))
                    .cloned()
                    .collect();
                self.push(StageResult {
                    stage: "converted",
                    outcome: StageOutcome::Observed,
                    root: Some(converted.to_string_lossy().into_owned()),
                    root_fingerprint: Some(fingerprint(converted)),
                    observed: format!(
                        "the client's admission owner reported {}: {} converted, {} already current, {} awaiting authority",
                        admission.status,
                        admission.applied_domain_ids.len(),
                        admission.skipped_domain_ids.len(),
                        admission.pending_authorization_domain_ids.len()
                    ),
                    refusal: None,
                    detail: Some(format!(
                        "target frontier {}; still owed: {}",
                        self.frontier_id,
                        if self.still_owed.is_empty() {
                            "none".to_string()
                        } else {
                            self.still_owed.join(", ")
                        }
                    )),
                });
            }
            Err(_) => self.refuse(
                "converted",
                "migration_owner_refused",
                "the client's own admission owner refused the disposable working root",
                Some(fingerprint(converted)),
            ),
        }
    }

    /// Capture the converted root into one plaintext container.
    fn archive(&mut self, converted: &Path, destination: &Path, stage: &'static str) -> bool {
        if !self.can_run(stage) {
            return false;
        }
        match archive::export(converted, destination, true) {
            Ok(report) => {
                // Custody is never proven by the archive itself: the owner reports Limited
                // coverage with the pending credential limitation on every capture. The
                // stage observes exactly that instead of asking for a Complete claim the
                // owner would refuse.
                self.coverage.push(
                    report.coverage == RecoveryCoverage::Limited
                        && report
                            .limitations
                            .iter()
                            .any(|limitation| limitation.domain == "gateway-credential-custody"),
                );
                self.push(StageResult {
                    stage,
                    outcome: StageOutcome::Observed,
                    root: Some(destination.to_string_lossy().into_owned()),
                    root_fingerprint: Some(file_fingerprint(destination)),
                    observed: format!(
                        "the archive owner captured {} files into the {} container with {} coverage",
                        report.file_count,
                        report.container,
                        if report.coverage == RecoveryCoverage::Complete {
                            "complete"
                        } else {
                            "limited"
                        }
                    ),
                    refusal: None,
                    detail: (!report.limitations.is_empty()).then(|| {
                        format!(
                            "the archive names {} limitation(s), so it is not a complete recovery",
                            report.limitations.len()
                        )
                    }),
                });
                true
            }
            Err(error) => {
                self.refuse(
                    stage,
                    error.code(),
                    "the archive owner refused the capture",
                    None,
                );
                false
            }
        }
    }

    /// Restore one archive into its own disposable destination.
    fn restore(
        &mut self,
        destination: &Path,
        target: &Path,
        stage: &'static str,
        container: &'static str,
    ) -> bool {
        let restore_stage = if container == "zip" {
            "restore-zip"
        } else {
            "restore-tar-gz"
        };
        if !self.can_run(restore_stage) {
            return false;
        }
        match archive::import(destination, target) {
            Ok(report) => {
                self.push(StageResult {
                    stage: restore_stage,
                    outcome: StageOutcome::Observed,
                    root: Some(target.to_string_lossy().into_owned()),
                    root_fingerprint: Some(fingerprint(target)),
                    observed: format!(
                        "the archive owner restored {} files from the {} container into an empty destination",
                        report.file_count, report.container
                    ),
                    refusal: None,
                    detail: Some(format!(
                        "the source archive {stage} is unchanged by the restore"
                    )),
                });
                true
            }
            Err(error) => {
                self.refuse(
                    restore_stage,
                    error.code(),
                    "the archive owner refused the restore",
                    None,
                );
                false
            }
        }
    }

    /// Reopen one restored root through the client's own admission owner.
    ///
    /// The oracle is the owner's own verdict: a restored root that still owes a move, or one
    /// whose domains the owner no longer accounts for, is not a recovered root.
    fn reopen(&mut self, target: &Path, restore_stage: &'static str, container: &'static str) {
        let stage = if container == "zip" {
            "reopen-zip"
        } else {
            "reopen-tar-gz"
        };
        if !self.can_run(stage) {
            return;
        }
        match licoup_native::domain::client_state_migration::admit(target) {
            Ok(admission) => {
                let unaccounted: Vec<String> = self
                    .domains
                    .iter()
                    .filter(|domain| !self.accounted(domain, &admission))
                    .cloned()
                    .collect();
                if !unaccounted.is_empty() || !admission.applied_domain_ids.is_empty() {
                    self.refuse(
                        stage,
                        "archive_restore_incomplete",
                        "the restored root still owes a conversion, so it is not the converted root",
                        Some(fingerprint(target)),
                    );
                    return;
                }
                self.push(StageResult {
                    stage,
                    outcome: StageOutcome::Observed,
                    root: Some(target.to_string_lossy().into_owned()),
                    root_fingerprint: Some(fingerprint(target)),
                    observed: format!(
                        "the client's admission owner reopened the restored root at frontier {} and converted nothing",
                        admission.frontier_id
                    ),
                    refusal: None,
                    detail: Some(format!(
                        "{} domains are at their target; the archive it came from was {}",
                        admission.skipped_domain_ids.len(),
                        restore_stage
                    )),
                });
            }
            Err(_) => self.refuse(
                stage,
                "migration_owner_refused",
                "the client's own admission owner refused the restored root",
                Some(fingerprint(target)),
            ),
        }
    }

    /// Whether one domain's outcome is one the owner accounted for.
    fn accounted(
        &self,
        domain: &str,
        admission: &licoup_native::domain::client_state_migration::AdmissionResult,
    ) -> bool {
        admission.applied_domain_ids.iter().any(|id| id == domain)
            || admission.skipped_domain_ids.iter().any(|id| id == domain)
            || admission
                .pending_authorization_domain_ids
                .iter()
                .any(|id| id == domain)
    }

    /// Whether a stage may run, or must be reported as not run behind the blocking stage.
    fn can_run(&mut self, stage: &'static str) -> bool {
        match self.stopped_by {
            None => true,
            Some(blocker) => {
                self.stages.push(StageResult::not_run(
                    stage,
                    format!("not run: the {blocker} stage refused before this stage could start"),
                ));
                false
            }
        }
    }

    /// Record a stage the owner refused and stop every stage behind it.
    fn refuse(
        &mut self,
        stage: &'static str,
        code: &'static str,
        observed: &str,
        root_fingerprint: Option<RootFingerprint>,
    ) {
        self.stopped_by = Some(stage);
        self.stages.push(StageResult {
            stage,
            outcome: StageOutcome::Refused,
            root: None,
            root_fingerprint,
            observed: observed.to_string(),
            refusal: Some(code.to_string()),
            detail: None,
        });
    }

    fn push(&mut self, stage: StageResult) {
        self.stages.push(stage);
    }
}

/// Copy every directory and regular file below `source` into `destination`.
///
/// Symbolic links are skipped rather than followed: the archive owner does not capture them
/// either, so following one here would put content into the copy that no owner would restore
/// and make the two roots disagree for a reason the rehearsal did not intend.
/// Copy one root's logical payload — regular files and directories, following no
/// symbolic links — into the rehearsal's disposable working root.
///
/// Directory permissions travel with the copy: the released producer writes its private
/// state directories owner-only, and the client's own readers verify that privacy, so a
/// copy that widened them would be refused by the owner for a reason the source does not
/// have. Permissions are applied after the subtree is written, so a restricted mode never
/// blocks the copy itself.
fn copy_tree(source: &Path, destination: &Path) -> std::io::Result<()> {
    for entry in fs::read_dir(source)? {
        let entry = entry?;
        let path = entry.path();
        let metadata = fs::symlink_metadata(&path)?;
        let target = destination.join(entry.file_name());
        if metadata.is_dir() {
            fs::create_dir_all(&target)?;
            copy_tree(&path, &target)?;
            fs::set_permissions(&target, metadata.permissions())?;
        } else if metadata.is_file() {
            fs::copy(&path, &target)?;
        }
    }
    Ok(())
}

/// Fold a root's shape into one stable fingerprint.
fn fingerprint(root: &Path) -> RootFingerprint {
    let mut entries: Vec<(String, u8, u64)> = Vec::new();
    collect(root, root, &mut entries);
    entries.sort();
    let mut digest = 0xcbf2_9ce4_8422_2325_u64;
    let mut bytes = 0_u64;
    for (relative, kind, size) in &entries {
        for byte in relative.as_bytes() {
            digest = (digest ^ u64::from(*byte)).wrapping_mul(0x0000_0100_0000_01b3);
        }
        digest = (digest ^ u64::from(*kind)).wrapping_mul(0x0000_0100_0000_01b3);
        for byte in size.to_le_bytes() {
            digest = (digest ^ u64::from(byte)).wrapping_mul(0x0000_0100_0000_01b3);
        }
        bytes += size;
    }
    RootFingerprint {
        entries: entries.len() as u64,
        bytes,
        digest: format!("{digest:016x}"),
    }
}

/// The fingerprint of one regular file, for a stage whose product is not a root.
fn file_fingerprint(path: &Path) -> RootFingerprint {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.is_file() => {
            let mut digest = 0xcbf2_9ce4_8422_2325_u64;
            if let Some(name) = path.file_name() {
                for byte in name.to_string_lossy().as_bytes() {
                    digest = (digest ^ u64::from(*byte)).wrapping_mul(0x0000_0100_0000_01b3);
                }
            }
            for byte in metadata.len().to_le_bytes() {
                digest = (digest ^ u64::from(byte)).wrapping_mul(0x0000_0100_0000_01b3);
            }
            RootFingerprint {
                entries: 1,
                bytes: metadata.len(),
                digest: format!("{digest:016x}"),
            }
        }
        _ => RootFingerprint {
            entries: 0,
            bytes: 0,
            digest: "0".repeat(16),
        },
    }
}

fn collect(root: &Path, directory: &Path, entries: &mut Vec<(String, u8, u64)>) {
    let Ok(read) = fs::read_dir(directory) else {
        return;
    };
    for entry in read.filter_map(Result::ok) {
        let path = entry.path();
        let Ok(metadata) = fs::symlink_metadata(&path) else {
            continue;
        };
        let relative = path
            .strip_prefix(root)
            .map(|value| value.to_string_lossy().replace('\\', "/"))
            .unwrap_or_default();
        // Admission recreates this ephemeral coordination file when a root is opened.
        // It is not archive payload, so it cannot make the same logical root appear to
        // change between restore and owner readback.
        if relative == ADMISSION_LOCK_PATH {
            continue;
        }
        if metadata.is_dir() {
            entries.push((relative, b'd', 0));
            collect(root, &path, entries);
        } else if metadata.is_file() {
            entries.push((relative, b'f', metadata.len()));
        }
    }
}

/// Resolve one caller-named path for use, creating nothing.
fn absolute(path: &Path) -> ToolResult<PathBuf> {
    match path.canonicalize() {
        Ok(resolved) => Ok(resolved),
        Err(_) => {
            let unresolvable = || ToolError::new("rehearsal_work_root_unwritable");
            let parent = path
                .parent()
                .filter(|parent| !parent.as_os_str().is_empty())
                .ok_or_else(unresolvable)?;
            let name = path.file_name().ok_or_else(unresolvable)?;
            let resolved = parent.canonicalize().map_err(|_| unresolvable())?;
            Ok(resolved.join(name))
        }
    }
}

#[cfg(test)]
mod tests {
    #![allow(dead_code)]

    use super::*;
    use licoup_foundation::platform::file_security::{
        atomic_write_private_text, ensure_private_dir,
    };

    include!("../../../tests/fixtures/client_state_migration/released_source.rs");

    /// The planned release identity the delivered tool is built with.
    const CANDIDATE_PRODUCT_VERSION: &str = "0.3.0";

    fn scratch(name: &str) -> PathBuf {
        let root = std::env::temp_dir()
            .canonicalize()
            .expect("temp dir")
            .join(format!("licoup-rehearse-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).expect("scratch");
        root
    }

    /// These cases convert the frozen v0.2.1 root, which the client's own guard refuses
    /// under a development identity. The candidate identity comes from the native build
    /// script's real `LICO_CLIENT_PRODUCT_VERSION` input; a fixture ledger is never lowered.
    fn assert_candidate_identity() {
        let running = licoup_native::domain::client_state_migration::running_product_version()
            .expect("embedded product identity");
        assert_eq!(
            running, CANDIDATE_PRODUCT_VERSION,
            "run these suites through tools/scripts/migration-crate-tests.mjs, which constructs \
             the candidate identity with the native build script's own input"
        );
    }

    /// Materialize the frozen released root through the released producers' own layouts.
    fn seed_released_root(root: &Path) {
        seed_released_conversation_store(root);
        seed_released_strategy_store(&root.join(RELEASED_STRATEGY_DATABASE));
        ensure_private_dir(&root.join("client-state/migrations/domain-state")).expect("marker dir");
        for (relative, content) in released_root_files() {
            let path = root.join(&relative);
            if relative == RELEASED_CONVERSATION_COMPLETION {
                fs::write(&path, content).expect("completion marker");
                continue;
            }
            let document: serde_json::Value =
                serde_json::from_str(&content).expect("released document");
            if let Some(parent) = path.parent() {
                ensure_private_dir(parent).expect("document directory");
            }
            atomic_write_private_text(&path, &document.to_string()).expect("document write");
        }
    }

    fn seed_released_conversation_store(root: &Path) {
        let database = root.join(RELEASED_CONVERSATION_DATABASE);
        fs::create_dir_all(database.parent().expect("database parent")).expect("directory");
        let connection = rusqlite::Connection::open(&database).expect("open released store");
        connection
            .execute_batch(RELEASED_CONVERSATION_SCHEMA)
            .expect("released conversation layout");
        connection
            .execute_batch(RELEASED_CONVERSATION_ROWS)
            .expect("released conversation rows");
    }

    fn seed_released_strategy_store(path: &Path) {
        fs::create_dir_all(path.parent().expect("database parent")).expect("directory");
        let connection = rusqlite::Connection::open(path).expect("open released store");
        connection
            .execute_batch(RELEASED_STRATEGY_SCHEMA)
            .expect("released strategy layout");
        connection
            .execute_batch(&released_strategy_rows())
            .expect("released strategy rows");
    }

    /// Seed the frozen released root plus additional user content.
    fn released_root(root: &Path) {
        assert_candidate_identity();
        seed_released_root(root);
        let notes = root.join("workspaces/demo/notes.md");
        fs::create_dir_all(notes.parent().expect("notes parent")).expect("notes directory");
        fs::write(notes, b"# synthetic workspace\n").expect("notes");
    }

    fn request(source: &Path, work: &Path, keep: bool) -> RehearsalRequest {
        RehearsalRequest {
            data_root: source.to_path_buf(),
            work_root: work.to_path_buf(),
            writers_stopped: true,
            keep_work_root: keep,
        }
    }

    #[test]
    fn the_fingerprint_follows_a_rewrite_of_the_root_it_describes() {
        let base = scratch("fingerprint");
        fs::write(base.join("one.txt"), b"one").expect("file");
        let before = fingerprint(&base);
        fs::write(base.join("one.txt"), b"one longer").expect("rewrite");
        assert_ne!(
            before,
            fingerprint(&base),
            "a rewritten file moves the digest"
        );
        assert_eq!(
            fingerprint(&base).entries,
            before.entries,
            "the entry count follows the tree, not the contents"
        );
    }

    #[test]
    fn the_fingerprint_excludes_only_the_ephemeral_admission_lock() {
        let base = scratch("fingerprint-admission-lock");
        fs::write(base.join("document.json"), b"{}").expect("application file");
        let admission_lock = base.join(ADMISSION_LOCK_PATH);
        fs::create_dir_all(admission_lock.parent().expect("lock parent")).expect("lock directory");
        let before = fingerprint(&base);
        fs::write(admission_lock, b"").expect("admission lock");
        assert_eq!(before, fingerprint(&base));

        fs::write(base.join("application.lock"), b"state").expect("application state");
        assert_ne!(before, fingerprint(&base), "other files remain payload");
    }

    #[test]
    fn a_released_root_is_converted_and_both_containers_round_trip() {
        let base = scratch("complete");
        let source = base.join("source");
        fs::create_dir_all(&source).expect("source");
        released_root(&source);
        let before = fingerprint(&source);

        let report = rehearse(&request(&source, &base.join("work"), true)).expect("rehearsal runs");

        assert_eq!(
            report.stages.len(),
            STAGES.len(),
            "every stage is named on every run: {:?}",
            report.stages
        );
        assert!(
            report.not_run.is_empty(),
            "no stage is silently skipped: {:?}",
            report.not_run
        );
        assert!(
            report
                .converted
                .iter()
                .any(|domain| domain == "adaptive-flywheel"),
            "the released strategy store moved to the current format: {:?}",
            report.converted
        );
        assert!(
            report.still_owed.is_empty(),
            "the owner accounted for every declared domain: {:?}",
            report.still_owed
        );
        assert!(
            report.recovery_complete,
            "the rehearsal observed both round trips: {report:?}"
        );
        assert_eq!(report.status, "complete");
        assert_eq!(before, fingerprint(&source), "the source is unchanged");
        assert_eq!(
            report.source_fingerprint, before,
            "the report states the fingerprint it read before it wrote anything"
        );

        // Every stage that claims a root names one that exists, and the fingerprint it
        // reports is the only statement about that root the report makes: a stage whose root
        // is absent fails its own oracle in `tests/rehearse.rs`, which reads the roots.
        for stage in &report.stages {
            let Some(root) = stage.root.as_deref() else {
                continue;
            };
            let root = Path::new(root);
            assert!(root.exists(), "{} left its root on disk", stage.stage);
            assert!(
                stage.root_fingerprint.is_some(),
                "{} reports the root it produced",
                stage.stage
            );
        }
    }

    #[test]
    fn a_source_that_declares_the_target_frontier_is_refused_before_anything_is_written() {
        let base = scratch("at-target");
        let source = base.join("source");
        fs::create_dir_all(&source).expect("source");
        released_root(&source);
        // Bring the source to this binary's own target frontier through the owner, which is
        // the format this binary writes and not the format the last published release left.
        licoup_native::domain::client_state_migration::admit(&source).expect("first admission");
        let before = fingerprint(&source);
        let work = base.join("work");

        let error = rehearse(&request(&source, &work, true)).expect_err("refused");
        assert_eq!(error, SOURCE_ROOT_ALREADY_AT_TARGET);
        assert_eq!(error.code(), "source_root_already_at_target");
        assert!(
            !work.exists(),
            "a refused rehearsal creates no working root"
        );
        assert_eq!(before, fingerprint(&source), "the source is unchanged");
    }

    #[test]
    fn a_missing_source_is_refused_and_writes_nothing() {
        let base = scratch("missing");
        let work = base.join("work");
        let error = rehearse(&request(&base.join("absent"), &work, true)).expect_err("refused");
        assert_eq!(error, SOURCE_ROOT_MISSING);
        assert!(!work.exists());
        assert!(fingerprint(&base).entries <= 1, "nothing was created");
    }

    #[test]
    fn a_source_without_a_released_shape_is_refused() {
        let base = scratch("empty");
        let source = base.join("source");
        fs::create_dir_all(&source).expect("source");
        let work = base.join("work");

        let error = rehearse(&request(&source, &work, true)).expect_err("refused");
        assert_eq!(error, SOURCE_ROOT_SHAPE_ABSENT);
        assert!(!work.exists());
    }

    #[test]
    fn a_run_without_the_stopped_writer_statement_writes_nothing() {
        let base = scratch("unconfirmed");
        let source = base.join("source");
        fs::create_dir_all(&source).expect("source");
        released_root(&source);
        let before = fingerprint(&source);
        let work = base.join("work");
        let mut unconfirmed = request(&source, &work, true);
        unconfirmed.writers_stopped = false;

        let error = rehearse(&unconfirmed).expect_err("refused");
        assert_eq!(error, WRITERS_RUNNING);
        assert!(!work.exists());
        assert_eq!(before, fingerprint(&source));
    }

    #[test]
    fn a_working_root_inside_the_source_is_refused() {
        let base = scratch("inside");
        let source = base.join("source");
        fs::create_dir_all(&source).expect("source");
        released_root(&source);
        let work = source.join("work");

        let error = rehearse(&request(&source, &work, true)).expect_err("refused");
        assert_eq!(error, WORK_ROOT_INSIDE_DATA_ROOT);
        assert!(!work.exists(), "the source is not written into");
    }

    #[test]
    fn a_stage_root_removed_after_the_run_is_never_claimed_by_the_report() {
        let base = scratch("removed");
        let source = base.join("source");
        fs::create_dir_all(&source).expect("source");
        released_root(&source);

        let report =
            rehearse(&request(&source, &base.join("work"), false)).expect("rehearsal runs");
        assert!(!report.work_root_retained);
        assert!(
            !Path::new(&report.work_root).exists(),
            "the disposable working root is removed when the caller does not keep it"
        );
        // The report still names what it observed; a caller that needs the roots asks for
        // them to be kept, and the oracle then reads them instead of the report.
        assert!(report.not_run.is_empty());
    }

    #[test]
    fn the_report_never_claims_a_stage_it_did_not_run() {
        let base = scratch("claimed");
        let source = base.join("source");
        fs::create_dir_all(&source).expect("source");
        released_root(&source);

        let report = rehearse(&request(&source, &base.join("work"), true)).expect("rehearsal runs");
        // Every named stage is accounted for exactly once, and the not-run list is exactly
        // the stages whose outcome says they did not run: a report can never claim a stage
        // that is missing from it, and can never omit one that did not run.
        for stage in STAGES {
            let matching = report
                .stages
                .iter()
                .filter(|result| result.stage == *stage)
                .count();
            assert_eq!(matching, 1, "{stage} appears exactly once in the report");
        }
        let declared: Vec<&str> = report
            .stages
            .iter()
            .filter(|result| result.outcome == StageOutcome::NotRun)
            .map(|result| result.stage)
            .collect();
        assert_eq!(declared, report.not_run);
        for stage in &report.stages {
            // A claimed stage carries its own root, and that root is on disk.
            if stage.outcome == StageOutcome::Observed {
                let root = stage
                    .root
                    .as_deref()
                    .expect("an observed stage names its root");
                assert!(
                    Path::new(root).exists(),
                    "{} claims to have observed {root}, which is absent",
                    stage.stage
                );
            }
        }
    }
}
