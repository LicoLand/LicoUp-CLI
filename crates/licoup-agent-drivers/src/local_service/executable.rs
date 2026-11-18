use serde_json::Value;
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

use super::params;

pub fn resolve(params: &Value, environment_keys: &[&str], default_executable: &str) -> String {
    if let Some(value) = params::text(params, &["executable", "binary", "binaryPath"]) {
        return value;
    }
    environment_keys
        .iter()
        .find_map(|key| {
            std::env::var(key)
                .ok()
                .map(|value| value.trim().to_string())
                .filter(|value| !value.is_empty())
        })
        .unwrap_or_else(|| default_executable.to_string())
}

pub fn available(executable: &str) -> bool {
    let path = Path::new(executable);
    if path.is_absolute() || executable.contains('/') || executable.contains('\\') {
        return path.is_file();
    }
    which(executable)
}

#[derive(Clone, Debug)]
pub struct ResolvedExecutable {
    pub path: String,
    pub private_file_identity: String,
}

pub fn resolve_file(executable: &str) -> Option<ResolvedExecutable> {
    let requested = Path::new(executable);
    let candidate =
        if requested.is_absolute() || executable.contains('/') || executable.contains('\\') {
            requested.to_path_buf()
        } else {
            which_path(executable)?
        };
    let canonical = candidate.canonicalize().ok()?;
    let metadata = canonical.metadata().ok()?;
    if !metadata.is_file() {
        return None;
    }
    let modified = metadata
        .modified()
        .ok()
        .and_then(|value| value.duration_since(UNIX_EPOCH).ok())
        .map(|value| value.as_nanos())
        .unwrap_or_default();
    let mut hasher = Sha256::new();
    hasher.update(canonical.to_string_lossy().as_bytes());
    hasher.update(metadata.len().to_le_bytes());
    hasher.update(modified.to_le_bytes());
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        hasher.update(metadata.dev().to_le_bytes());
        hasher.update(metadata.ino().to_le_bytes());
    }
    Some(ResolvedExecutable {
        path: canonical.to_string_lossy().into_owned(),
        private_file_identity: format!("{:x}", hasher.finalize()),
    })
}

pub fn which(name: &str) -> bool {
    which_path(name).is_some()
}

fn which_path(name: &str) -> Option<PathBuf> {
    // The user shell PATH (with the process PATH fallback) is searched so a
    // CLI on the user's terminal PATH is found even when the LicoUp process
    // PATH lacks it.
    which_path_in_dirs(
        name,
        &licoup_agent_targets::platform::user_shell_environment::search_path_dirs(),
    )
}

pub fn which_path_in_dirs(name: &str, dirs: &[PathBuf]) -> Option<PathBuf> {
    dirs.iter().find_map(|entry| {
        let candidate = entry.join(name);
        if candidate.is_file() {
            return Some(candidate);
        }
        #[cfg(windows)]
        {
            let candidate = entry.join(format!("{}.exe", name));
            if candidate.is_file() {
                return Some(candidate);
            }
        }
        None
    })
}
