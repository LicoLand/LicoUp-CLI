use anyhow::{Result, anyhow, ensure};
use licoup_foundation::platform::file_security::{
    validate_export_destination, validate_path_owner, validate_private_path_ancestors,
};
use licoup_foundation::platform::paths::portable_data_dir;
use std::fs::{self, OpenOptions};
use std::io::{self, Read};
use std::path::{Path, PathBuf};

use super::policy;

pub fn portable_state_root() -> Result<PathBuf> {
    Ok(state_root_for_data_home(&portable_data_dir()?))
}

/// Project the owned layout without opening or initializing a data home.
pub fn state_root_for_data_home(root: &Path) -> PathBuf {
    root.join(policy::CLIENT_STATE_DIR)
}

pub fn portable_state_root_read_only() -> Result<PathBuf> {
    Ok(state_root_for_data_home(
        &licoup_foundation::platform::paths::portable_data_dir_read_only()?,
    ))
}

pub fn activity_path(root: &Path) -> PathBuf {
    root.join(policy::ACTIVITY_DIR).join(policy::ACTIVITY_FILE)
}

pub fn activity_root(root: &Path) -> PathBuf {
    root.join(policy::ACTIVITY_DIR)
}

pub fn snapshot_root(root: &Path) -> PathBuf {
    root.join(policy::SNAPSHOT_DIR)
}

pub fn collection_path(root: &Path, collection: &str) -> PathBuf {
    root.join(format!("{collection}.json"))
}

pub fn snapshot_path(root: &Path, snapshot_id: &str) -> Result<PathBuf> {
    ensure!(
        snapshot_id.starts_with("snapshot-")
            && !snapshot_id.is_empty()
            && snapshot_id.len() <= policy::MAX_SNAPSHOT_ID_BYTES
            && snapshot_id
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'_'),
        "snapshot identifier is invalid"
    );
    Ok(root.join(format!("{snapshot_id}.json")))
}

pub fn internal_state_reference(kind: &str, path: &Path) -> String {
    let leaf = path
        .file_name()
        .and_then(|value| value.to_str())
        .filter(|value| {
            !value.is_empty()
                && value
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || b"._-".contains(&byte))
        })
        .unwrap_or("private-entry");
    format!("{kind}/{leaf}")
}

pub fn redacted_local_path() -> &'static str {
    policy::REDACTED_LOCAL_PATH
}

pub fn read_owned_local_text_bounded(path: &Path, max_bytes: usize) -> Result<Option<String>> {
    validate_local_path(path)?;
    validate_private_path_ancestors(path)?;
    let path_metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(_) => return Err(anyhow!("local snapshot source metadata is unavailable")),
    };
    validate_owned_regular_file(&path_metadata)?;
    ensure!(
        path_metadata.len() <= max_bytes as u64,
        "local snapshot source exceeds its bounded size"
    );

    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(nix::libc::O_NOFOLLOW);
    }
    let mut file = options
        .open(path)
        .map_err(|_| anyhow!("local snapshot source could not be opened"))?;
    let opened_metadata = file.metadata()?;
    validate_owned_regular_file(&opened_metadata)?;
    ensure_same_file(&path_metadata, &opened_metadata)?;

    let mut content = Vec::with_capacity(opened_metadata.len() as usize);
    Read::by_ref(&mut file)
        .take((max_bytes as u64).saturating_add(1))
        .read_to_end(&mut content)?;
    ensure!(
        content.len() <= max_bytes,
        "local snapshot source exceeds its bounded size"
    );
    let stable_metadata = fs::symlink_metadata(path)
        .map_err(|_| anyhow!("local snapshot source changed while reading"))?;
    validate_owned_regular_file(&stable_metadata)?;
    ensure_same_file(&opened_metadata, &stable_metadata)?;
    String::from_utf8(content)
        .map(Some)
        .map_err(|_| anyhow!("local snapshot source is not UTF-8"))
}

pub fn validate_restore_destination(path: &Path) -> Result<()> {
    validate_local_path(path)?;
    validate_private_path_ancestors(path)?;
    validate_export_destination(path)
}

pub fn remove_owned_local_file_if_present(path: &Path) -> Result<bool> {
    validate_restore_destination(path)?;
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(false),
        Err(_) => return Err(anyhow!("local restore destination metadata is unavailable")),
    };
    validate_owned_regular_file(&metadata)?;
    fs::remove_file(path).map_err(|_| anyhow!("local restore destination could not be removed"))?;
    Ok(true)
}

fn validate_local_path(path: &Path) -> Result<()> {
    ensure!(
        path.is_absolute()
            && !path.as_os_str().is_empty()
            && path.as_os_str().len() <= policy::MAX_LOCAL_PATH_BYTES,
        "local snapshot path is invalid"
    );
    Ok(())
}

fn validate_owned_regular_file(metadata: &fs::Metadata) -> Result<()> {
    ensure!(
        metadata.file_type().is_file() && !metadata.file_type().is_symlink(),
        "local snapshot path is not a stable regular file"
    );
    validate_path_owner(metadata)
}

#[cfg(unix)]
fn ensure_same_file(left: &fs::Metadata, right: &fs::Metadata) -> Result<()> {
    use std::os::unix::fs::MetadataExt;
    ensure!(
        left.dev() == right.dev() && left.ino() == right.ino(),
        "local snapshot path was replaced"
    );
    Ok(())
}

#[cfg(not(unix))]
fn ensure_same_file(left: &fs::Metadata, right: &fs::Metadata) -> Result<()> {
    ensure!(
        left.file_type().is_file()
            && right.file_type().is_file()
            && left.len() == right.len()
            && left.created().ok() == right.created().ok(),
        "local snapshot path was replaced"
    );
    Ok(())
}
