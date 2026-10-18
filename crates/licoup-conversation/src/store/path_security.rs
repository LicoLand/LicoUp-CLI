//! Minimal private-path policy owned by the durable Conversation crate.
//!
//! The store cannot depend on a host platform module. These operations keep
//! the same fail-closed invariants needed by SQLite and bundle export: reject
//! symlinks, require stable file/directory nodes, and apply owner-only modes on
//! Unix. Platform-specific stronger custody remains a host responsibility.

use anyhow::{Result, anyhow, ensure};
use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::path::{Component, Path, PathBuf};

pub(super) fn ensure_private_dir(path: &Path) -> Result<()> {
    validate_no_parent_traversal(path)?;
    if let Ok(metadata) = fs::symlink_metadata(path) {
        ensure!(
            metadata.file_type().is_dir() && !metadata.file_type().is_symlink(),
            "private path is not a stable directory"
        );
    }
    fs::create_dir_all(path)?;
    harden_private_path(path)
}

pub(super) fn harden_private_path(path: &Path) -> Result<()> {
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(_) => return Err(anyhow!("private path metadata is unavailable")),
    };
    ensure!(
        !metadata.file_type().is_symlink()
            && (metadata.file_type().is_dir() || metadata.file_type().is_file()),
        "private path is not a stable file or directory"
    );
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = if metadata.file_type().is_dir() {
            0o700
        } else {
            0o600
        };
        fs::set_permissions(path, fs::Permissions::from_mode(mode))?;
    }
    Ok(())
}

pub(super) fn validate_existing_private_file(path: &Path) -> Result<bool> {
    validate_no_parent_traversal(path)?;
    match fs::symlink_metadata(path) {
        Ok(metadata) => {
            ensure!(
                metadata.file_type().is_file() && !metadata.file_type().is_symlink(),
                "private path is not a stable file"
            );
            Ok(true)
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(false),
        Err(_) => Err(anyhow!("private path metadata is unavailable")),
    }
}

pub(super) fn validate_export_destination(path: &Path) -> Result<()> {
    validate_no_parent_traversal(path)?;
    let check = match fs::symlink_metadata(path) {
        Ok(metadata) => {
            ensure!(
                metadata.file_type().is_file() && !metadata.file_type().is_symlink(),
                "export destination must be a stable file"
            );
            path
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => path
            .parent()
            .ok_or_else(|| anyhow!("export destination has no parent directory"))?,
        Err(_) => return Err(anyhow!("export destination metadata is unavailable")),
    };
    let metadata = fs::symlink_metadata(check)
        .map_err(|_| anyhow!("export destination metadata is unavailable"))?;
    ensure!(
        !metadata.file_type().is_symlink(),
        "export destination must not be a symlink"
    );
    Ok(())
}

pub(super) fn atomic_write_private_text(path: &Path, content: &str) -> Result<()> {
    validate_export_destination(path)?;
    let parent = path
        .parent()
        .ok_or_else(|| anyhow!("export destination has no parent directory"))?;
    let temporary = temporary_sibling(path);
    let result = (|| -> Result<()> {
        let mut options = OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut file = options.open(&temporary)?;
        file.write_all(content.as_bytes())?;
        file.sync_all()?;
        drop(file);
        fs::rename(&temporary, path)?;
        harden_private_path(path)?;
        if let Ok(directory) = fs::File::open(parent) {
            let _ = directory.sync_all();
        }
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    result
}

fn temporary_sibling(path: &Path) -> PathBuf {
    let name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("conversation-export");
    path.with_file_name(format!(".{name}.{}.tmp", uuid::Uuid::new_v4()))
}

fn validate_no_parent_traversal(path: &Path) -> Result<()> {
    ensure!(
        !path
            .components()
            .any(|component| component == Component::ParentDir),
        "path contains a parent traversal component"
    );
    Ok(())
}
