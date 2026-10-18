use anyhow::{Result, anyhow, ensure};
use sha2::{Digest, Sha256};
use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

use super::model::AssemblyPayloadFile;
use super::{ASSEMBLED_RUNTIME_DATA_DIRECTORY, ASSEMBLY_MANIFEST_FILE, ASSEMBLY_SNAPSHOT_FILE};
use crate::domain::collaboration_plugin::manifest::{
    normalized_relative_protocol_path, validate_relative_path,
};
use crate::domain::collaboration_plugin::package::{SelectedPayloadFile, read_file_no_follow};

pub(super) fn from_selected(files: &[SelectedPayloadFile]) -> Result<Vec<AssemblyPayloadFile>> {
    let mut inventory = files
        .iter()
        .map(|file| {
            Ok(AssemblyPayloadFile {
                selection_id: file.selection_id.clone(),
                source_relative_path: normalized_relative_protocol_path(
                    &file.source_relative_path,
                )?,
                destination_relative_path: normalized_relative_protocol_path(
                    &file.destination_relative_path,
                )?,
                digest_sha256: file.digest_sha256.clone(),
                bytes: file.bytes.len(),
            })
        })
        .collect::<Result<Vec<_>>>()?;
    inventory.sort_by(|left, right| {
        left.destination_relative_path
            .cmp(&right.destination_relative_path)
    });
    validate(&inventory)?;
    Ok(inventory)
}

pub(super) fn digest(files: &[AssemblyPayloadFile]) -> Result<String> {
    validate(files)?;
    let mut hasher = Sha256::new();
    hasher.update(b"LICOUP-ASSEMBLY-PAYLOAD-INVENTORY-V1\0");
    for file in files {
        for field in [
            file.selection_id.as_str(),
            file.source_relative_path.as_str(),
            file.destination_relative_path.as_str(),
            file.digest_sha256.as_str(),
        ] {
            hasher.update((field.len() as u64).to_be_bytes());
            hasher.update(field.as_bytes());
        }
        hasher.update((file.bytes as u64).to_be_bytes());
    }
    Ok(format!("{:x}", hasher.finalize()))
}

pub(super) fn validate(files: &[AssemblyPayloadFile]) -> Result<()> {
    ensure!(
        !files.is_empty() && files.len() <= 512,
        "collaboration_local_server_payload_inventory_invalid"
    );
    let mut destinations = BTreeSet::new();
    for file in files {
        let source = validate_relative_path(
            &file.source_relative_path,
            "collaboration_local_server_payload_source_path_invalid",
        )?;
        let destination = validate_relative_path(
            &file.destination_relative_path,
            "collaboration_local_server_payload_destination_path_invalid",
        )?;
        let first = destination
            .components()
            .next()
            .and_then(|part| part.as_os_str().to_str());
        ensure!(
            is_slug(&file.selection_id)
                && !matches!(file.selection_id.as_str(), "runner" | "runtime-data")
                && first == Some(file.selection_id.as_str())
                && source.components().count() <= 16
                && destination.components().count() <= 17
                && destinations.insert(destination)
                && is_sha256(&file.digest_sha256)
                && file.bytes <= 32 * 1024 * 1024,
            "collaboration_local_server_payload_inventory_invalid"
        );
    }
    ensure!(
        files
            .windows(2)
            .all(|pair| { pair[0].destination_relative_path < pair[1].destination_relative_path }),
        "collaboration_local_server_payload_inventory_invalid"
    );
    Ok(())
}

pub(super) fn verify_tree(
    root: &Path,
    files: &[AssemblyPayloadFile],
    runner_relative_path: &str,
) -> Result<()> {
    let metadata = fs::symlink_metadata(root)
        .map_err(|_| anyhow!("collaboration_local_server_assembly_tree_unavailable"))?;
    ensure!(
        metadata.file_type().is_dir() && !metadata.file_type().is_symlink(),
        "collaboration_local_server_assembly_root_invalid"
    );
    validate(files)?;
    for file in files {
        let path = root.join(&file.destination_relative_path);
        let bytes = read_file_no_follow(&path, file.bytes)?;
        ensure!(
            bytes.len() == file.bytes
                && format!("{:x}", Sha256::digest(&bytes)) == file.digest_sha256,
            "collaboration_local_server_payload_digest_mismatch"
        );
    }
    reject_unlisted_entries(root, files, runner_relative_path)
}

fn reject_unlisted_entries(
    root: &Path,
    files: &[AssemblyPayloadFile],
    runner_relative_path: &str,
) -> Result<()> {
    let mut expected_files = files
        .iter()
        .map(|file| PathBuf::from(&file.destination_relative_path))
        .collect::<BTreeSet<_>>();
    expected_files.insert(PathBuf::from(ASSEMBLY_MANIFEST_FILE));
    expected_files.insert(PathBuf::from(ASSEMBLY_SNAPSHOT_FILE));
    expected_files.insert(PathBuf::from(runner_relative_path));
    let mut expected_directories = BTreeSet::new();
    for file in &expected_files {
        let mut parent = file.parent();
        while let Some(directory) = parent {
            if directory.as_os_str().is_empty() {
                break;
            }
            expected_directories.insert(directory.to_path_buf());
            parent = directory.parent();
        }
    }
    visit_tree(root, root, &expected_files, &expected_directories)
}

fn visit_tree(
    root: &Path,
    directory: &Path,
    expected_files: &BTreeSet<PathBuf>,
    expected_directories: &BTreeSet<PathBuf>,
) -> Result<()> {
    let mut entries = fs::read_dir(directory)
        .map_err(|_| anyhow!("collaboration_local_server_assembly_tree_unavailable"))?
        .collect::<std::io::Result<Vec<_>>>()?;
    entries.sort_by_key(|entry| entry.file_name());
    for entry in entries {
        let path = entry.path();
        let relative = path
            .strip_prefix(root)
            .map_err(|_| anyhow!("collaboration_local_server_assembly_tree_invalid"))?
            .to_path_buf();
        let normalized = normalized_relative_protocol_path(&relative)?;
        if normalized == ASSEMBLED_RUNTIME_DATA_DIRECTORY {
            let metadata = fs::symlink_metadata(&path)?;
            ensure!(
                metadata.file_type().is_dir() && !metadata.file_type().is_symlink(),
                "collaboration_local_server_runtime_data_invalid"
            );
            continue;
        }
        let metadata = fs::symlink_metadata(&path)?;
        ensure!(
            !metadata.file_type().is_symlink(),
            "collaboration_local_server_assembly_symlink_rejected"
        );
        if metadata.file_type().is_dir() {
            ensure!(
                expected_directories.contains(&relative),
                "collaboration_local_server_unlisted_assembly_entry"
            );
            visit_tree(root, &path, expected_files, expected_directories)?;
        } else {
            ensure!(
                metadata.file_type().is_file() && expected_files.contains(&relative),
                "collaboration_local_server_unlisted_assembly_entry"
            );
        }
    }
    Ok(())
}

fn is_slug(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && value
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
}

fn is_sha256(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
}
