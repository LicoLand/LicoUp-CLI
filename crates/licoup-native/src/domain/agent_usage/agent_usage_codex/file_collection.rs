use std::collections::{BTreeSet, HashSet};
use std::fs;
#[cfg(unix)]
use std::os::unix::fs::MetadataExt;
#[cfg(windows)]
use std::os::windows::fs::MetadataExt;
use std::path::PathBuf;
use std::time::UNIX_EPOCH;

#[derive(Clone, Debug)]
pub(super) struct FileMetadata {
    pub(super) modified_ns: u64,
    pub(super) size: u64,
    pub(super) file_id: Option<String>,
}

pub(super) fn collect_usage_files(roots: &[PathBuf]) -> BTreeSet<PathBuf> {
    let mut pending = roots.to_vec();
    let mut visited_directories = HashSet::<PathBuf>::new();
    let mut files = BTreeSet::<PathBuf>::new();
    while let Some(path) = pending.pop() {
        let Ok(metadata) = fs::symlink_metadata(&path) else {
            continue;
        };
        if metadata.file_type().is_symlink() {
            continue;
        }
        if metadata.is_dir() {
            if !visited_directories.insert(path.clone()) {
                continue;
            }
            let Ok(entries) = fs::read_dir(&path) else {
                continue;
            };
            pending.extend(entries.flatten().map(|entry| entry.path()));
            continue;
        }
        if !metadata.is_file() {
            continue;
        }
        let extension = path
            .extension()
            .and_then(|value| value.to_str())
            .unwrap_or_default()
            .to_ascii_lowercase();
        if matches!(extension.as_str(), "jsonl" | "ndjson") {
            files.insert(path);
        }
    }
    files
}

pub(super) fn file_metadata(path: &std::path::Path) -> Option<FileMetadata> {
    let metadata = fs::metadata(path).ok()?;
    let modified_ns = metadata
        .modified()
        .ok()?
        .duration_since(UNIX_EPOCH)
        .ok()?
        .as_nanos()
        .min(u64::MAX as u128) as u64;
    #[cfg(unix)]
    let file_id = Some(format!("{}:{}", metadata.dev(), metadata.ino()));
    #[cfg(windows)]
    let file_id =
        (metadata.creation_time() > 0).then(|| format!("windows:{}", metadata.creation_time()));
    #[cfg(not(any(unix, windows)))]
    let file_id = None;
    Some(FileMetadata {
        modified_ns,
        size: metadata.len(),
        file_id,
    })
}
