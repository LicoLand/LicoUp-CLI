//! Coordinate one package-owned conversion across supported skipped releases.
//!
//! A conversion here is one operation with four owners, and this module is only
//! the join between them:
//!
//! * the **package store** says what is installed, and owns the only admission a
//!   payload gets: preflight bounds, the content digest, the manifest identity and
//!   the client-compatibility rule ([`PackageStore::install_local_import`]);
//! * the **package's own manifest** says which formats its converter owns, so the
//!   tool selects by declaration and never by a table of names or by the version of
//!   the installed client ([`crate::converter`]);
//! * the **host's own admission owner** says whether this host still owns
//!   unfinished local work, which is the question a maintenance operation must ask
//!   first ([`WorkAdmission`]);
//! * the **converter** performs the move, in a bounded native subprocess with the
//!   documented protocol ([`crate::converter_process`]).
//!
//! Three properties are what the run is built around:
//!
//! 1. **The source is only ever read.** The data root is copied into the caller's
//!    working root, the converter is given that copy, and the run verifies the
//!    data root's digest is unchanged afterwards. A converter that writes inside
//!    the source it was given is refused, not reported as converted.
//! 2. **A run is continued, never restarted.** The journal records each step
//!    before it is attempted. An unsettled run is resumed with the same declared
//!    package, pair, payload digest and source digest; `package-convert` refuses to
//!    start a second run over it at all, and the converter is told `--resume` so it
//!    continues the target it already has. Nothing deletes the target.
//! 3. **Completion is the converter's own report, checked.** The process exiting
//!    zero is not a conversion: the result document must be the documented one, name
//!    the required pair, say `complete` and leave a non-empty target. Anything else
//!    leaves the run unfinished and the exit status non-zero.

use crate::converter;
use crate::converter_process::{self, ConverterOutcome, ConverterRequest};
use crate::error::{
    CONVERTER_ENDPOINT_MISMATCH, CONVERTER_ENTRY_OUTSIDE_PACKAGE, CONVERTER_ENTRY_UNEXECUTABLE,
    CONVERTER_INCOMPLETE, CONVERTER_INVALID, CONVERTER_MISSING, CONVERTER_MODIFIED_SOURCE,
    CONVERTER_NOT_NATIVE, CONVERTER_RESULT_INCOMPLETE, CONVERTER_UNAVAILABLE, DATA_ROOT_MISSING,
    DATA_ROOT_NOT_DIRECTORY, MAINTENANCE_ADMISSION_CLOSED, MAINTENANCE_ADMISSION_UNAVAILABLE,
    MAINTENANCE_WORK_UNFINISHED, PACKAGE_CONVERSION_ABSENT, PACKAGE_CONVERSION_MISMATCHED,
    PACKAGE_CONVERSION_UNFINISHED, PACKAGE_IMPORT_REFUSED, PACKAGE_INDEX_CONVERTER_MISMATCH,
    PACKAGE_INDEX_ENTRY_MISSING, PACKAGE_PAYLOAD_INVALID, SOURCE_ROOT_UNREADABLE, ToolError,
    ToolResult, WORK_ROOT_INSIDE_DATA_ROOT, WRITERS_RUNNING,
};
use crate::inventory::{self, InventoryRequest, SelectedConverter};
use crate::package_journal::{
    self as journal, Journal, RUN_CONVERTER, RunPlan, STAGE_SOURCE, StepFact, StepStatus,
};
use licoup_extension_contracts::manifest::{FrozenEndpoints, conversion_code};
use licoup_native::domain::work_admission::{AdmissionBlocker, AdmissionDecision, WorkAdmission};
use licoup_native::platform::extension_packages::{
    ArtifactLimits, ExpandedPackage, PackageStore, TrustRecord, content_digest, digest_directory,
    preflight,
};
use serde::Serialize;
use std::path::{Path, PathBuf};

/// The staged copy of the source root inside the working root.
pub const SOURCE_DIRECTORY: &str = "source";
/// The target root the converter produces inside the working root.
pub const TARGET_DIRECTORY: &str = "target";
/// The converter's result document inside the working root.
pub const RESULT_FILE: &str = "result.json";
/// Where an offline payload is expanded for its own declaration before it is imported.
const IMPORT_DIRECTORY: &str = "import";

/// How the caller asked for one package conversion.
pub struct PackageConversionRequest<'a> {
    /// The installed data root: read, copied, and verified unchanged.
    pub data_root: &'a Path,
    /// The caller's durable working root for this run.
    pub work_root: &'a Path,
    /// The managed root of the package store.
    pub package_store: &'a Path,
    /// One package identity the caller selected, when it named one.
    pub package: Option<&'a str>,
    /// An offline payload to import before selecting, when the caller has one.
    pub payload: Option<&'a Path>,
    /// A signed release index to verify candidates and payloads against.
    pub index: Option<&'a Path>,
    /// The public key catalogue the index is verified with.
    pub index_public_keys: Option<&'a Path>,
    /// The operator's statement that every writer is stopped against the data root.
    pub writers_stopped: bool,
    /// Whether this invocation continues the run the working root records.
    pub resume: bool,
    /// Asked while the converter runs; a true answer stops the process group.
    pub stop: Option<&'a dyn Fn() -> bool>,
}

/// One step, as the report renders it.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StepReport {
    pub step: String,
    /// `pending`, `running` or `committed`.
    pub status: &'static str,
    pub attempt: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pending_reason: Option<String>,
}

/// What one coordinated conversion concluded.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ConversionReport {
    /// `converted`, `partial` or `alreadyCurrent`.
    pub status: &'static str,
    /// Whether this invocation continued a run instead of starting one.
    pub resumed: bool,
    pub package_id: String,
    pub package_version: String,
    pub trust_channel: String,
    pub entry: String,
    pub source_format: String,
    pub target_format: String,
    /// Whether a verified signed index admitted the payload.
    pub index_verified: bool,
    /// How many converter invocations the run has attempted.
    pub attempts: u32,
    /// Whether the staged copy of the source was reused instead of staged again.
    pub staged_source_reused: bool,
    /// Whether the data root was byte-identical after the run.
    pub source_untouched: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub target_bytes: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub converted_records: Option<u64>,
    pub steps: Vec<StepReport>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub converter: Option<ConverterOutcome>,
}

impl ConversionReport {
    /// Whether the run reached the result it was asked for.
    pub fn is_complete(&self) -> bool {
        matches!(self.status, "converted" | "alreadyCurrent")
    }
}

/// Coordinate one package conversion over the installed data root.
pub fn convert(request: &PackageConversionRequest<'_>) -> ToolResult<ConversionReport> {
    if !request.data_root.exists() {
        return Err(DATA_ROOT_MISSING);
    }
    if !request.data_root.is_dir() {
        return Err(DATA_ROOT_NOT_DIRECTORY);
    }
    // The statement is checked before anything is opened or moved, exactly as the
    // client-owner conversion checks it: the tool's own lock cannot stop a writer
    // that never heard of it.
    if !request.writers_stopped {
        return Err(WRITERS_RUNNING);
    }
    let data_root = request
        .data_root
        .canonicalize()
        .map_err(|_| DATA_ROOT_MISSING)?;

    // Installed state may only change while this host owns no unfinished work, and
    // the decision is read before this run creates anything at all: a refused
    // conversion leaves no staging directory, no journal and no target behind.
    match admission(data_root.as_path())?.decision {
        AdmissionDecision::Idle => {}
        AdmissionDecision::Blocked => return Err(MAINTENANCE_WORK_UNFINISHED),
        AdmissionDecision::Closed => return Err(MAINTENANCE_ADMISSION_CLOSED),
    }
    let work_root = prepare_work_root(request.work_root, &data_root)?;

    let required = converter::required_conversion()?;
    if let Some(payload) = request.payload {
        import_payload(request, &work_root, payload, &required)?;
    }
    let (_, selected) = inventory::inventory(
        &InventoryRequest {
            package_store: request.package_store,
            index: request.index,
            index_public_keys: request.index_public_keys,
            requested_package: request.package,
        },
        &required,
    )?;
    let selected = selected.ok_or(CONVERTER_UNAVAILABLE)?;
    let entry = resolve_entry(&selected)?;

    let declared_source_digest = source_digest(&data_root)?;
    let plan = RunPlan {
        package_id: selected.package_id.clone(),
        package_version: selected.package_version.clone(),
        entry: selected.entry.clone(),
        source_format: required.source_format().to_string(),
        target_format: required.target_format().to_string(),
        source_digest: declared_source_digest.clone(),
        record_digest: selected.record_digest.clone(),
        index_verified: request.index.is_some(),
    };

    let existing = journal::open(&work_root)?;
    let (mut record, resumed) = match existing {
        Some(record) => {
            if !record.matches(&plan) {
                return Err(PACKAGE_CONVERSION_MISMATCHED);
            }
            if record.is_complete() {
                // A settled run is the evidence of the finished conversion, so a
                // repeated attempt answers from the record instead of running a
                // second conversion over a target that is already the result.
                return Ok(ConversionReport {
                    status: "alreadyCurrent",
                    ..report_of(&record, &selected, false, None, false)
                });
            }
            if !request.resume {
                return Err(PACKAGE_CONVERSION_UNFINISHED);
            }
            (record, true)
        }
        None => {
            if request.resume {
                return Err(PACKAGE_CONVERSION_ABSENT);
            }
            (journal::initialize(&work_root, &plan)?, false)
        }
    };

    // Step one: the source copy. A settled step with an intact copy is reused, so a
    // resume does not stage again; anything else is staged afresh, and the digest of
    // the staged copy is what the record keeps.
    let staged = work_root.join(SOURCE_DIRECTORY);
    let mut staged_source_reused = false;
    if record.settled(STAGE_SOURCE)
        && staged.is_dir()
        && digest_directory(&staged)
            .map(|(digest, _)| Some(digest) == record.staged_source_digest)
            .unwrap_or(false)
    {
        staged_source_reused = true;
    } else {
        journal::mark_running(&work_root, STAGE_SOURCE)?;
        record = stage_source(&data_root, &staged, &work_root)?;
    }

    // Step two: the converter, over the staged copy and into the target.
    let target = work_root.join(TARGET_DIRECTORY);
    std::fs::create_dir_all(&target).map_err(|_| SOURCE_ROOT_UNREADABLE)?;
    let result_path = work_root.join(RESULT_FILE);
    if result_path.exists() {
        // A previous attempt's document must not be read as this attempt's report.
        let _ = std::fs::remove_file(&result_path);
    }
    let resume_converter = record.converter_attempted();
    journal::mark_running(&work_root, RUN_CONVERTER)?;
    let outcome = converter_process::run(&ConverterRequest {
        entry: &entry,
        source: &staged,
        target: &target,
        source_format: &plan.source_format,
        target_format: &plan.target_format,
        result: &result_path,
        resume: resume_converter,
        stop: request.stop,
    })?;

    let source_untouched = source_digest(&data_root)? == declared_source_digest;
    if !source_untouched {
        journal::mark_pending(&work_root, RUN_CONVERTER, CONVERTER_MODIFIED_SOURCE.code())?;
        return Err(CONVERTER_MODIFIED_SOURCE);
    }
    // The staged copy is the source the converter was given, and the protocol says the
    // converter reads it. A copy it wrote into is re-staged by the next attempt, and
    // this run is refused rather than recorded as a conversion.
    let staged_intact = record
        .staged_source_digest
        .as_deref()
        .is_some_and(|digest| {
            digest_directory(&staged)
                .map(|(current, _)| current == digest)
                .unwrap_or(false)
        });
    if !staged_intact {
        journal::mark_pending(&work_root, STAGE_SOURCE, CONVERTER_MODIFIED_SOURCE.code())?;
        journal::mark_pending(&work_root, RUN_CONVERTER, CONVERTER_MODIFIED_SOURCE.code())?;
        return Err(CONVERTER_MODIFIED_SOURCE);
    }

    let mut converted = false;
    if outcome.settled() {
        let (target_digest, target_bytes) =
            digest_directory(&target).map_err(|_| SOURCE_ROOT_UNREADABLE)?;
        if target_bytes == 0 {
            // A converter that reports a complete conversion and produces nothing
            // has not converted anything, whatever it wrote in its document.
            journal::mark_pending(
                &work_root,
                RUN_CONVERTER,
                CONVERTER_RESULT_INCOMPLETE.code(),
            )?;
        } else {
            record = journal::mark_committed(
                &work_root,
                RUN_CONVERTER,
                StepFact::Target {
                    digest: target_digest,
                    bytes: target_bytes,
                    converted_records: outcome
                        .result
                        .as_ref()
                        .and_then(|result| result.converted_records),
                },
            )?;
            converted = record.settled(RUN_CONVERTER);
        }
    } else {
        journal::mark_pending(
            &work_root,
            RUN_CONVERTER,
            outcome
                .reason
                .as_deref()
                .unwrap_or(CONVERTER_RESULT_INCOMPLETE.code()),
        )?;
        record = journal::open(&work_root)?.unwrap_or(record);
    }

    if converted && record.steps.values().all(|entry| entry.status.is_settled()) {
        record = journal::finish(&work_root)?;
    } else {
        record = journal::open(&work_root)?.unwrap_or(record);
    }

    let mut report = report_of(
        &record,
        &selected,
        resumed,
        Some(outcome),
        staged_source_reused,
    );
    report.status = if record.is_complete() {
        "converted"
    } else {
        "partial"
    };
    Ok(report)
}

/// The report projection of one record.
fn report_of(
    record: &Journal,
    selected: &SelectedConverter,
    resumed: bool,
    converter: Option<ConverterOutcome>,
    staged_source_reused: bool,
) -> ConversionReport {
    ConversionReport {
        status: if record.is_complete() {
            "converted"
        } else {
            "partial"
        },
        resumed,
        package_id: record.package_id.clone(),
        package_version: record.package_version.clone(),
        trust_channel: selected.trust_channel.clone(),
        entry: record.entry.clone(),
        source_format: record.source_format.clone(),
        target_format: record.target_format.clone(),
        index_verified: record.index_verified,
        attempts: record.attempts,
        staged_source_reused,
        source_untouched: true,
        target_bytes: record.target_bytes,
        converted_records: record.converted_records,
        steps: record
            .steps
            .iter()
            .map(|(step, entry)| StepReport {
                step: step.clone(),
                status: match entry.status {
                    StepStatus::Pending => "pending",
                    StepStatus::Running => "running",
                    StepStatus::Committed => "committed",
                },
                attempt: entry.attempt,
                pending_reason: entry.pending_reason.clone(),
            })
            .collect(),
        converter,
    }
}

/// Resolve the declared entry inside the installed package payload.
fn resolve_entry(selected: &SelectedConverter) -> ToolResult<PathBuf> {
    let root = selected
        .installed_root
        .canonicalize()
        .map_err(|_| CONVERTER_ENTRY_UNEXECUTABLE)?;
    let entry = root
        .join(&selected.entry)
        .canonicalize()
        .map_err(|_| CONVERTER_ENTRY_UNEXECUTABLE)?;
    if !entry.starts_with(&root) || !entry.is_file() {
        return Err(CONVERTER_ENTRY_UNEXECUTABLE);
    }
    // A native executable the payload publishes must be one this host can run. The
    // release tooling already refuses a payload whose converter has no executable
    // bit; a store that lost it is refused here rather than failing as a spawn.
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(&entry)
            .map_err(|_| CONVERTER_ENTRY_UNEXECUTABLE)?
            .permissions()
            .mode();
        if mode & 0o111 == 0 {
            return Err(CONVERTER_ENTRY_UNEXECUTABLE);
        }
    }
    Ok(entry)
}

/// Create (or adopt) the caller's working root, refusing one inside the source.
fn prepare_work_root(work_root: &Path, data_root: &Path) -> ToolResult<PathBuf> {
    std::fs::create_dir_all(work_root).map_err(|_| SOURCE_ROOT_UNREADABLE)?;
    let work_root = work_root
        .canonicalize()
        .map_err(|_| SOURCE_ROOT_UNREADABLE)?;
    if work_root.starts_with(data_root) {
        return Err(WORK_ROOT_INSIDE_DATA_ROOT);
    }
    Ok(work_root)
}

/// Stage one copy of the source root and record its digest.
fn stage_source(data_root: &Path, staged: &Path, work_root: &Path) -> ToolResult<Journal> {
    if staged.exists() {
        // A copy left by an interrupted attempt is never trusted: it is this run's
        // own staging directory, so it is cleared and written whole.
        std::fs::remove_dir_all(staged).map_err(|_| SOURCE_ROOT_UNREADABLE)?;
    }
    copy_tree(data_root, staged)?;
    let (digest, _) = digest_directory(staged).map_err(|_| SOURCE_ROOT_UNREADABLE)?;
    journal::mark_committed(work_root, STAGE_SOURCE, StepFact::StagedSource { digest })
}

/// Copy one directory tree, refusing symbolic links.
///
/// A link inside the source is not this run's to follow: following it would copy
/// something outside the root that was named, and refusing it names the fact.
fn copy_tree(source: &Path, destination: &Path) -> ToolResult<()> {
    std::fs::create_dir_all(destination).map_err(|_| SOURCE_ROOT_UNREADABLE)?;
    let entries = std::fs::read_dir(source).map_err(|_| SOURCE_ROOT_UNREADABLE)?;
    for entry in entries {
        let entry = entry.map_err(|_| SOURCE_ROOT_UNREADABLE)?;
        let path = entry.path();
        let metadata = std::fs::symlink_metadata(&path).map_err(|_| SOURCE_ROOT_UNREADABLE)?;
        if metadata.file_type().is_symlink() {
            return Err(SOURCE_ROOT_UNREADABLE);
        }
        let target = destination.join(entry.file_name());
        if metadata.is_dir() {
            copy_tree(&path, &target)?;
        } else if metadata.is_file() {
            std::fs::copy(&path, &target).map_err(|_| SOURCE_ROOT_UNREADABLE)?;
        }
    }
    Ok(())
}

/// The digest of one root, as this tool reports it.
fn source_digest(root: &Path) -> ToolResult<String> {
    digest_directory(root)
        .map(|(digest, _)| digest)
        .map_err(|_| SOURCE_ROOT_UNREADABLE)
}

/// Import one offline payload through the store's own admission, when asked.
///
/// The payload is the caller's already-downloaded converter. It is verified with
/// the store's own bounds and digest, checked against the signed index when one was
/// supplied, and published by [`PackageStore::install_local_import`] — the store's
/// existing import path, not a second installer. Nothing here reaches a network and
/// nothing here reads the version of the installed client.
fn import_payload(
    request: &PackageConversionRequest<'_>,
    work_root: &Path,
    payload: &Path,
    required: &FrozenEndpoints,
) -> ToolResult<()> {
    let metadata = std::fs::metadata(payload).map_err(|_| PACKAGE_PAYLOAD_INVALID)?;
    let limits = ArtifactLimits::default();
    if !metadata.is_file() || metadata.len() == 0 || metadata.len() > limits.max_archive_bytes {
        return Err(PACKAGE_PAYLOAD_INVALID);
    }
    let bytes = std::fs::read(payload).map_err(|_| PACKAGE_PAYLOAD_INVALID)?;
    // The store's own preflight and digest decide what these bytes are.
    preflight(&bytes, &limits).map_err(|_| PACKAGE_PAYLOAD_INVALID)?;
    let digest = content_digest(&bytes);

    let staging = work_root.join(IMPORT_DIRECTORY).join("staging");
    if staging.exists() {
        let _ = std::fs::remove_dir_all(&staging);
    }
    let expanded = ExpandedPackage::expand(&bytes, &staging, &limits).map_err(payload_refusal)?;
    let manifest = expanded.manifest().clone();
    let package_id = manifest.id.clone();
    let package_version = manifest.version.clone();
    if request
        .package
        .is_some_and(|requested| requested != package_id)
    {
        let _ = std::fs::remove_dir_all(work_root.join(IMPORT_DIRECTORY));
        return Err(PACKAGE_PAYLOAD_INVALID);
    }

    let trust = match request.index {
        Some(index_path) => {
            let index = inventory::verified_index(index_path, request.index_public_keys)?;
            let entry = index
                .package(&package_id)
                .ok_or(PACKAGE_INDEX_ENTRY_MISSING)?;
            if entry.package_version != package_version {
                let _ = std::fs::remove_dir_all(work_root.join(IMPORT_DIRECTORY));
                return Err(PACKAGE_INDEX_ENTRY_MISSING);
            }
            entry
                .reconcile(&manifest, required)
                .map_err(|_| PACKAGE_INDEX_CONVERTER_MISMATCH)?;
            entry
                .verify_payload(&bytes)
                .map_err(|_| PACKAGE_PAYLOAD_INVALID)?;
            TrustRecord::publisher_verified(
                entry.payload.sha256.clone(),
                manifest.permissions.clone(),
            )
            .map_err(|_| PACKAGE_IMPORT_REFUSED)?
        }
        None => TrustRecord::local_approved(digest.clone(), manifest.permissions.clone())
            .map_err(|_| PACKAGE_IMPORT_REFUSED)?,
    };

    if !request.package_store.is_dir() {
        let _ = std::fs::remove_dir_all(work_root.join(IMPORT_DIRECTORY));
        return Err(PACKAGE_IMPORT_REFUSED);
    }
    let store = PackageStore::open(request.package_store).map_err(|_| PACKAGE_IMPORT_REFUSED)?;
    let existing = store
        .installed_version(&package_id, &package_version)
        .map_err(|_| PACKAGE_IMPORT_REFUSED)?;
    match existing {
        // The same payload is already installed: the import is satisfied, and a
        // record of different bytes is refused instead of replaced silently.
        Some(installed) if installed.digest == trust.digest() => {}
        Some(_) => {
            let _ = std::fs::remove_dir_all(work_root.join(IMPORT_DIRECTORY));
            return Err(PACKAGE_PAYLOAD_INVALID);
        }
        None => {
            store
                .install_local_import(&package_id, &package_version, trust, &bytes)
                .map_err(|_| PACKAGE_IMPORT_REFUSED)?;
        }
    }
    let _ = std::fs::remove_dir_all(work_root.join(IMPORT_DIRECTORY));
    Ok(())
}

/// The tool's code for a payload the store's own artifact admission refused.
///
/// The store validates the manifest before it publishes anything, so its refusal is
/// already the precise rule the payload broke. The conversion vocabulary is carried
/// through where the contract publishes one — an interpreter-carried converter is
/// `converter_not_native`, not "an invalid payload" — and anything else stays a
/// payload refusal.
fn payload_refusal(failure: licoup_extension_contracts::ApplicationFailure) -> ToolError {
    match failure.code.as_str() {
        conversion_code::MISSING => CONVERTER_MISSING,
        conversion_code::NOT_NATIVE => CONVERTER_NOT_NATIVE,
        conversion_code::ENTRY_OUTSIDE_PACKAGE => CONVERTER_ENTRY_OUTSIDE_PACKAGE,
        conversion_code::INCOMPLETE => CONVERTER_INCOMPLETE,
        conversion_code::INVALID => CONVERTER_INVALID,
        conversion_code::ENDPOINT_MISMATCH => CONVERTER_ENDPOINT_MISMATCH,
        _ => PACKAGE_PAYLOAD_INVALID,
    }
}

/// The host's own maintenance decision for one data root.
pub fn admission(data_root: &Path) -> ToolResult<licoup_native::domain::work_admission::Admission> {
    WorkAdmission::open(data_root)
        .admission()
        .map_err(|_| MAINTENANCE_ADMISSION_UNAVAILABLE)
}

/// One blocker owner, counted, as a report names it.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BlockerOwnerReport {
    pub owner: &'static str,
    pub kind: String,
    pub count: usize,
}

/// The maintenance decision a caller reports alongside its own result.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AdmissionReport {
    /// `idle`, `blocked` or `closed`.
    pub decision: &'static str,
    /// The owners whose unfinished work blocks maintenance, counted by kind.
    pub blockers: Vec<BlockerOwnerReport>,
    /// Whether more blockers exist than the bounded read reported.
    pub truncated: bool,
}

/// Read the maintenance decision as a report, for a verb that renders one.
///
/// A decision that cannot be read is reported as `unavailable` rather than as
/// idle: a caller that cannot tell must not read silence as permission.
pub fn admission_report(data_root: &Path) -> AdmissionReport {
    match admission(data_root) {
        Ok(admission) => AdmissionReport {
            decision: admission.decision.as_str(),
            blockers: count_blockers(&admission.blockers),
            truncated: admission.truncated,
        },
        Err(_) => AdmissionReport {
            decision: "unavailable",
            blockers: Vec::new(),
            truncated: false,
        },
    }
}

fn count_blockers(blockers: &[AdmissionBlocker]) -> Vec<BlockerOwnerReport> {
    let mut counted: Vec<BlockerOwnerReport> = Vec::new();
    for blocker in blockers {
        let owner = blocker.owner.as_str();
        match counted
            .iter_mut()
            .find(|entry| entry.owner == owner && entry.kind == blocker.kind)
        {
            Some(entry) => entry.count += 1,
            None => counted.push(BlockerOwnerReport {
                owner,
                kind: blocker.kind.clone(),
                count: 1,
            }),
        }
    }
    counted
}
