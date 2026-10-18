use super::super::package::{SelectedPayloadFile, write_selected_payload_tree};
use anyhow::{Result, anyhow, ensure};
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use uuid::Uuid;

#[cfg(unix)]
use std::ffi::CString;
#[cfg(unix)]
use std::os::fd::{AsRawFd, FromRawFd};
#[cfg(unix)]
use std::os::unix::ffi::OsStrExt;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum CommitKind {
    Directory,
    File,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct CommitUnit {
    pub(super) staging: PathBuf,
    pub(super) destination: PathBuf,
    pub(super) kind: CommitKind,
}

#[derive(Clone, Copy, Debug, Default)]
pub(super) struct CommitOptions {
    #[cfg(test)]
    pub(super) fail_after_commits: Option<usize>,
    #[cfg(test)]
    pub(super) replace_destination_before_commit: Option<usize>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) struct CommitOutcome {
    pub(super) cleanup_pending: bool,
}

pub(super) fn stage_payload(
    files: &[SelectedPayloadFile],
    destination: &Path,
) -> Result<CommitUnit> {
    let parent = destination
        .parent()
        .ok_or_else(|| anyhow!("collaboration_workflow_destination_parent_missing"))?;
    let staging = parent.join(format!(".licoup-stage-{}", Uuid::new_v4()));
    write_selected_payload_tree(files, &staging)?;
    Ok(CommitUnit {
        staging: staging.clone(),
        destination: destination.to_path_buf(),
        kind: CommitKind::Directory,
    })
}

pub(super) fn stage_private_registration(content: &str, destination: &Path) -> Result<CommitUnit> {
    let parent = destination
        .parent()
        .ok_or_else(|| anyhow!("collaboration_workflow_destination_parent_missing"))?;
    let staging = parent.join(format!(".licoup-stage-{}", Uuid::new_v4()));
    create_private_staging_file(&staging, content.as_bytes())?;
    Ok(CommitUnit {
        staging,
        destination: destination.to_path_buf(),
        kind: CommitKind::File,
    })
}

pub(super) fn cleanup_staged(units: &[CommitUnit]) {
    for unit in units {
        remove_staging(unit);
    }
}

pub(super) fn commit_directory_no_replace(source: &Path, destination: &Path) -> Result<()> {
    let unit = CommitUnit {
        staging: source.to_path_buf(),
        destination: destination.to_path_buf(),
        kind: CommitKind::Directory,
    };
    validate_commit_unit(&unit)?;
    rename_no_replace(source, destination, CommitKind::Directory)
}

pub(super) fn commit_file_no_replace(source: &Path, destination: &Path) -> Result<()> {
    let unit = CommitUnit {
        staging: source.to_path_buf(),
        destination: destination.to_path_buf(),
        kind: CommitKind::File,
    };
    validate_commit_unit(&unit)?;
    rename_no_replace(source, destination, CommitKind::File)
}

pub(super) fn commit_all(units: &[CommitUnit], _options: CommitOptions) -> Result<CommitOutcome> {
    let mut committed = Vec::new();
    for (index, unit) in units.iter().enumerate() {
        #[cfg(test)]
        if _options.fail_after_commits == Some(index) {
            rollback(units, &committed)?;
            return Err(anyhow!("collaboration_workflow_test_commit_failure"));
        }
        #[cfg(test)]
        if _options.replace_destination_before_commit == Some(index) {
            match unit.kind {
                CommitKind::Directory => {
                    fs::create_dir(&unit.destination)?;
                    fs::write(unit.destination.join("sentinel"), b"preserve")?;
                }
                CommitKind::File => fs::write(&unit.destination, b"preserve")?,
            }
        }
        if let Err(error) = validate_commit_unit(unit)
            .and_then(|()| rename_no_replace(&unit.staging, &unit.destination, unit.kind))
        {
            rollback(units, &committed)?;
            return Err(error);
        }
        committed.push(index);
    }

    Ok(CommitOutcome {
        cleanup_pending: false,
    })
}

fn validate_commit_unit(unit: &CommitUnit) -> Result<()> {
    #[cfg(unix)]
    drop(open_parent_no_follow(&unit.destination)?);
    #[cfg(not(unix))]
    licoup_foundation::platform::file_security::validate_no_symlink_ancestors(&unit.destination)?;
    licoup_foundation::platform::file_security::validate_export_destination(&unit.destination)?;
    ensure!(
        fs::symlink_metadata(&unit.destination).is_err(),
        "collaboration_workflow_destination_must_be_new"
    );
    let staging_metadata = fs::symlink_metadata(&unit.staging)
        .map_err(|_| anyhow!("collaboration_workflow_staging_missing"))?;
    ensure!(
        !staging_metadata.file_type().is_symlink()
            && match unit.kind {
                CommitKind::Directory => staging_metadata.is_dir(),
                CommitKind::File => staging_metadata.is_file(),
            },
        "collaboration_workflow_staging_invalid"
    );
    Ok(())
}

fn rollback(units: &[CommitUnit], committed: &[usize]) -> Result<()> {
    let mut rollback_failed = false;
    for index in committed.iter().rev().copied() {
        let unit = &units[index];
        if rename_no_replace(&unit.destination, &unit.staging, unit.kind).is_err() {
            rollback_failed = true;
        }
    }
    cleanup_staged(units);
    ensure!(
        !rollback_failed,
        "collaboration_workflow_commit_rollback_failed"
    );
    Ok(())
}

fn remove_staging(unit: &CommitUnit) {
    match unit.kind {
        CommitKind::Directory => {
            let _ = fs::remove_dir_all(&unit.staging);
        }
        CommitKind::File => {
            let _ = fs::remove_file(&unit.staging);
        }
    }
}

#[cfg(unix)]
fn create_private_staging_file(path: &Path, content: &[u8]) -> Result<()> {
    let parent = open_parent_no_follow(path)?;
    let leaf = component_cstring(
        path.file_name()
            .ok_or_else(|| anyhow!("collaboration_workflow_staging_path_invalid"))?,
    )?;
    let flags = nix::libc::O_WRONLY
        | nix::libc::O_CREAT
        | nix::libc::O_EXCL
        | nix::libc::O_NOFOLLOW
        | nix::libc::O_CLOEXEC;
    // SAFETY: `parent` owns a live directory descriptor, `leaf` is a validated
    // NUL-free component, and the return value is checked before ownership.
    let fd = unsafe { nix::libc::openat(parent.as_raw_fd(), leaf.as_ptr(), flags, 0o600) };
    ensure!(fd >= 0, "collaboration_workflow_staging_create_failed");
    // SAFETY: the successful `openat` returned a new descriptor owned only here.
    let mut file = unsafe { fs::File::from_raw_fd(fd) };
    file.write_all(content)?;
    file.sync_all()?;
    ensure!(
        file.metadata()?.len() == content.len() as u64,
        "collaboration_workflow_staging_verification_failed"
    );
    parent.sync_all()?;
    Ok(())
}

#[cfg(not(unix))]
fn create_private_staging_file(path: &Path, content: &[u8]) -> Result<()> {
    licoup_foundation::platform::file_security::validate_no_symlink_ancestors(path)?;
    let mut options = fs::OpenOptions::new();
    options.create_new(true).write(true);
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        const FILE_FLAG_OPEN_REPARSE_POINT: u32 = 0x0020_0000;
        options.custom_flags(FILE_FLAG_OPEN_REPARSE_POINT);
    }
    let mut file = options
        .open(path)
        .map_err(|_| anyhow!("collaboration_workflow_staging_create_failed"))?;
    file.write_all(content)?;
    file.sync_all()?;
    licoup_foundation::platform::file_security::harden_private_path(path)?;
    let metadata = fs::symlink_metadata(path)?;
    ensure!(
        metadata.is_file()
            && !metadata.file_type().is_symlink()
            && metadata.len() == content.len() as u64,
        "collaboration_workflow_staging_verification_failed"
    );
    Ok(())
}

#[cfg(unix)]
fn rename_no_replace(source: &Path, destination: &Path, kind: CommitKind) -> Result<()> {
    let source_parent = open_parent_no_follow(source)?;
    let destination_parent = open_parent_no_follow(destination)?;
    let source_leaf = component_cstring(
        source
            .file_name()
            .ok_or_else(|| anyhow!("collaboration_workflow_commit_path_invalid"))?,
    )?;
    let destination_leaf = component_cstring(
        destination
            .file_name()
            .ok_or_else(|| anyhow!("collaboration_workflow_commit_path_invalid"))?,
    )?;
    validate_source_at(&source_parent, &source_leaf, kind)?;
    ensure_destination_missing_at(&destination_parent, &destination_leaf)?;

    #[cfg(any(target_os = "linux", target_os = "android"))]
    // SAFETY: both directory descriptors and component C strings stay alive for
    // the syscall, and `RENAME_NOREPLACE` makes the commit fail closed.
    let renamed = unsafe {
        nix::libc::syscall(
            nix::libc::SYS_renameat2,
            source_parent.as_raw_fd(),
            source_leaf.as_ptr(),
            destination_parent.as_raw_fd(),
            destination_leaf.as_ptr(),
            nix::libc::RENAME_NOREPLACE,
        )
    } == 0;

    #[cfg(any(target_os = "macos", target_os = "ios"))]
    // SAFETY: both directory descriptors and component C strings stay alive for
    // the call, and `RENAME_EXCL` prevents destination replacement.
    let renamed = unsafe {
        const RENAME_EXCL: u32 = 0x0000_0004;
        nix::libc::renameatx_np(
            source_parent.as_raw_fd(),
            source_leaf.as_ptr(),
            destination_parent.as_raw_fd(),
            destination_leaf.as_ptr(),
            RENAME_EXCL,
        )
    } == 0;

    #[cfg(all(
        unix,
        not(any(
            target_os = "linux",
            target_os = "android",
            target_os = "macos",
            target_os = "ios"
        ))
    ))]
    let renamed = false;

    ensure!(renamed, "collaboration_workflow_commit_no_replace_failed");
    destination_parent.sync_all()?;
    Ok(())
}

#[cfg(not(unix))]
fn rename_no_replace(source: &Path, destination: &Path, kind: CommitKind) -> Result<()> {
    licoup_foundation::platform::file_security::validate_no_symlink_ancestors(source)?;
    licoup_foundation::platform::file_security::validate_no_symlink_ancestors(destination)?;
    ensure!(
        fs::symlink_metadata(destination).is_err(),
        "collaboration_workflow_destination_must_be_new"
    );
    let metadata = fs::symlink_metadata(source)?;
    ensure!(
        !metadata.file_type().is_symlink()
            && match kind {
                CommitKind::Directory => metadata.is_dir(),
                CommitKind::File => metadata.is_file(),
            },
        "collaboration_workflow_staging_invalid"
    );
    fs::rename(source, destination)
        .map_err(|_| anyhow!("collaboration_workflow_commit_no_replace_failed"))?;
    Ok(())
}

#[cfg(unix)]
fn open_parent_no_follow(path: &Path) -> Result<fs::File> {
    let parent = path
        .parent()
        .ok_or_else(|| anyhow!("collaboration_workflow_destination_parent_missing"))?;
    super::super::package::open_directory_path_no_follow(parent)
}

#[cfg(unix)]
fn component_cstring(name: &std::ffi::OsStr) -> Result<CString> {
    CString::new(name.as_bytes())
        .map_err(|_| anyhow!("collaboration_workflow_destination_encoding_invalid"))
}

#[cfg(unix)]
fn validate_source_at(parent: &fs::File, leaf: &CString, kind: CommitKind) -> Result<()> {
    let flags = match kind {
        CommitKind::Directory => {
            nix::libc::O_RDONLY
                | nix::libc::O_DIRECTORY
                | nix::libc::O_NOFOLLOW
                | nix::libc::O_CLOEXEC
        }
        CommitKind::File => nix::libc::O_RDONLY | nix::libc::O_NOFOLLOW | nix::libc::O_CLOEXEC,
    };
    // SAFETY: the live directory descriptor and validated component remain valid
    // for `openat`; the result is checked before conversion.
    let fd = unsafe { nix::libc::openat(parent.as_raw_fd(), leaf.as_ptr(), flags) };
    ensure!(fd >= 0, "collaboration_workflow_staging_changed");
    // SAFETY: the successful `openat` returned a new descriptor owned only here.
    drop(unsafe { fs::File::from_raw_fd(fd) });
    Ok(())
}

#[cfg(unix)]
fn ensure_destination_missing_at(parent: &fs::File, leaf: &CString) -> Result<()> {
    let mut stat = std::mem::MaybeUninit::<nix::libc::stat>::uninit();
    // SAFETY: `stat` points to writable storage and the live directory descriptor
    // plus validated component remain valid for the duration of `fstatat`.
    let result = unsafe {
        nix::libc::fstatat(
            parent.as_raw_fd(),
            leaf.as_ptr(),
            stat.as_mut_ptr(),
            nix::libc::AT_SYMLINK_NOFOLLOW,
        )
    };
    if result == 0 {
        return Err(anyhow!("collaboration_workflow_destination_must_be_new"));
    }
    ensure!(
        std::io::Error::last_os_error().kind() == std::io::ErrorKind::NotFound,
        "collaboration_workflow_destination_state_unavailable"
    );
    Ok(())
}
