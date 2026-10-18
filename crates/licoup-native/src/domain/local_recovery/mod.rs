//! Native composition for complete local recovery through the archive owner.
//!
//! [`export_data_home`] and [`import_archive`] are the reusable native composition a
//! caller uses instead of marshalling a second restore policy: both the client CLI
//! and the standalone migration tool. The archive owner in `licoup-foundation` owns
//! the container, manifest, extraction and publication; this module owns only the two
//! steps that need the application's own owners:
//!
//! * a restore into another home applies the existing data-home reference rewrite,
//!   which rewrites owner-managed references and never touches user message content
//!   or unrelated project paths; and
//! * restored workflow revisions are re-frozen and read back through the workflow
//!   package owner.
//!
//! Writer coordination is [`acquire_exclusive_selected_home`], which wraps the existing
//! data-home access owner. The caller holds the returned lease for the capture or
//! import and refuses, rather than reading a live root, when a writer is already
//! active.

use anyhow::{Result, anyhow};
use licoup_foundation::core::full_data_root_archive::{
    ExportOutcome, ExportRequest, RestoreOutcome, RestoreRequest, export_data_root,
    restore_data_root_with_preparation,
};
use licoup_foundation::platform::data_home_access::{
    DataHomeRelocationLease, try_acquire_data_home_relocation_lease,
};
use licoup_foundation::platform::paths;
use std::path::{Path, PathBuf};

/// The stable refusal when another process still uses the selected data home.
pub const WRITERS_RUNNING: &str = "backup_writers_running";

/// The stable refusal when failed publication cannot be completely rolled back.
///
/// The source archive is untouched. The retained destination is unverified and must
/// not be activated; retry from the archive into a fresh empty home.
pub const RECOVERY_CLEANUP_FAILED: &str = "recovery_target_cleanup_failed";

/// One completed import, including the owner checks performed before publication.
#[derive(Clone, Debug)]
pub struct RecoveryImport {
    pub outcome: RestoreOutcome,
    /// The restored home differed from the captured logical source home, so the
    /// data-home reference rewrite was applied.
    pub relocated: bool,
    /// Revision digests re-frozen and read back through the workflow package owner.
    pub verified_workflow_revisions: Vec<String>,
}

/// Resolve the logical data home for a recovery command.
///
/// An explicit root is normalized lexically against the process working directory; the
/// operator names it, so it is not resolved through links. Without one, the existing
/// authoritative resolver selects `LICOUP_HOME`, the published alias, the saved
/// selection and the platform default in that order.
pub fn resolve_data_home(explicit: Option<&Path>) -> Result<PathBuf> {
    match explicit {
        Some(path) => std::path::absolute(path).map_err(|_| anyhow!("data_root_unresolved")),
        None => Ok(paths::selected_data_home()?.path),
    }
}

/// Acquire the selected data home's exclusive relocation lease without waiting.
///
/// The command must run in a process that holds no shared lease. `Ok(None)` means
/// another process is using the data home; the operation must then be refused with
/// [`WRITERS_RUNNING`] instead of reading a live root, and no writer is stopped.
pub fn acquire_exclusive_selected_home() -> Result<Option<DataHomeRelocationLease>> {
    try_acquire_data_home_relocation_lease()
}

/// Export the complete data home into one plaintext archive.
pub fn export_data_home(
    data_root: Option<&Path>,
    archive_path: &Path,
    writers_stopped: bool,
) -> Result<ExportOutcome> {
    let data_root = resolve_data_home(data_root)?;
    let archive_path =
        std::path::absolute(archive_path).map_err(|_| anyhow!("backup_archive_unresolved"))?;
    export_data_root(&ExportRequest {
        data_root,
        archive_path,
        writers_stopped,
    })
}

/// Import one archive into an empty target home.
///
/// The generic restore owns publication and its refusals: it validates the archive and
/// every declared member before anything is published, and publishes only into an empty
/// destination. Before publication this composition rebases owner-managed references when the
/// target differs from the captured logical source home, then re-establishes and verifies
/// the workflow revision protections through their owner.
///
/// Preparation operates only on Foundation's private verified staging payload. Foundation
/// revalidates its bounded structure and preserves owner-proven read-only protections at
/// publication. One owner handles checked scratch cleanup and publication rollback;
/// no second native importer or cleanup implementation is retained. This does not activate
/// the recovered home or make interrupted multi-file publication an accepted recovery.
pub fn import_archive(archive_path: &Path, target_root: &Path) -> Result<RecoveryImport> {
    let archive_path =
        std::path::absolute(archive_path).map_err(|_| anyhow!("backup_archive_unresolved"))?;
    let target_root =
        std::path::absolute(target_root).map_err(|_| anyhow!("backup_target_root_unresolved"))?;
    let mut verified_workflow_revisions = Vec::new();
    let outcome = restore_data_root_with_preparation(
        &RestoreRequest {
            archive_path,
            target_root: target_root.clone(),
        },
        |staged_root, source_home, _resolved_target| {
            crate::domain::mobile_relay::prepare_recovered_custody_metadata(
                staged_root,
                source_home,
            )?;
            // The existing conversation-snapshot owner owns which references travel with
            // the home; it returns immediately when the source and target are the same.
            crate::domain::conversation::snapshots::relocate_copied_data_home_references(
                staged_root,
                source_home,
                &target_root,
            )?;
            verified_workflow_revisions =
                match crate::domain::workflow_runtime::StrategyPackageImporter::open_restored(
                    staged_root,
                )? {
                    Some(importer) => importer.restore_revision_invariants()?,
                    None => Vec::new(),
                };
            Ok(())
        },
    )
    .map_err(|error| {
        let message = error.to_string();
        if message.contains("archive_target_cleanup_")
            || message.contains("archive_target_rollback_incomplete")
        {
            error.context(RECOVERY_CLEANUP_FAILED)
        } else {
            error
        }
    })?;
    Ok(RecoveryImport {
        relocated: outcome.source_home != target_root,
        outcome,
        verified_workflow_revisions,
    })
}
