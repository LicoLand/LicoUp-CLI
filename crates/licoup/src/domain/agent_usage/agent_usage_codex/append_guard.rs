use super::constants::CONTENT_GUARD_BUFFER_BYTES;
use super::models::CachedFile;
use anyhow::{Context, Result};
use sha2::{Digest, Sha256};
use std::fs;
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;

pub(super) fn append_guard_matches(path: &Path, cached: &CachedFile) -> bool {
    !cached.append_guard.is_empty()
        && content_guard_state(path, cached.size)
            .map(|state| content_guard_digest(&state) == cached.append_guard)
            .unwrap_or(false)
}

pub(super) fn append_guard(path: &Path, guarded_bytes: u64) -> Result<String> {
    content_guard_state(path, guarded_bytes).map(|state| content_guard_digest(&state))
}

pub(super) fn content_guard_state(path: &Path, guarded_bytes: u64) -> Result<Sha256> {
    let mut file = fs::File::open(path).context("Codex usage append guard open failed")?;
    let file_size = file.metadata()?.len();
    if guarded_bytes > file_size {
        anyhow::bail!("Codex usage append guard exceeds file length");
    }
    let mut hasher = Sha256::new();
    hasher.update(b"codex-content-guard-v2\0");
    read_content_guard(&mut file, guarded_bytes, &mut hasher)?;
    Ok(hasher)
}

pub(super) fn extend_content_guard(
    path: &Path,
    guarded_bytes: u64,
    target_bytes: u64,
    hasher: &mut Sha256,
) -> Result<()> {
    if target_bytes < guarded_bytes {
        anyhow::bail!("Codex usage append guard target precedes cached length");
    }
    let mut file = fs::File::open(path).context("Codex usage append guard open failed")?;
    if file.metadata()?.len() < target_bytes {
        anyhow::bail!("Codex usage append guard exceeds file length");
    }
    file.seek(SeekFrom::Start(guarded_bytes))?;
    read_content_guard(
        &mut file,
        target_bytes.saturating_sub(guarded_bytes),
        hasher,
    )
}

fn read_content_guard(file: &mut fs::File, mut remaining: u64, hasher: &mut Sha256) -> Result<()> {
    let mut buffer = vec![0_u8; CONTENT_GUARD_BUFFER_BYTES];
    while remaining > 0 {
        let requested = remaining.min(buffer.len() as u64) as usize;
        file.read_exact(&mut buffer[..requested])?;
        hasher.update(&buffer[..requested]);
        remaining = remaining.saturating_sub(requested as u64);
    }
    Ok(())
}

pub(super) fn content_guard_digest(hasher: &Sha256) -> String {
    format!("{:x}", hasher.clone().finalize())
}
