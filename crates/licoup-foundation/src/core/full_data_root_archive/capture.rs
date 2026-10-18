//! Capture one data root into a plaintext archive.
//!
//! The caller names the destination; the owner never defaults one, and it never captures
//! the archive it is writing. Writers must be stopped before capture: the caller's
//! explicit `writers_stopped` statement is the authority, and the owner additionally
//! coordinates with the application's own state admission when its lock file already
//! exists. The owner never creates admission state in the source.
//!
//! The archive is written to a private temporary file beside the destination and
//! committed by rename, so an existing output is replaced only by a complete archive,
//! and a refused capture leaves both the source and any previous destination untouched.
//! The same inventory policy the importer applies is checked before the first byte is
//! written, so a successful export is importable by its own owner.

use anyhow::{Result, anyhow, ensure};
use flate2::Compression;
use flate2::write::GzEncoder;
use fs2::FileExt;
use std::fs::{File, OpenOptions};
use std::io::{BufReader, BufWriter, Seek, Write};
use std::path::{Path, PathBuf};

use crate::core::safe_archive::default_zip_extraction_limits;
use crate::platform::file_security::{AtomicPrivateFile, CleanupOutcome, CommitDurability};

use super::inventory::{
    ArchiveManifest, InventoryEntry, InventoryKind, RecoveryCoverage, RecoveryLimitation,
    ensure_within_archive_limits, inventory_data_root, validate_inventory_structure,
};
use super::{ADMISSION_LOCK_PATH, ArchiveContainer, DATA_PREFIX, MANIFEST_MEMBER};

/// Credential custody domain whose key material never travels in a plaintext archive.
///
/// Published so an inventory owner classifies the limitation by the domain this
/// owner declares rather than by a copy of its spelling.
pub const CREDENTIAL_DOMAIN: &str = "gateway-credential-custody";
/// Non-secret inventory document at the data-root-relative path its owner reads.
///
/// The document travels as metadata; the key material it describes does not.
/// Published for the same reason as [`CREDENTIAL_DOMAIN`]: a consumer attributes
/// the path its owner actually writes.
pub const CREDENTIAL_INVENTORY_PATH: &str = "llm-api-key-inventory.json";
#[derive(Clone, Debug)]
pub struct ExportRequest {
    /// Absolute data root to capture.
    pub data_root: PathBuf,
    /// Destination archive. The container is inferred from its extension.
    pub archive_path: PathBuf,
    /// The caller states that every writer, including older clients, is stopped.
    pub writers_stopped: bool,
}

#[derive(Clone, Debug)]
pub struct ExportOutcome {
    pub container: ArchiveContainer,
    /// The logical data-home spelling recorded in the archive manifest.
    pub source_home: PathBuf,
    pub coverage: RecoveryCoverage,
    pub limitations: Vec<RecoveryLimitation>,
    pub file_count: usize,
    pub total_bytes: u64,
}

/// Capture `data_root` into a standard plaintext archive.
pub fn export_data_root(request: &ExportRequest) -> Result<ExportOutcome> {
    // Decide every refusal before the source is inspected or the destination is touched.
    // The archive is never part of the data the capture operates on: an archive named
    // inside the root would be captured as a member of itself and a restore would then
    // write that file back into the restored root.
    ensure!(
        !archive_path_inside_data_root(&request.data_root, &request.archive_path),
        "archive_path_inside_data_root"
    );
    ensure!(request.writers_stopped, "archive_writers_running");
    ensure!(request.data_root.is_dir(), "data_root_missing");
    // The logical source home travels as provenance so a later import can rebase the
    // references its owners rewrite. It is normalized lexically here, never resolved
    // through links: the selected spelling is the identity the owners' references use.
    let source_home =
        std::path::absolute(&request.data_root).map_err(|_| anyhow!("data_root_unresolved"))?;
    let container = ArchiveContainer::from_path(&request.archive_path)?;
    if let Ok(metadata) = std::fs::symlink_metadata(&request.archive_path) {
        // A symbolic-link destination (including a dangling link into the root) is
        // refused no-follow; the atomic writer never follows one either.
        ensure!(
            !metadata.file_type().is_symlink(),
            "archive_destination_unsafe"
        );
    }

    let _admission = AdmissionGuard::acquire(&request.data_root)?;

    // Inventory and policy first: a refused capture publishes nothing and mutates nothing.
    let mut entries = inventory_data_root(&request.data_root)?;
    entries.retain(|entry| entry.path != ADMISSION_LOCK_PATH);
    let facts = validate_inventory_structure(&entries)
        .map_err(|_| anyhow!("data_root_path_not_portable"))?;
    let limitations = recovery_limitations(&entries);
    let coverage = if limitations.is_empty() {
        RecoveryCoverage::Complete
    } else {
        RecoveryCoverage::Limited
    };
    let manifest = ArchiveManifest::new(
        container.extension(),
        &source_home,
        coverage,
        limitations,
        entries,
    )?;
    let manifest_bytes = manifest.to_bytes()?;
    ensure_within_archive_limits(
        &facts,
        manifest_bytes.len() as u64,
        "archive_export_limits_exceeded",
    )?;

    let mut output = AtomicPrivateFile::create(&request.archive_path).map_err(|failure| {
        cleanup_failure("archive_destination_unwritable", failure.into_parts().1)
    })?;
    let write_result = (|| -> Result<()> {
        let mut buffered = BufWriter::new(output.file_mut());
        match container {
            ArchiveContainer::Zip => {
                let writer = write_zip(
                    &mut buffered,
                    &request.data_root,
                    &manifest,
                    &manifest_bytes,
                )?;
                writer
                    .flush()
                    .map_err(|_| anyhow!("archive_write_failed"))?;
            }
            ArchiveContainer::TarGz => {
                let writer = write_tar_gz(
                    &mut buffered,
                    &request.data_root,
                    &manifest,
                    &manifest_bytes,
                )?;
                writer
                    .flush()
                    .map_err(|_| anyhow!("archive_write_failed"))?;
            }
        }
        Ok(())
    })();
    if write_result.is_err() {
        // Explicit checked abort: report a retained private temporary instead of
        // relying on Drop, which cannot surface a cleanup failure.
        return Err(cleanup_failure("archive_write_failed", output.discard()));
    }
    finish_archive(output)?;

    Ok(ExportOutcome {
        container,
        source_home,
        coverage: manifest.coverage,
        limitations: manifest.limitations.clone(),
        file_count: manifest.file_count(),
        total_bytes: manifest.total_bytes,
    })
}

/// Whether `archive_path` names a destination the capture would read back as its own data.
///
/// The root is compared under its resolved name, and a destination that does not exist yet
/// is resolved through its nearest existing ancestor, so a destination placed inside the
/// root through a symbolic link is refused exactly like a direct one. The root's own path
/// counts as inside it: a capture cannot write over the thing it is describing. A path that
/// cannot be resolved is not this rule's refusal; the capture then reports the destination
/// it cannot write with its own code.
pub fn archive_path_inside_data_root(data_root: &Path, archive_path: &Path) -> bool {
    let (Ok(root), Some(destination)) = (
        data_root.canonicalize(),
        resolve_through_existing_ancestor(archive_path),
    ) else {
        return false;
    };
    destination.starts_with(root)
}

/// Resolve one path for comparison, following the ancestors that exist.
///
/// A relative name is read against the process working directory, exactly as the capture's
/// own file creation reads it, so the comparison describes the file the capture would open.
fn resolve_through_existing_ancestor(path: &Path) -> Option<PathBuf> {
    let mut existing = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir().ok()?.join(path)
    };
    let mut suffix: Vec<std::ffi::OsString> = Vec::new();
    loop {
        if existing.exists() {
            let mut resolved = existing.canonicalize().ok()?;
            while let Some(component) = suffix.pop() {
                resolved.push(component);
            }
            return Some(resolved);
        }
        let name = existing.file_name()?.to_os_string();
        suffix.push(name);
        if !existing.pop() {
            return None;
        }
    }
}

/// The credential custody limit, reported for every archive.
///
/// The archive can carry the non-secret inventory document when it exists, but credential
/// key material is not transported, and whether a key is available is determined by the
/// credential owner rather than by a file in the data root. File presence is therefore
/// reported as what it is and never completes the recovery.
fn recovery_limitations(entries: &[InventoryEntry]) -> Vec<RecoveryLimitation> {
    let metadata_present = entries
        .iter()
        .any(|entry| entry.kind == InventoryKind::File && entry.path == CREDENTIAL_INVENTORY_PATH);
    let reason = if metadata_present {
        format!(
            "{CREDENTIAL_INVENTORY_PATH} travels as non-secret metadata; credential key material is not transported and its availability is determined by the credential owner"
        )
    } else {
        format!(
            "{CREDENTIAL_INVENTORY_PATH} is absent from the captured root; credential key material is not transported and its availability is determined by the credential owner"
        )
    };
    vec![RecoveryLimitation {
        domain: CREDENTIAL_DOMAIN.to_string(),
        reason,
    }]
}

/// Turn a private-write cleanup outcome into the caller's refusal message: a retained
/// temporary is named as an incomplete cleanup instead of being erased.
fn cleanup_failure(code: &'static str, cleanup: CleanupOutcome) -> anyhow::Error {
    match cleanup {
        CleanupOutcome::Removed => anyhow!("{code}"),
        CleanupOutcome::DurabilityUnconfirmed(parent) => anyhow!(
            "{code}; archive_cleanup_durability_unconfirmed: {}",
            parent.display()
        ),
        CleanupOutcome::Retained(path) => retained_failure(code, Some(&path)),
    }
}

/// The same refusal with a retained private temporary already extracted.
fn retained_failure(code: &'static str, retained: Option<&Path>) -> anyhow::Error {
    match retained {
        Some(path) => anyhow!(
            "{code}; archive_cleanup_incomplete: retained {}",
            path.display()
        ),
        None => anyhow!("{code}"),
    }
}

/// Check the finished private artifact against the importer's own byte policy before
/// committing it, so a successful export is always importable under that policy.
///
/// The check runs after the bytes are written but before the rename that publishes
/// them; a refusal removes the private temporary file with a checked outcome and leaves
/// any prior output in place.
fn finish_archive(mut output: AtomicPrivateFile) -> Result<()> {
    let artifact_bytes = match output.file_mut().metadata() {
        Ok(metadata) => metadata.len(),
        Err(_) => return Err(cleanup_failure("archive_write_failed", output.discard())),
    };
    if artifact_bytes > default_zip_extraction_limits().max_archive_bytes {
        return Err(cleanup_failure(
            "archive_export_limits_exceeded",
            output.discard(),
        ));
    }
    match output.commit() {
        Ok(CommitDurability::Confirmed) => Ok(()),
        // The archive is complete at its destination and must be kept; only its
        // directory entry could not be confirmed durable. Report that typed incomplete
        // state rather than success or a misleading write failure.
        Ok(CommitDurability::Unconfirmed) => Err(anyhow!("archive_commit_durability_unconfirmed")),
        Err(failure) => Err(cleanup_failure(
            "archive_write_failed",
            failure.into_parts().1,
        )),
    }
}

fn write_zip<W: Write + Seek>(
    writer: W,
    root: &Path,
    manifest: &ArchiveManifest,
    manifest_bytes: &[u8],
) -> Result<W> {
    let mut zip = zip::ZipWriter::new(writer);
    let options = zip::write::SimpleFileOptions::default()
        .compression_method(zip::CompressionMethod::Deflated);

    zip.start_file(MANIFEST_MEMBER, options)
        .map_err(|_| anyhow!("archive_write_failed"))?;
    zip.write_all(manifest_bytes)
        .map_err(|_| anyhow!("archive_write_failed"))?;

    for entry in &manifest.entries {
        let member = format!("{DATA_PREFIX}{}", entry.path);
        match entry.kind {
            InventoryKind::Directory => {
                zip.add_directory(member, options)
                    .map_err(|_| anyhow!("archive_write_failed"))?;
            }
            InventoryKind::File => {
                zip.start_file(member, options)
                    .map_err(|_| anyhow!("archive_write_failed"))?;
                copy_file(root, entry, &mut zip)?;
            }
        }
    }
    zip.finish().map_err(|_| anyhow!("archive_write_failed"))
}

fn write_tar_gz<W: Write>(
    writer: W,
    root: &Path,
    manifest: &ArchiveManifest,
    manifest_bytes: &[u8],
) -> Result<W> {
    let encoder = GzEncoder::new(writer, Compression::default());
    let mut builder = tar::Builder::new(encoder);
    builder.mode(tar::HeaderMode::Deterministic);

    let mut header = tar::Header::new_gnu();
    header.set_size(manifest_bytes.len() as u64);
    header.set_mode(0o600);
    header.set_mtime(manifest.created_at_unix);
    header.set_cksum();
    builder
        .append_data(&mut header, MANIFEST_MEMBER, manifest_bytes)
        .map_err(|_| anyhow!("archive_write_failed"))?;

    for entry in &manifest.entries {
        let member = format!("{DATA_PREFIX}{}", entry.path);
        match entry.kind {
            InventoryKind::Directory => {
                let mut header = tar::Header::new_gnu();
                header.set_entry_type(tar::EntryType::Directory);
                header.set_size(0);
                header.set_mode(0o700);
                header.set_mtime(manifest.created_at_unix);
                header.set_cksum();
                builder
                    .append_data(&mut header, format!("{member}/"), std::io::empty())
                    .map_err(|_| anyhow!("archive_write_failed"))?;
            }
            InventoryKind::File => {
                let file = File::open(root.join(&entry.path))
                    .map_err(|_| anyhow!("data_root_entry_unreadable"))?;
                let metadata = file
                    .metadata()
                    .map_err(|_| anyhow!("data_root_entry_unreadable"))?;
                ensure!(
                    metadata.is_file() && metadata.len() == entry.size,
                    "data_root_entry_changed"
                );
                let mut header = tar::Header::new_gnu();
                header.set_entry_type(tar::EntryType::Regular);
                header.set_size(entry.size);
                header.set_mode(0o600);
                header.set_mtime(manifest.created_at_unix);
                header.set_cksum();
                builder
                    .append_data(&mut header, member, BufReader::new(file))
                    .map_err(|_| anyhow!("archive_write_failed"))?;
            }
        }
    }

    let encoder = builder
        .into_inner()
        .map_err(|_| anyhow!("archive_write_failed"))?;
    // The inner writer is returned to the caller, which flushes it and then commits the
    // atomic file; a final ENOSPC therefore fails the capture instead of being dropped.
    encoder
        .finish()
        .map_err(|_| anyhow!("archive_write_failed"))
}

fn copy_file<W: Write>(root: &Path, entry: &InventoryEntry, writer: &mut W) -> Result<()> {
    let file =
        File::open(root.join(&entry.path)).map_err(|_| anyhow!("data_root_entry_unreadable"))?;
    let metadata = file
        .metadata()
        .map_err(|_| anyhow!("data_root_entry_unreadable"))?;
    ensure!(
        metadata.is_file() && metadata.len() == entry.size,
        "data_root_entry_changed"
    );
    let mut reader = BufReader::new(file);
    let written =
        std::io::copy(&mut reader, writer).map_err(|_| anyhow!("archive_write_failed"))?;
    ensure!(written == entry.size, "data_root_entry_changed");
    Ok(())
}

/// Coordination with the application's own state admission, when its lock file exists.
///
/// The caller's explicit `writers_stopped` statement remains the authority; holding this
/// lock is coordination with the current application's writers and is not proof that every
/// writer (including an older client) has stopped. The guard opens only an existing lock
/// file and never creates admission state inside the source root.
struct AdmissionGuard {
    file: Option<File>,
}

impl AdmissionGuard {
    fn acquire(data_root: &Path) -> Result<Self> {
        let path = data_root.join(ADMISSION_LOCK_PATH);
        match OpenOptions::new().read(true).open(&path) {
            Ok(file) => {
                file.try_lock_exclusive()
                    .map_err(|_| anyhow!("archive_writers_running"))?;
                Ok(Self { file: Some(file) })
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(Self { file: None }),
            Err(_) => Err(anyhow!("archive_admission_unavailable")),
        }
    }
}

impl Drop for AdmissionGuard {
    fn drop(&mut self) {
        if let Some(file) = &self.file {
            let _ = fs2::FileExt::unlock(file);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::full_data_root_archive::inventory::InventoryFacts;
    use crate::core::safe_archive::default_zip_extraction_limits;
    use std::io::{self, SeekFrom};

    struct FaultyZipWriter {
        inner: std::io::Cursor<Vec<u8>>,
        fail_after: usize,
    }

    impl Write for FaultyZipWriter {
        fn write(&mut self, buffer: &[u8]) -> io::Result<usize> {
            if self.inner.position() as usize >= self.fail_after {
                return Err(io::Error::new(
                    io::ErrorKind::Other,
                    "injected write failure",
                ));
            }
            self.inner.write(buffer)
        }

        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    impl Seek for FaultyZipWriter {
        fn seek(&mut self, position: SeekFrom) -> io::Result<u64> {
            self.inner.seek(position)
        }
    }

    struct FailingWriter {
        fail_after: usize,
        written: usize,
    }

    impl Write for FailingWriter {
        fn write(&mut self, buffer: &[u8]) -> io::Result<usize> {
            if self.written >= self.fail_after {
                return Err(io::Error::new(
                    io::ErrorKind::Other,
                    "injected write failure",
                ));
            }
            let remaining = self.fail_after - self.written;
            let accepted = buffer.len().min(remaining.max(1));
            self.written += accepted;
            Ok(accepted)
        }

        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    struct FlushFailingWriter;

    impl Write for FlushFailingWriter {
        fn write(&mut self, buffer: &[u8]) -> io::Result<usize> {
            Ok(buffer.len())
        }

        fn flush(&mut self) -> io::Result<()> {
            Err(io::Error::new(
                io::ErrorKind::Other,
                "injected flush failure",
            ))
        }
    }

    fn empty_manifest(container: &str) -> (ArchiveManifest, Vec<u8>) {
        let manifest = ArchiveManifest::new(
            container,
            Path::new("/synthetic/source-home"),
            RecoveryCoverage::Limited,
            Vec::new(),
            Vec::new(),
        )
        .expect("empty manifest");
        let bytes = manifest.to_bytes().expect("encode manifest");
        (manifest, bytes)
    }

    #[test]
    fn zip_write_propagates_an_injected_writer_failure() {
        let (manifest, bytes) = empty_manifest("zip");
        let writer = FaultyZipWriter {
            inner: std::io::Cursor::new(Vec::new()),
            fail_after: 2,
        };
        let result = write_zip(writer, Path::new("."), &manifest, &bytes);
        assert!(result.is_err());
    }

    #[test]
    fn tar_gz_write_propagates_an_injected_writer_failure() {
        let (manifest, bytes) = empty_manifest("tar.gz");
        let writer = FailingWriter {
            fail_after: 1,
            written: 0,
        };
        let result = write_tar_gz(writer, Path::new("."), &manifest, &bytes);
        assert!(result.is_err());
    }

    #[test]
    fn tar_gz_returns_the_inner_writer_so_the_final_flush_is_observed() {
        let (manifest, bytes) = empty_manifest("tar.gz");
        let writer = write_tar_gz(FlushFailingWriter, Path::new("."), &manifest, &bytes)
            .expect("encoder finishes");
        let mut writer = writer;
        assert!(
            writer.flush().is_err(),
            "the returned writer's flush must be observable"
        );
    }

    #[test]
    fn manifest_overhead_counts_towards_the_export_limit() {
        let limits = default_zip_extraction_limits();
        let facts = InventoryFacts {
            file_bytes: limits.max_total_bytes,
            member_count: 2,
            max_member_depth: 2,
            max_file_bytes: 1,
        };
        assert!(ensure_within_archive_limits(&facts, 1, "archive_export_limits_exceeded").is_err());
    }

    fn artifact_scratch(name: &str) -> std::path::PathBuf {
        let root = std::env::temp_dir().join(format!("lico-capture-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).expect("create scratch");
        root
    }

    #[test]
    fn capture_excludes_the_ephemeral_admission_lock() {
        let fixture = artifact_scratch("admission-lock");
        let root = fixture.join("source");
        let migration_root = root.join("client-state/migrations");
        std::fs::create_dir_all(&migration_root).expect("create migration root");
        std::fs::write(migration_root.join("admission.lock"), b"").expect("create admission lock");
        std::fs::write(root.join("document.json"), b"{}").expect("create application document");
        let archive = fixture.join("capture.zip");

        export_data_root(&ExportRequest {
            data_root: root,
            archive_path: archive.clone(),
            writers_stopped: true,
        })
        .expect("capture while holding the admission lock");

        let mut zip = zip::ZipArchive::new(std::fs::File::open(archive).expect("open capture"))
            .expect("read capture");
        let names = (0..zip.len())
            .map(|index| {
                zip.by_index(index)
                    .expect("archive member")
                    .name()
                    .expect("valid entry name")
                    .to_string()
            })
            .collect::<Vec<_>>();
        assert!(names.contains(&format!("{DATA_PREFIX}document.json")));
        assert!(!names.contains(&format!("{DATA_PREFIX}{ADMISSION_LOCK_PATH}")));

        std::fs::remove_dir_all(fixture).expect("remove scratch");
    }

    #[test]
    fn an_artifact_beyond_the_import_byte_policy_is_refused_before_commit() {
        let root = artifact_scratch("oversized-artifact");
        let destination = root.join("artifact.zip");
        let mut output = AtomicPrivateFile::create(&destination).expect("create private output");
        let limits = default_zip_extraction_limits();
        output
            .file_mut()
            .set_len(limits.max_archive_bytes + 1)
            .expect("size the sparse artifact");
        let error = finish_archive(output).expect_err("oversized artifact is refused");
        assert_eq!(error.to_string(), "archive_export_limits_exceeded");
        assert!(
            !destination.exists(),
            "a refused artifact is never published"
        );
        assert_eq!(
            std::fs::read_dir(&root).expect("scratch").count(),
            0,
            "the private temporary artifact is removed"
        );
        std::fs::remove_dir_all(root).expect("remove scratch");
    }

    #[test]
    fn an_artifact_at_the_import_byte_policy_commits() {
        let root = artifact_scratch("supported-artifact");
        let destination = root.join("artifact.zip");
        let mut output = AtomicPrivateFile::create(&destination).expect("create private output");
        let limits = default_zip_extraction_limits();
        output
            .file_mut()
            .set_len(limits.max_archive_bytes)
            .expect("size the sparse artifact");
        finish_archive(output).expect("a supported artifact commits");
        assert_eq!(
            std::fs::metadata(&destination)
                .expect("committed artifact")
                .len(),
            limits.max_archive_bytes
        );
        std::fs::remove_dir_all(root).expect("remove scratch");
    }

    #[cfg(unix)]
    #[test]
    fn a_commit_failure_with_retained_temporary_reports_incomplete_cleanup() {
        use std::os::unix::fs::PermissionsExt;

        let root = artifact_scratch("retained-temporary");
        let destination = root.join("artifact.zip");
        let mut output = AtomicPrivateFile::create(&destination).expect("create private output");
        output
            .file_mut()
            .write_all(b"archive")
            .expect("write artifact");
        // The destination cannot be replaced and the private temporary cannot be removed.
        std::fs::set_permissions(&root, std::fs::Permissions::from_mode(0o500))
            .expect("restrict parent");

        let error = finish_archive(output).expect_err("the commit must fail");
        let message = error.to_string();
        assert!(message.starts_with("archive_write_failed"), "{message}");
        assert!(message.contains("archive_cleanup_incomplete"), "{message}");
        assert!(!destination.exists());

        std::fs::set_permissions(&root, std::fs::Permissions::from_mode(0o700))
            .expect("restore parent");
        std::fs::remove_dir_all(root).expect("remove scratch");
    }

    #[test]
    fn capture_preserves_cleanup_durability_failure_without_claiming_retained_data() {
        let error = cleanup_failure(
            "archive_write_failed",
            CleanupOutcome::DurabilityUnconfirmed(PathBuf::from("fixture")),
        );
        assert!(
            error
                .to_string()
                .contains("archive_cleanup_durability_unconfirmed")
        );
        assert!(!error.to_string().contains("retained"));
    }
}
