//! Full data-root capture and restore.
//!
//! One plaintext archive carries the complete logical client data root in either a
//! standard ZIP or TAR.GZ container. Native owners decide what is coherent to
//! capture; the migration tool only marshals options and prints privacy-safe
//! results. No live database is copied directly, no opaque key handle is presented
//! as a portable secret, and a partial recovery is never reported as complete.
//!
//! Layout inside the archive:
//!
//! ```text
//! licoup-data-root.json          manifest: layout, container, inventory
//! data/<relative path>           one member per captured entry
//! ```
//!
//! The manifest is written before any member so a restore can validate the declared
//! inventory before publishing anything.

mod capture;
mod inventory;
mod restore;

pub use capture::{
    CREDENTIAL_DOMAIN, CREDENTIAL_INVENTORY_PATH, ExportOutcome, ExportRequest,
    archive_path_inside_data_root, export_data_root,
};
pub use inventory::{
    ARCHIVE_LAYOUT, ArchiveManifest, InventoryEntry, InventoryKind, RecoveryCoverage,
    RecoveryLimitation,
};
pub use restore::{
    RestoreOutcome, RestoreRequest, restore_data_root, restore_data_root_with_preparation,
};

/// Ephemeral writer-coordination state excluded from archive payload and logical
/// data-root fingerprints. Admission recreates it whenever a root is opened.
pub const ADMISSION_LOCK_PATH: &str = "client-state/migrations/admission.lock";

use anyhow::{Result, anyhow};
use std::path::Path;

/// Supported plaintext containers. The container is inferred from the file name.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ArchiveContainer {
    Zip,
    TarGz,
}

impl ArchiveContainer {
    pub const fn extension(self) -> &'static str {
        match self {
            Self::Zip => "zip",
            Self::TarGz => "tar.gz",
        }
    }

    /// Infer the container from an archive path. Development backups are never
    /// encrypted and never use another extension.
    pub fn from_path(path: &Path) -> Result<Self> {
        let name = path
            .file_name()
            .and_then(|name| name.to_str())
            .ok_or_else(|| anyhow!("archive_path_invalid"))?;
        let lowered = name.to_ascii_lowercase();
        if lowered.ends_with(".tar.gz") || lowered.ends_with(".tgz") {
            Ok(Self::TarGz)
        } else if lowered.ends_with(".zip") {
            Ok(Self::Zip)
        } else {
            Err(anyhow!("archive_container_unsupported"))
        }
    }
}

/// Manifest member name. Fixed, so a restore never has to guess.
pub(crate) const MANIFEST_MEMBER: &str = "licoup-data-root.json";
/// Member prefix that holds the captured data root.
pub(crate) const DATA_PREFIX: &str = "data/";
