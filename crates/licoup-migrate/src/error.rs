//! Typed failures.
//!
//! Every failure is a stable code with no local path and no stored value in it, so the
//! tool can be run on a machine whose paths never leave it.
//!
//! The code set is one value shared by every verb, so the crate keeps the whole
//! vocabulary live rather than pruning the constants one consumer happens not to use yet.

#![allow(dead_code)]

use std::fmt;

/// A failure the tool reports without exposing the machine it ran on.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ToolError {
    code: &'static str,
}

impl ToolError {
    pub const fn new(code: &'static str) -> Self {
        Self { code }
    }

    pub const fn code(&self) -> &'static str {
        self.code
    }
}

impl fmt::Display for ToolError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.code)
    }
}

impl std::error::Error for ToolError {}

pub type ToolResult<T> = Result<T, ToolError>;

/// The data root the caller named is missing.
pub const DATA_ROOT_MISSING: ToolError = ToolError::new("data_root_missing");
/// The data root is not a directory.
pub const DATA_ROOT_NOT_DIRECTORY: ToolError = ToolError::new("data_root_not_directory");
/// The client's own owners could not report the frontier.
pub const FRONTIER_UNAVAILABLE: ToolError = ToolError::new("migration_frontier_unavailable");
/// The client's own owners could not report the observed state.
pub const STATE_UNAVAILABLE: ToolError = ToolError::new("migration_state_unavailable");
/// The stored state is newer than this tool knows how to read.
pub const STATE_NEWER_THAN_TOOL: ToolError = ToolError::new("state_newer_than_binary");
/// The named target is not one the fixed endpoints offer.
pub const TARGET_UNSUPPORTED: ToolError = ToolError::new("migration_target_unsupported");
/// The operator has not stated that every writer is stopped.
pub const WRITERS_RUNNING: ToolError = ToolError::new("maintenance_confirmation_required");
/// A run marker exists but cannot be parsed.
pub const MARKER_INVALID: ToolError = ToolError::new("migration_marker_invalid");
/// A run marker exists but cannot be read.
pub const MARKER_UNREADABLE: ToolError = ToolError::new("migration_marker_unreadable");
/// A run marker could not be written.
pub const MARKER_UNWRITABLE: ToolError = ToolError::new("migration_marker_unwritable");
/// A resume was asked for a journal whose run does not match this binary's frontier.
pub const JOURNAL_MISMATCHED: ToolError = ToolError::new("migration_journal_mismatched");
/// A step the owner reported as committed is not supported by the observed state.
pub const COMMIT_UNSUPPORTED: ToolError = ToolError::new("migration_commit_unsupported");
/// A resume point could not be advanced, so no further step was taken.
pub const STOPPED: ToolError = ToolError::new("migration_stopped");
/// The client's own conversion owner refused the root.
pub const OWNER_REFUSED: ToolError = ToolError::new("migration_owner_refused");
/// A verb was reached without the data root it reads.
pub const DATA_ROOT_REQUIRED: ToolError = ToolError::new("data_root_required");
/// An archive verb was reached without the archive it reads or writes.
pub const ARCHIVE_REQUIRED: ToolError = ToolError::new("archive_required");
/// An import was reached without the empty destination it publishes into.
pub const TARGET_ROOT_REQUIRED: ToolError = ToolError::new("target_root_required");
/// A rehearsal was reached without the disposable working directory it stages in.
pub const WORK_ROOT_REQUIRED: ToolError = ToolError::new("work_root_required");
/// The archive an export was asked to write sits inside the root it would capture.
pub const ARCHIVE_INSIDE_DATA_ROOT: ToolError = ToolError::new("archive_path_inside_data_root");
/// The rehearsal's disposable working root sits inside the source it promised only to read.
pub const WORK_ROOT_INSIDE_DATA_ROOT: ToolError = ToolError::new("work_root_inside_data_root");
/// The named package manifest could not be read as a manifest document.
pub const CONVERTER_MANIFEST_UNREADABLE: ToolError =
    ToolError::new("converter_manifest_unreadable");
/// The manifest is not a valid package manifest, so no conversion was read from it.
pub const CONVERTER_MANIFEST_INVALID: ToolError = ToolError::new("converter_manifest_invalid");
/// The package declares no conversion, so this conversion has no owner here.
pub const CONVERTER_MISSING: ToolError = ToolError::new("converter_missing");
/// The declared converter is not a native executable.
pub const CONVERTER_NOT_NATIVE: ToolError = ToolError::new("converter_not_native");
/// The declared entry is not an entry inside the package payload.
pub const CONVERTER_ENTRY_OUTSIDE_PACKAGE: ToolError =
    ToolError::new("converter_entry_outside_package");
/// The declared entry is not present in the package payload.
pub const CONVERTER_ENTRY_MISSING: ToolError = ToolError::new("converter_entry_missing");
/// The declaration is present and incomplete.
pub const CONVERTER_INCOMPLETE: ToolError = ToolError::new("converter_incomplete");
/// The declaration is malformed or over a published bound.
pub const CONVERTER_INVALID: ToolError = ToolError::new("converter_invalid");
/// The declared formats are not the endpoints this tool requires.
pub const CONVERTER_ENDPOINT_MISMATCH: ToolError = ToolError::new("converter_endpoint_mismatch");
/// The package store the caller named could not be read as one.
pub const PACKAGE_STORE_UNAVAILABLE: ToolError = ToolError::new("package_store_unavailable");
/// The signed release index the caller supplied did not verify.
pub const PACKAGE_INDEX_INVALID: ToolError = ToolError::new("package_index_invalid");
/// The signed release index describes no entry for the package it was checked against.
pub const PACKAGE_INDEX_ENTRY_MISSING: ToolError = ToolError::new("package_index_entry_missing");
/// The signed release index and the package's own declaration disagree.
pub const PACKAGE_INDEX_CONVERTER_MISMATCH: ToolError =
    ToolError::new("package_index_converter_mismatch");
/// Payload bytes are not the bytes the signed index or the store record describes.
pub const PACKAGE_PAYLOAD_INVALID: ToolError = ToolError::new("package_payload_invalid");
/// The store refused to publish the payload the caller handed in.
pub const PACKAGE_IMPORT_REFUSED: ToolError = ToolError::new("package_import_refused");
/// No installed package declares a conversion for the required endpoints.
pub const CONVERTER_UNAVAILABLE: ToolError = ToolError::new("converter_unavailable");
/// The installed converter entry is not a runnable file inside the package payload.
pub const CONVERTER_ENTRY_UNEXECUTABLE: ToolError = ToolError::new("converter_entry_unexecutable");
/// The host still owns unfinished local work, so no maintenance may begin.
pub const MAINTENANCE_WORK_UNFINISHED: ToolError = ToolError::new("maintenance_work_unfinished");
/// Another maintenance operation holds this data root's close-admission barrier.
pub const MAINTENANCE_ADMISSION_CLOSED: ToolError = ToolError::new("maintenance_admission_closed");
/// The host's own admission owner could not report its decision.
pub const MAINTENANCE_ADMISSION_UNAVAILABLE: ToolError =
    ToolError::new("maintenance_admission_unavailable");
/// A conversion run is recorded here and has not settled; it is resumed, never restarted.
pub const PACKAGE_CONVERSION_UNFINISHED: ToolError =
    ToolError::new("package_conversion_unfinished");
/// No interrupted package conversion is recorded in the named working root.
pub const PACKAGE_CONVERSION_ABSENT: ToolError = ToolError::new("package_conversion_absent");
/// The recorded run declares another package, another pair or another source root.
pub const PACKAGE_CONVERSION_MISMATCHED: ToolError =
    ToolError::new("package_conversion_mismatched");
/// The source root is not the one the recorded run staged.
pub const PACKAGE_CONVERSION_SOURCE_CHANGED: ToolError =
    ToolError::new("package_conversion_source_changed");
/// The converter could not be started, or exited without producing a target.
pub const CONVERTER_RUN_FAILED: ToolError = ToolError::new("converter_run_failed");
/// The converter's result document is missing or not the documented document.
pub const CONVERTER_RESULT_INVALID: ToolError = ToolError::new("converter_result_invalid");
/// The converter ran and reported that the conversion is not complete.
pub const CONVERTER_RESULT_INCOMPLETE: ToolError = ToolError::new("converter_result_incomplete");
/// The converter wrote inside the source root it promised only to read.
pub const CONVERTER_MODIFIED_SOURCE: ToolError = ToolError::new("converter_modified_source");
/// A conversion the tool owns was interrupted before it settled.
pub const PACKAGE_CONVERSION_STOPPED: ToolError = ToolError::new("package_conversion_stopped");
/// The source root the run stages from could not be read as one directory tree.
pub const SOURCE_ROOT_UNREADABLE: ToolError = ToolError::new("source_root_unreadable");

/// A failure that names the marker it read, never the path it read it from.
pub fn marker_read_failed(_path: &std::path::Path) -> ToolError {
    MARKER_UNREADABLE
}

/// A marker document that could not be parsed.
pub const fn marker_invalid(_what: &'static str) -> ToolError {
    MARKER_INVALID
}

/// A marker that could not be written.
pub const fn marker_unwritable() -> ToolError {
    MARKER_UNWRITABLE
}
