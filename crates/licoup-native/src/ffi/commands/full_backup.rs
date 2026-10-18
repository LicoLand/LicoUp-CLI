//! Public backup commands.
//!
//! `backup export` and `backup import` marshal the selected data home, the archive
//! path and the caller's writer statement into the native recovery composition. The
//! command layer never copies a live database itself and never invents a key: the
//! archive owner captures and publishes, and the native composition rewrites
//! owner-managed references and re-freezes restored workflow revisions. The command
//! holds the selected data home's exclusive lease for its duration, so an active
//! writer is refused with a typed error instead of being stopped or captured.

use super::{AdmittedCommand, CliExecution};
use anyhow::{Result, anyhow, ensure};
use serde_json::json;
use std::path::PathBuf;

pub(super) fn handle_backup_export(command: AdmittedCommand) -> Result<CliExecution> {
    let archive = absolute_path(
        command.required_text("archive"),
        "backup_archive_unresolved",
    )?;
    let requested_root = command
        .option_text("data-root")
        .map(|value| absolute_path(value, "backup_data_root_unresolved"))
        .transpose()?;
    let _selected_home = exclusive_selected_home()?;
    let outcome = crate::domain::local_recovery::export_data_home(
        requested_root.as_deref(),
        &archive,
        command.option_flag("writers-stopped"),
    )?;
    Ok(CliExecution::Json(json!({
        "status": "exported",
        "container": outcome.container.extension(),
        "sourceHome": outcome.source_home.display().to_string(),
        "coverage": outcome.coverage,
        "limitations": outcome.limitations,
        "fileCount": outcome.file_count,
        "totalBytes": outcome.total_bytes,
    })))
}

pub(super) fn handle_backup_import(command: AdmittedCommand) -> Result<CliExecution> {
    let archive = absolute_path(
        command.required_text("archive"),
        "backup_archive_unresolved",
    )?;
    let target_root = command
        .option_text("target-root")
        .ok_or_else(|| anyhow!("backup_target_root_required"))?;
    let target_root = absolute_path(target_root, "backup_target_root_unresolved")?;
    let _selected_home = exclusive_selected_home()?;
    let imported = crate::domain::local_recovery::import_archive(&archive, &target_root)?;
    let outcome = imported.outcome;
    Ok(CliExecution::Json(json!({
        "status": "imported",
        "container": outcome.container.extension(),
        "sourceHome": outcome.source_home.display().to_string(),
        "relocated": imported.relocated,
        "verifiedWorkflowRevisions": imported.verified_workflow_revisions,
        "coverage": outcome.coverage,
        "limitations": outcome.limitations,
        "fileCount": outcome.file_count,
        "totalBytes": outcome.total_bytes,
    })))
}

/// Hold the selected data home's exclusive lease, or refuse the operation.
///
/// The refusal is the owner's stable code and leaves every writer running: a live root
/// is never copied, and no process is asked to stop.
fn exclusive_selected_home()
-> Result<licoup_foundation::platform::data_home_access::DataHomeRelocationLease> {
    crate::domain::local_recovery::acquire_exclusive_selected_home()?
        .ok_or_else(|| anyhow!(crate::domain::local_recovery::WRITERS_RUNNING))
}

/// The absolute form of one admitted path: a relative name the caller supplied is read
/// against the process working directory, exactly as the composition will read it.
fn absolute_path(raw: &str, unresolved: &'static str) -> Result<PathBuf> {
    ensure!(!raw.trim().is_empty(), "{unresolved}");
    std::path::absolute(raw).map_err(|_| anyhow!(unresolved))
}
