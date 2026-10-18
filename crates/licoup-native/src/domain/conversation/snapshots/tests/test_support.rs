use super::*;

pub(super) fn temp_dir(name: &str) -> PathBuf {
    let counter = TEMP_DIR_COUNTER.fetch_add(1, Ordering::Relaxed);
    let timestamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let dir = env::temp_dir().join(format!(
        "lico-conversation-snapshots-{}-{}-{}-{}",
        name,
        std::process::id(),
        timestamp,
        counter
    ));
    fs::create_dir_all(&dir).unwrap();
    // macOS exposes the system temporary directory through a stable
    // symlink alias. Archive extraction deliberately rejects symlinked
    // destination ancestors, so tests exercise the real no-follow path.
    dir.canonicalize().unwrap()
}
