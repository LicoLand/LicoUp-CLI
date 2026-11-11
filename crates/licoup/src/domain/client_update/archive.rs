use std::{
    fs,
    io::Read,
    path::{Component, Path, PathBuf},
};

use anyhow::{Context, Result, bail, ensure};
use flate2::read::GzDecoder;
use tar::Archive as TarArchive;

use super::{model::VerifiedUpdateSelection, tree::validate_tree_with_contained_links};

const MAX_ARCHIVE_ENTRIES: usize = 100_000;
const MAX_EXPANDED_BYTES: u64 = 8 * 1024 * 1024 * 1024;

pub(super) fn extract_signed_archive(
    selection: &VerifiedUpdateSelection,
    staged_path: &Path,
    extraction_root: &Path,
) -> Result<()> {
    let file = fs::File::open(staged_path).context("failed to open signed update archive")?;
    let expanded_limit = selection
        .artifact
        .size
        .checked_mul(64)
        .unwrap_or(MAX_EXPANDED_BYTES)
        .min(MAX_EXPANDED_BYTES);
    let file_name = selection.artifact.file_name.as_str();
    if file_name.ends_with(".tar.gz") {
        extract_tar(GzDecoder::new(file), extraction_root, expanded_limit)?;
    } else if file_name.ends_with(".tar") {
        extract_tar(file, extraction_root, expanded_limit)?;
    } else if file_name.ends_with(".zip") {
        extract_zip(file, extraction_root, expanded_limit)?;
    } else {
        bail!("client update artifact must be a signed tar, tar.gz or zip archive");
    }
    validate_tree_with_contained_links(extraction_root)
}

fn extract_tar<R: Read>(reader: R, extraction_root: &Path, expanded_limit: u64) -> Result<()> {
    let mut archive = TarArchive::new(reader);
    let mut entry_count = 0_usize;
    let mut expanded_bytes = 0_u64;
    for entry in archive
        .entries()
        .context("failed to read signed update archive")?
    {
        let mut entry = entry.context("failed to read signed update archive entry")?;
        entry_count = entry_count
            .checked_add(1)
            .context("client update archive entry count overflow")?;
        ensure!(
            entry_count <= MAX_ARCHIVE_ENTRIES,
            "client update archive contains too many entries"
        );
        let entry_type = entry.header().entry_type();
        ensure!(
            entry_type.is_file() || entry_type.is_dir(),
            "client update archive links and special entries are forbidden"
        );
        let entry_path = entry
            .path()
            .context("client update archive path is invalid")?;
        validate_archive_path(&entry_path)?;
        expanded_bytes = expanded_bytes
            .checked_add(entry.header().size()?)
            .context("client update archive expanded size overflow")?;
        ensure!(
            expanded_bytes <= expanded_limit,
            "client update archive exceeds its expansion limit"
        );
        ensure!(
            entry.unpack_in(extraction_root)?,
            "client update archive entry escapes its extraction root"
        );
    }
    Ok(())
}

fn extract_zip(file: fs::File, extraction_root: &Path, expanded_limit: u64) -> Result<()> {
    let mut archive =
        zip::ZipArchive::new(file).context("failed to open signed update zip archive")?;
    let mut entry_count = 0_usize;
    let mut expanded_bytes = 0_u64;
    let mut links = Vec::new();
    for index in 0..archive.len() {
        let mut entry = archive
            .by_index(index)
            .context("failed to read signed update zip archive entry")?;
        entry_count = entry_count
            .checked_add(1)
            .context("client update archive entry count overflow")?;
        ensure!(
            entry_count <= MAX_ARCHIVE_ENTRIES,
            "client update archive contains too many entries"
        );
        let unix_type = entry.unix_mode().unwrap_or(0) & 0o170000;
        let is_symlink = unix_type == 0o120000;
        ensure!(
            entry.is_file() || entry.is_dir() || is_symlink,
            "client update archive special entries are forbidden"
        );
        let entry_path = entry
            .enclosed_name()
            .context("client update archive entry path is invalid")?;
        validate_archive_path(&entry_path)?;
        expanded_bytes = expanded_bytes
            .checked_add(entry.size())
            .context("client update archive expanded size overflow")?;
        ensure!(
            expanded_bytes <= expanded_limit,
            "client update archive exceeds its expansion limit"
        );
        let destination = extraction_root.join(&entry_path);
        if entry.is_dir() {
            fs::create_dir_all(&destination)
                .context("failed to create client update archive directory")?;
            continue;
        }
        if is_symlink {
            ensure!(
                entry.size() <= 1024,
                "client update archive link target is too long"
            );
            let mut target = String::new();
            entry
                .read_to_string(&mut target)
                .context("client update archive link target is invalid")?;
            validate_link_target(Path::new(&target))?;
            links.push((destination, PathBuf::from(target)));
            continue;
        }
        let parent = destination
            .parent()
            .context("client update archive destination is invalid")?;
        fs::create_dir_all(parent).context("failed to create client update archive directory")?;
        let mut output = fs::File::create(&destination)
            .context("failed to create client update archive file")?;
        let written = std::io::copy(&mut entry, &mut output)
            .context("failed to extract client update archive file")?;
        ensure!(
            written == entry.size(),
            "client update archive entry size does not match its declared size"
        );
    }
    create_contained_links(extraction_root, links)?;
    Ok(())
}

fn validate_link_target(target: &Path) -> Result<()> {
    ensure!(
        !target.as_os_str().is_empty()
            && target.as_os_str().len() <= 1024
            && target
                .components()
                .all(|component| matches!(component, Component::Normal(_))),
        "client update archive link target must be relative and normalized"
    );
    Ok(())
}

#[cfg(unix)]
fn create_contained_links(
    extraction_root: &Path,
    mut pending: Vec<(PathBuf, PathBuf)>,
) -> Result<()> {
    use std::os::unix::fs::symlink;

    let canonical_root = fs::canonicalize(extraction_root)
        .context("failed to canonicalize client update extraction root")?;
    while !pending.is_empty() {
        let mut deferred = Vec::new();
        let mut created = 0_usize;
        for (destination, target) in pending {
            let parent = destination
                .parent()
                .context("client update archive link destination is invalid")?;
            let canonical_parent = match fs::canonicalize(parent) {
                Ok(value) => value,
                Err(_) => {
                    deferred.push((destination, target));
                    continue;
                }
            };
            ensure!(
                canonical_parent.starts_with(&canonical_root),
                "client update archive link destination escapes its extraction root"
            );
            let resolved_target = match fs::canonicalize(canonical_parent.join(&target)) {
                Ok(value) => value,
                Err(_) => {
                    deferred.push((destination, target));
                    continue;
                }
            };
            ensure!(
                resolved_target.starts_with(&canonical_root),
                "client update archive link target escapes its extraction root"
            );
            ensure!(
                !destination.exists(),
                "client update archive link destination already exists"
            );
            symlink(&target, &destination)
                .context("failed to create client update archive link")?;
            created += 1;
        }
        ensure!(
            created > 0,
            "client update archive contains a dangling or cyclic link"
        );
        pending = deferred;
    }
    Ok(())
}

#[cfg(not(unix))]
fn create_contained_links(_extraction_root: &Path, links: Vec<(PathBuf, PathBuf)>) -> Result<()> {
    ensure!(
        links.is_empty(),
        "client update archive links are unsupported on this platform"
    );
    Ok(())
}

fn validate_archive_path(path: &Path) -> Result<()> {
    ensure!(
        path.as_os_str().len() <= 1024
            && path
                .components()
                .all(|component| matches!(component, Component::Normal(_))),
        "client update archive path must be relative and normalized"
    );
    Ok(())
}

#[cfg(test)]
pub(in crate::domain::client_update) fn validate_archive_path_for_test(path: &Path) -> Result<()> {
    validate_archive_path(path)
}
