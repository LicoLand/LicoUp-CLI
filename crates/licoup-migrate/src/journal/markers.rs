//! Durable marker storage for the tool's own run state.
//!
//! The tool keeps one small JSON marker per run: the journal while a conversion is
//! unfinished, the ledger snapshot after it completes, and the step markers the resume
//! path advances. None of them is a second schema authority — every version in them was
//! read from the client's own projection — so what this module owns is only *durability*.
//!
//! A marker is written the way every other durable decision in this repository is
//! written: serialise, write to a temporary name in the same directory, flush it, rename
//! it over the destination, then flush the directory entry. A reader therefore sees the
//! previous document or the new one, and a crash between the two can leave an unreferenced
//! temporary file, never a half-written marker under the real name. The stale temporary
//! files a crash does leave are swept the next time the directory is written, so an
//! interrupted run does not accumulate garbage.

use crate::error::{ToolResult, marker_read_failed, marker_unwritable};
use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

/// The largest marker this tool will write.
///
/// A journal holds one entry per declared domain, not per record, so this is far above
/// any real document; it exists to refuse a runaway write rather than to size a buffer.
pub const MAX_MARKER_BYTES: usize = 16 * 1024 * 1024;

/// The file name of the run journal inside the migrations directory.
pub const JOURNAL_FILE: &str = "data-migration-journal.json";

/// The file name of the ledger snapshot inside the migrations directory.
pub const LEDGER_FILE: &str = "data-migration-ledger.json";

/// The temporary-name suffix an in-flight write uses.
const TEMPORARY_SUFFIX: &str = ".tmp";

static WRITE_SEQUENCE: AtomicU64 = AtomicU64::new(0);

/// Where one data root keeps the state this tool owns.
///
/// The directory is the client's own migrations directory, so the tool's markers live
/// beside the client's ledger instead of in a second location the tool would have to
/// keep in step.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MarkerRoot {
    directory: PathBuf,
}

impl MarkerRoot {
    /// The marker directory for one data root.
    pub fn at(data_root: &Path) -> Self {
        Self {
            directory: data_root.join("client-state").join("migrations"),
        }
    }

    /// The directory itself.
    pub fn directory(&self) -> &Path {
        &self.directory
    }

    /// The journal file for this root.
    pub fn journal_path(&self) -> PathBuf {
        self.directory.join(JOURNAL_FILE)
    }

    /// The ledger snapshot file for this root.
    pub fn ledger_path(&self) -> PathBuf {
        self.directory.join(LEDGER_FILE)
    }

    /// The client's own migration ledger, which this tool reads and never writes.
    pub fn client_ledger_path(&self) -> PathBuf {
        self.directory.join("ledger.json")
    }

    /// The client's own domain marker directory.
    pub fn client_domain_state_directory(&self) -> PathBuf {
        self.directory.join("domain-state")
    }
}

/// Read one JSON marker.
///
/// A missing marker and an empty marker both read as `None`: an interrupted write never
/// leaves an empty file under a real name, but a synchronisation tool or an editor can,
/// and reporting "no marker" is both true and the safe direction — it sends the caller
/// back through the client's owner instead of trusting a document that is not there.
pub fn read_json<T: serde::de::DeserializeOwned>(path: &Path) -> ToolResult<Option<T>> {
    let raw = match fs::read(path) {
        Ok(raw) => raw,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(_) => return Err(marker_read_failed(path)),
    };
    if raw.iter().all(u8::is_ascii_whitespace) {
        return Ok(None);
    }
    // The document is present and whole but is not the JSON this marker has to be: that is
    // a different failure from not being able to read it at all, and the two codes are what
    // let an operator tell a damaged marker from a permissions problem.
    serde_json::from_slice(&raw)
        .map(Some)
        .map_err(|_| crate::error::MARKER_INVALID)
}

/// Write one JSON marker atomically, creating its directory when it is missing.
pub fn write_json<T: serde::Serialize>(path: &Path, value: &T) -> ToolResult<()> {
    let mut serialized = serde_json::to_vec_pretty(value).map_err(|_| marker_unwritable())?;
    serialized.push(b'\n');
    if serialized.len() > MAX_MARKER_BYTES {
        return Err(marker_unwritable());
    }
    let Some(directory) = path.parent() else {
        return Err(marker_unwritable());
    };
    if let Some(state_root) = directory.parent() {
        ensure_private_directory(state_root)?;
    }
    ensure_private_directory(directory)?;
    sweep_stale_temporaries(directory);

    let sequence = WRITE_SEQUENCE.fetch_add(1, Ordering::Relaxed);
    let temporary = directory.join(format!(
        "{}.{}.{sequence}{TEMPORARY_SUFFIX}",
        path.file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_else(|| "marker".to_string()),
        std::process::id()
    ));
    let mut guard = TemporaryFile::new(temporary.clone());
    {
        let mut file = open_temporary(&temporary).map_err(|_| marker_unwritable())?;
        file.write_all(&serialized)
            .map_err(|_| marker_unwritable())?;
        file.flush().map_err(|_| marker_unwritable())?;
        file.sync_all().map_err(|_| marker_unwritable())?;
    }
    fs::rename(&temporary, path).map_err(|_| marker_unwritable())?;
    guard.disarm();
    sync_directory(directory);
    Ok(())
}

/// Remove one marker, reporting whether it was there.
pub fn remove(path: &Path) -> ToolResult<bool> {
    match fs::remove_file(path) {
        Ok(()) => Ok(true),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(_) => Err(marker_unwritable()),
    }
}

/// Read the bytes of one marker, for a caller that hashes the document rather than
/// parsing it.
pub fn read_bytes(path: &Path) -> ToolResult<Option<Vec<u8>>> {
    match fs::read(path) {
        Ok(raw) => Ok(Some(raw)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(_) => Err(marker_read_failed(path)),
    }
}

fn open_temporary(path: &Path) -> std::io::Result<File> {
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    options.open(path)
}

fn ensure_private_directory(directory: &Path) -> ToolResult<()> {
    fs::create_dir_all(directory).map_err(|_| marker_unwritable())?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(directory, fs::Permissions::from_mode(0o700))
            .map_err(|_| marker_unwritable())?;
    }
    Ok(())
}

/// Flush the directory entry itself, so the rename survives a power loss.
///
/// Not every platform allows a directory to be opened for this, and failing to flush it
/// does not undo the atomic rename that already happened; it only means the entry's
/// power-loss durability is not claimed.
fn sync_directory(directory: &Path) {
    if let Ok(handle) = File::open(directory) {
        let _ = handle.sync_all();
    }
}

/// Delete temporary files a previous interrupted write left behind.
///
/// The sweep is best effort and only ever touches names this module itself creates, so a
/// crash cannot leave the directory growing without bound.
fn sweep_stale_temporaries(directory: &Path) {
    let Ok(entries) = fs::read_dir(directory) else {
        return;
    };
    for entry in entries.filter_map(Result::ok) {
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if name.ends_with(TEMPORARY_SUFFIX) {
            let _ = fs::remove_file(entry.path());
        }
    }
}

struct TemporaryFile {
    path: PathBuf,
    armed: bool,
}

impl TemporaryFile {
    fn new(path: PathBuf) -> Self {
        Self { path, armed: true }
    }

    fn disarm(&mut self) {
        self.armed = false;
    }
}

impl Drop for TemporaryFile {
    fn drop(&mut self) {
        if self.armed {
            let _ = fs::remove_file(&self.path);
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use crate::journal::test_support::scratch;
    use serde::Deserialize;

    #[derive(Debug, Deserialize, PartialEq, serde::Serialize)]
    #[serde(rename_all = "camelCase")]
    struct Sample {
        value: u32,
    }

    #[test]
    fn a_marker_is_read_back_exactly_as_it_was_written() {
        let root = scratch("round-trip");
        let path = root.join("nested/document.json");
        write_json(&path, &Sample { value: 7 }).expect("write");
        let read: Sample = read_json(&path).expect("read").expect("present");
        assert_eq!(read, Sample { value: 7 });
    }

    #[test]
    fn an_absent_or_empty_marker_reads_as_absent() {
        let root = scratch("absent");
        let absent = root.join("absent.json");
        assert!(read_json::<Sample>(&absent).expect("absent").is_none());
        let empty = root.join("empty.json");
        fs::write(&empty, b"   \n").expect("write empty");
        assert!(read_json::<Sample>(&empty).expect("empty").is_none());
    }

    #[test]
    fn truncated_json_is_refused_instead_of_read_as_empty() {
        let root = scratch("truncated");
        let path = root.join("truncated.json");
        fs::write(&path, b"{\"value\":").expect("write truncated");
        let error = read_json::<Sample>(&path).expect_err("truncated marker");
        assert_eq!(error.code(), "migration_marker_invalid");
    }

    #[test]
    fn no_temporary_file_survives_a_write() {
        let root = scratch("no-temporaries");
        let path = root.join("document.json");
        write_json(&path, &Sample { value: 1 }).expect("write");
        write_json(&path, &Sample { value: 2 }).expect("rewrite");
        let leftovers: Vec<String> = fs::read_dir(&root)
            .expect("read dir")
            .filter_map(Result::ok)
            .map(|entry| entry.file_name().to_string_lossy().into_owned())
            .filter(|name| name.ends_with(TEMPORARY_SUFFIX))
            .collect();
        assert!(
            leftovers.is_empty(),
            "an atomic write leaves no temporary file: {leftovers:?}"
        );
    }

    #[test]
    fn a_previous_interrupted_write_is_swept_away() {
        let root = scratch("sweep");
        let stale = root.join(format!("document.json.999.0{TEMPORARY_SUFFIX}"));
        fs::write(&stale, b"half").expect("write stale");
        write_json(&root.join("document.json"), &Sample { value: 3 }).expect("write");
        assert!(!stale.exists(), "the sweep removes an orphaned temporary");
    }

    #[test]
    fn removing_an_absent_marker_is_not_a_failure() {
        let root = scratch("remove");
        assert!(!remove(&root.join("absent.json")).expect("remove absent"));
        let path = root.join("present.json");
        write_json(&path, &Sample { value: 1 }).expect("write");
        assert!(remove(&path).expect("remove present"));
        assert!(!path.exists());
    }

    #[test]
    fn a_marker_never_carries_a_path_in_its_failure_code() {
        let root = scratch("privacy");
        let path = root.join("truncated.json");
        fs::write(&path, b"{").expect("write truncated");
        let error = read_json::<Sample>(&path).expect_err("truncated marker");
        let rendered = error.to_string();
        assert_eq!(rendered, "migration_marker_invalid");
        assert!(!rendered.contains(root.to_string_lossy().as_ref()));
    }
}
