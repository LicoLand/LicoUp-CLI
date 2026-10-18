//! The bounded platform file owner the file stage removes through.
//!
//! Two facts make a file stage safe to restart: writers are not admitted into
//! the root while it runs, and every removal is confined to one root-relative
//! path that cannot escape it. This owner establishes both from the owners the
//! repository already has — the data-root admission lock the full-data-root
//! archive owner uses, and the bounded private-file primitives in
//! `licoup-foundation` — and never follows a symbolic link, never traverses an
//! external project reference, and never walks anything but the named entry.
//!
//! A completed removal is not reversible and this owner never claims it is. A
//! failure leaves that entry exactly where it was and reports it pending.

use anyhow::{Context, Result, anyhow, ensure};
use fs2::FileExt;
use std::fs::{self, File, OpenOptions};
use std::path::{Path, PathBuf};

use licoup_foundation::core::full_data_root_archive::ADMISSION_LOCK_PATH;
use licoup_foundation::platform::file_security::sync_directory;

use super::target::{
    CleanupInventoryEntry, CleanupInventoryKind, CleanupTarget, canonical_relative_posix,
};

/// What one removal observed.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FileStageOutcome {
    /// The entry was present and is now gone.
    Removed { bytes: u64 },
    /// The entry was already absent. A resumed stage reaches this state.
    AlreadyAbsent,
}

/// The exclusive right to write the root, held for the whole file stage.
///
/// The guard is the data-root admission lock the archive owner uses, taken the
/// same way: a root with no admission file has no admitted writer, and a root
/// whose admission file another process holds is refused, never signalled.
pub struct WriterQuiescence {
    backend: &'static str,
    admission_path: Option<PathBuf>,
    lock: Option<File>,
    exclusive: bool,
}

impl WriterQuiescence {
    fn unadmitted(admission_path: PathBuf) -> Self {
        Self {
            backend: "data-root-admission-absent",
            admission_path: Some(admission_path),
            lock: None,
            exclusive: true,
        }
    }

    fn held(admission_path: PathBuf, lock: File) -> Self {
        Self {
            backend: "data-root-admission-exclusive",
            admission_path: Some(admission_path),
            lock: Some(lock),
            exclusive: true,
        }
    }

    /// A quiescence a non-platform fixture owns. Only a fixture whose whole
    /// world is synthetic may claim exclusion this way.
    #[cfg(test)]
    pub(crate) fn fixture(backend: &'static str) -> Self {
        Self {
            backend,
            admission_path: None,
            lock: None,
            exclusive: true,
        }
    }

    pub fn backend(&self) -> &'static str {
        self.backend
    }

    /// Whether this guard is still the reason no writer can be admitted.
    pub fn holds_exclusion(&self) -> bool {
        self.exclusive
    }

    pub fn admission_path(&self) -> Option<&Path> {
        self.admission_path.as_deref()
    }
}

impl Drop for WriterQuiescence {
    fn drop(&mut self) {
        if let Some(lock) = self.lock.take() {
            let _ = FileExt::unlock(&lock);
        }
    }
}

impl std::fmt::Debug for WriterQuiescence {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("WriterQuiescence")
            .field("backend", &self.backend)
            .field("holdsExclusion", &self.holds_exclusion())
            .finish()
    }
}

/// What the terminal settlement removed.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct CleanupMaterialSettlement {
    pub removed_file_count: usize,
    pub removed_directory_count: usize,
    pub admission_lock_removed: bool,
}

impl CleanupMaterialSettlement {
    /// A settlement is terminal only once nothing it owns is left.
    pub fn is_terminal(self) -> bool {
        self.admission_lock_removed
    }
}

/// The effects the cleanup stage needs from the platform's file layer.
pub trait CleanupFileOwner: Send + Sync {
    fn backend(&self) -> &'static str;

    /// Close admission and prove no application writer holds the root.
    fn quiesce_writers(&self, target: &CleanupTarget) -> Result<WriterQuiescence>;

    /// Remove exactly one frozen entry. Never follows a link, never recurses
    /// beyond the named path, and never removes anything the inventory did not
    /// name.
    fn remove_owned_entry(&self, entry: &CleanupInventoryEntry) -> Result<FileStageOutcome>;

    /// Whether the named entry is absent now. Used to confirm a resumed stage
    /// instead of trusting the journal alone.
    fn observe_absent(&self, entry: &CleanupInventoryEntry) -> Result<bool>;
}

/// The production file owner for one LicoUp data root.
pub struct PrivateDataRootFileOwner {
    root: PathBuf,
}

impl PrivateDataRootFileOwner {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }
}

impl CleanupFileOwner for PrivateDataRootFileOwner {
    fn backend(&self) -> &'static str {
        "data-root-private-files"
    }

    fn quiesce_writers(&self, _target: &CleanupTarget) -> Result<WriterQuiescence> {
        ensure!(self.root.is_dir(), "cleanup_data_root_missing");
        let admission_path = self.root.join(ADMISSION_LOCK_PATH);
        match OpenOptions::new().read(true).open(&admission_path) {
            Ok(file) => {
                // The same lock the archive owner takes. A live writer is
                // refused; it is never signalled or stopped from here.
                match file.try_lock_exclusive() {
                    Ok(()) => Ok(WriterQuiescence::held(admission_path, file)),
                    Err(_) => Err(anyhow!("cleanup_writers_running")),
                }
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                Ok(WriterQuiescence::unadmitted(admission_path))
            }
            Err(_) => Err(anyhow!("cleanup_admission_unavailable")),
        }
    }

    fn remove_owned_entry(&self, entry: &CleanupInventoryEntry) -> Result<FileStageOutcome> {
        let path = self.resolve(entry.path())?;
        let metadata = match fs::symlink_metadata(&path) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Ok(FileStageOutcome::AlreadyAbsent);
            }
            Err(_) => return Err(anyhow!("cleanup_entry_unreadable")),
        };
        // A link is refused, never resolved: removing one must not remove
        // whatever it happens to point at, inside or outside this root.
        ensure!(
            !metadata.file_type().is_symlink(),
            "cleanup_entry_symlink_refused"
        );
        match entry.kind() {
            CleanupInventoryKind::File => {
                ensure!(
                    metadata.file_type().is_file(),
                    "cleanup_entry_kind_mismatch"
                );
                let bytes = metadata.len();
                fs::remove_file(&path).map_err(|_| anyhow!("cleanup_entry_removal_failed"))?;
                sync_directory(path.parent().unwrap_or(&self.root))?;
                Ok(FileStageOutcome::Removed { bytes })
            }
            CleanupInventoryKind::Directory => {
                ensure!(metadata.file_type().is_dir(), "cleanup_entry_kind_mismatch");
                // Only an empty directory is a consistent boundary. A
                // non-empty one means the frozen inventory did not account for
                // its contents, so this stage stops instead of guessing.
                fs::remove_dir(&path).map_err(|_| anyhow!("cleanup_directory_not_empty"))?;
                sync_directory(path.parent().unwrap_or(&self.root))?;
                Ok(FileStageOutcome::Removed { bytes: 0 })
            }
        }
    }

    fn observe_absent(&self, entry: &CleanupInventoryEntry) -> Result<bool> {
        let path = self.resolve(entry.path())?;
        match fs::symlink_metadata(&path) {
            Ok(_) => Ok(false),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(true),
            Err(_) => Err(anyhow!("cleanup_entry_unreadable")),
        }
    }
}

impl PrivateDataRootFileOwner {
    /// Resolve one frozen path under the root, refusing anything that is not a
    /// canonical relative POSIX path and re-checking the ancestor chain so a
    /// link swapped in after freezing cannot redirect the removal.
    fn resolve(&self, path: &str) -> Result<PathBuf> {
        ensure!(canonical_relative_posix(path), "cleanup_path_not_portable");
        let resolved = self.root.join(path);
        ensure!(
            resolved.starts_with(&self.root),
            "cleanup_path_escapes_root"
        );
        if let Some(parent) = resolved.parent() {
            licoup_foundation::platform::file_security::validate_private_path_ancestors(parent)
                .context("cleanup_path_ancestor_invalid")?;
        }
        Ok(resolved)
    }

    /// Remove this owner's remaining cleanup material: the progress journal and
    /// the state directory that holds it, then the root's admission lock.
    ///
    /// Only the terminal stage calls this. It refuses a symbolic link and a
    /// non-empty state directory rather than recursing, so a leftover payload
    /// is reported instead of silently erased.
    pub fn settle_cleanup_material(
        &self,
        state_directory: &Path,
    ) -> Result<CleanupMaterialSettlement> {
        let mut settlement = CleanupMaterialSettlement::default();
        remove_tree_without_links(state_directory, &mut settlement)?;
        let admission_path = self.root.join(ADMISSION_LOCK_PATH);
        match fs::remove_file(&admission_path) {
            Ok(()) => {
                settlement.admission_lock_removed = true;
                if let Some(parent) = admission_path.parent() {
                    sync_directory(parent)?;
                }
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                settlement.admission_lock_removed = true;
            }
            Err(_) => return Err(anyhow!("cleanup_admission_lock_removal_failed")),
        }
        if let Some(parent) = state_directory.parent() {
            sync_directory(parent)?;
        }
        Ok(settlement)
    }
}

fn remove_tree_without_links(
    path: &Path,
    settlement: &mut CleanupMaterialSettlement,
) -> Result<()> {
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(_) => return Err(anyhow!("cleanup_material_unreadable")),
    };
    ensure!(
        !metadata.file_type().is_symlink(),
        "cleanup_material_symlink_refused"
    );
    if metadata.file_type().is_file() {
        fs::remove_file(path).map_err(|_| anyhow!("cleanup_material_removal_failed"))?;
        settlement.removed_file_count += 1;
        return Ok(());
    }
    ensure!(
        metadata.file_type().is_dir(),
        "cleanup_material_unsupported_node"
    );
    let mut children: Vec<PathBuf> = Vec::new();
    for child in fs::read_dir(path).map_err(|_| anyhow!("cleanup_material_unreadable"))? {
        children.push(
            child
                .map_err(|_| anyhow!("cleanup_material_unreadable"))?
                .path(),
        );
    }
    children.sort();
    for child in children {
        remove_tree_without_links(&child, settlement)?;
    }
    fs::remove_dir(path).map_err(|_| anyhow!("cleanup_directory_not_empty"))?;
    settlement.removed_directory_count += 1;
    Ok(())
}
