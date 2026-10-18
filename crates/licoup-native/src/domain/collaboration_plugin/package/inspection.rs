use super::super::manifest::{MANIFEST_FILE, parse_manifest};
use super::descriptor::{LOCAL_DEPLOYMENT_SCHEMA, MCP_INSTALL_SCHEMA, validate_descriptor};
use super::{InspectedPackage, PackageFile};
use anyhow::{Result, anyhow, ensure};
use sha2::{Digest, Sha256};
use std::fs;
use std::path::{Component, Path};

const MAX_PACKAGE_FILES: usize = 512;
const MAX_PACKAGE_BYTES: usize = 32 * 1024 * 1024;
const MAX_PACKAGE_DEPTH: usize = 16;

pub(in crate::domain::collaboration_plugin) fn inspect_package(
    root: &Path,
) -> Result<InspectedPackage> {
    let metadata = fs::symlink_metadata(root)
        .map_err(|_| anyhow!("collaboration_plugin_package_unavailable"))?;
    ensure!(
        metadata.file_type().is_dir() && !metadata.file_type().is_symlink(),
        "collaboration_plugin_package_invalid"
    );

    let mut files = Vec::new();
    let mut total_bytes = 0usize;
    collect_files(root, root, 0, &mut files, &mut total_bytes)?;
    files.sort_by(|left, right| left.relative_path.cmp(&right.relative_path));
    ensure!(
        !files.is_empty() && files.len() <= MAX_PACKAGE_FILES,
        "collaboration_plugin_package_file_count_invalid"
    );
    ensure!(
        total_bytes <= MAX_PACKAGE_BYTES,
        "collaboration_plugin_package_too_large"
    );

    let manifest_bytes = package_file(&files, Path::new(MANIFEST_FILE))?;
    let manifest = parse_manifest(manifest_bytes)?;
    ensure!(
        super::inventory::signed_inventory_digest(&files)?
            == manifest.signed_package_inventory_digest_sha256,
        "collaboration_plugin_signed_inventory_digest_mismatch"
    );
    super::runner::validate_server_runner_files(&manifest, &files)?;
    validate_descriptor(
        package_file(&files, &manifest.local_deployment_descriptor)?,
        LOCAL_DEPLOYMENT_SCHEMA,
        &files,
    )?;
    validate_descriptor(
        package_file(&files, &manifest.mcp_install_descriptor)?,
        MCP_INSTALL_SCHEMA,
        &files,
    )?;

    let mut hasher = Sha256::new();
    for file in &files {
        let relative = normalized_relative_text(&file.relative_path)?;
        hasher.update((relative.len() as u64).to_be_bytes());
        hasher.update(relative.as_bytes());
        hasher.update((file.bytes.len() as u64).to_be_bytes());
        hasher.update(&file.bytes);
    }
    Ok(InspectedPackage {
        manifest,
        digest_sha256: format!("{:x}", hasher.finalize()),
        file_count: files.len(),
        total_bytes,
        files,
    })
}

fn collect_files(
    root: &Path,
    directory: &Path,
    depth: usize,
    files: &mut Vec<PackageFile>,
    total_bytes: &mut usize,
) -> Result<()> {
    ensure!(
        depth <= MAX_PACKAGE_DEPTH,
        "collaboration_plugin_package_depth_exceeded"
    );
    let mut entries = fs::read_dir(directory)
        .map_err(|_| anyhow!("collaboration_plugin_package_read_failed"))?
        .collect::<std::io::Result<Vec<_>>>()?;
    entries.sort_by_key(|entry| entry.file_name());
    for entry in entries {
        if entry.file_name() == ".git" {
            continue;
        }
        let path = entry.path();
        let metadata = fs::symlink_metadata(&path)
            .map_err(|_| anyhow!("collaboration_plugin_package_entry_unavailable"))?;
        ensure!(
            !metadata.file_type().is_symlink(),
            "collaboration_plugin_package_symlink_rejected"
        );
        if metadata.file_type().is_dir() {
            collect_files(root, &path, depth + 1, files, total_bytes)?;
            continue;
        }
        ensure!(
            metadata.file_type().is_file(),
            "collaboration_plugin_package_entry_type_rejected"
        );
        ensure!(
            files.len() < MAX_PACKAGE_FILES,
            "collaboration_plugin_package_file_count_invalid"
        );
        ensure_non_executable(&metadata)?;
        let relative_path = path
            .strip_prefix(root)
            .map_err(|_| anyhow!("collaboration_plugin_package_path_invalid"))?
            .to_path_buf();
        validate_relative_components(&relative_path)?;
        let declared_bytes = usize::try_from(metadata.len())
            .map_err(|_| anyhow!("collaboration_plugin_package_file_too_large"))?;
        let next_total = total_bytes
            .checked_add(declared_bytes)
            .ok_or_else(|| anyhow!("collaboration_plugin_package_size_invalid"))?;
        ensure!(
            declared_bytes <= MAX_PACKAGE_BYTES && next_total <= MAX_PACKAGE_BYTES,
            "collaboration_plugin_package_too_large"
        );
        let bytes = super::secure_file::read_file_no_follow(&path, declared_bytes)?;
        ensure!(
            bytes.len() == declared_bytes,
            "collaboration_plugin_package_file_changed"
        );
        *total_bytes = next_total;
        files.push(PackageFile {
            relative_path,
            bytes,
        });
    }
    Ok(())
}

pub(super) fn package_file<'a>(files: &'a [PackageFile], relative: &Path) -> Result<&'a [u8]> {
    files
        .binary_search_by(|file| file.relative_path.as_path().cmp(relative))
        .ok()
        .map(|index| files[index].bytes.as_slice())
        .ok_or_else(|| anyhow!("collaboration_plugin_required_file_missing"))
}

pub(super) fn validate_relative_components(path: &Path) -> Result<()> {
    let mut count = 0usize;
    for component in path.components() {
        match component {
            Component::Normal(value) => {
                let text = value
                    .to_str()
                    .ok_or_else(|| anyhow!("collaboration_plugin_package_path_encoding_invalid"))?;
                ensure!(text != ".git", "collaboration_plugin_git_metadata_rejected");
                ensure!(
                    !text.is_empty()
                        && text.bytes().all(|byte| byte.is_ascii_alphanumeric()
                            || matches!(byte, b'.' | b'_' | b'-')),
                    "collaboration_plugin_package_path_invalid"
                );
                count += 1;
            }
            _ => return Err(anyhow!("collaboration_plugin_package_path_invalid")),
        }
    }
    ensure!(count > 0, "collaboration_plugin_package_path_invalid");
    Ok(())
}

fn normalized_relative_text(path: &Path) -> Result<String> {
    validate_relative_components(path)?;
    let parts = path
        .components()
        .map(|component| match component {
            Component::Normal(value) => value
                .to_str()
                .map(str::to_owned)
                .ok_or_else(|| anyhow!("collaboration_plugin_package_path_encoding_invalid")),
            _ => Err(anyhow!("collaboration_plugin_package_path_invalid")),
        })
        .collect::<Result<Vec<_>>>()?;
    Ok(parts.join("/"))
}

#[cfg(unix)]
fn ensure_non_executable(metadata: &fs::Metadata) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    ensure!(
        metadata.permissions().mode() & 0o111 == 0,
        "collaboration_plugin_executable_file_rejected"
    );
    Ok(())
}

#[cfg(not(unix))]
fn ensure_non_executable(_metadata: &fs::Metadata) -> Result<()> {
    Ok(())
}
