//! Restore one archive into a target root.
//!
//! Restore never writes outside its target and never replaces an active root: the
//! caller supplies a disposable destination, inspects the result, and only then
//! promotes it. The destination is used under its resolved name, so a legitimate
//! destination reached through a symbolic link is accepted while every write stays
//! confined to it.
//!
//! The manifest is parsed, grammar-checked and limit-checked before any member is
//! extracted. Inspection and extraction both read the archive through the same bounded
//! decoded stream, the whole raw member list is compared against the declared
//! inventory, and extraction runs in a private staging directory whose name is chosen
//! outside every case-normalized declared path. Publication records each side effect
//! before it happens and rolls back with checked cleanup, so a reported failure leaves
//! the target in its prior recoverable state.

use anyhow::{Result, anyhow, ensure};
use std::collections::BTreeSet;
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};

use crate::core::safe_archive::{
    BoundedStream, ZipExtractionLimits, decoded_tar_gz_budget, decoded_tar_gz_metadata_budget,
    default_zip_extraction_limits, extract_tar_gz_safe, extract_zip_safe, finish_tar_gz,
    tar_gz_decoder, validate_tar_gz_structure, validate_zip_structure,
};
use crate::platform::file_security::{ensure_private_dir, harden_private_path, sync_directory};

use super::inventory::{
    ArchiveManifest, InventoryEntry, InventoryKind, ensure_within_archive_limits,
    inventory_data_root, posix_relative, validate_inventory_structure,
};
use super::{ArchiveContainer, DATA_PREFIX, MANIFEST_MEMBER, RecoveryCoverage, RecoveryLimitation};

/// Staging directory base name. The concrete name is chosen so it cannot equal or
/// contain any declared payload path under the target's case normalization; a payload
/// may legitimately use this name in any letter case.
const STAGING_DIRECTORY_BASE: &str = ".licoup-restore-staging";

#[derive(Clone, Debug)]
pub struct RestoreRequest {
    pub archive_path: PathBuf,
    /// Disposable destination. It is created when absent and must be empty.
    pub target_root: PathBuf,
}

#[derive(Clone, Debug)]
pub struct RestoreOutcome {
    pub container: ArchiveContainer,
    /// The logical data home the archive was captured from. An import into another
    /// home uses it to rebase owner-managed references; it never selects a root.
    pub source_home: PathBuf,
    pub coverage: RecoveryCoverage,
    pub limitations: Vec<RecoveryLimitation>,
    pub file_count: usize,
    pub total_bytes: u64,
}

/// One inspected archive: its validated manifest, the manifest member's own bytes, and
/// every raw member name the container declares.
struct InspectedArchive {
    manifest: ArchiveManifest,
    manifest_member_bytes: u64,
    members: Vec<String>,
}

/// Extract `archive_path` into `target_root` and verify it against its manifest.
pub fn restore_data_root(request: &RestoreRequest) -> Result<RestoreOutcome> {
    restore_data_root_with_preparation(request, |_, _, _| Ok(()))
}

/// Run trusted application-owner preparation on the verified private payload,
/// before publishing any payload entry. The source is logical provenance; the
/// destination is the final resolved home, not the operation's scratch path.
/// Foundation still owns bounds, no-follow inventory, publication and rollback.
pub fn restore_data_root_with_preparation(
    request: &RestoreRequest,
    prepare: impl FnOnce(&Path, &Path, &Path) -> Result<()>,
) -> Result<RestoreOutcome> {
    let container = ArchiveContainer::from_path(&request.archive_path)?;
    let limits = default_zip_extraction_limits();
    let bytes = read_archive_bounded(&request.archive_path, &limits)?;
    // Manifest validation and raw member collection happen before the destination is
    // touched: an archive refused on its own shape leaves the caller's target as it was.
    let inspected = inspect_archive(&bytes, container, &limits)?;

    // Extraction enforces exactly what the manifest declares: its payload plus its own
    // manifest member, and the declared member count.
    let declared_stream_bytes = inspected
        .manifest
        .total_bytes
        .saturating_add(inspected.manifest_member_bytes);
    let declared_member_count = inspected.manifest.entries.len().saturating_add(1);

    let mut created_target_dirs = BTreeSet::new();
    let target_root = prepare_target_root(&request.target_root, &mut created_target_dirs)?;
    // Scratch lives beside the target, outside the target namespace: every declared
    // payload path is target-relative, so no payload name can alias the scratch under
    // any filesystem case or Unicode normalization.
    let staging = create_sibling_scratch(&target_root)?;
    let mut staging_directory = StagingDirectory {
        path: staging.clone(),
        cleaned: false,
    };

    let extraction = match container {
        ArchiveContainer::Zip => extract_zip_safe(
            &bytes,
            &staging,
            ZipExtractionLimits {
                max_total_bytes: declared_stream_bytes,
                max_entries: declared_member_count,
                ..limits
            },
        )
        .map(|_| ()),
        ArchiveContainer::TarGz => extract_tar_gz_safe(
            &bytes,
            &staging,
            Some(declared_stream_bytes),
            Some(declared_member_count),
            Some(limits.max_depth),
        ),
    };
    if extraction.is_err() {
        return Err(staging_directory.fail(anyhow!("archive_extraction_refused")));
    }

    // The whole archive inventory must match the declared members, not only what
    // extraction happened to materialize, and the materialized payload must match the
    // declared kinds and sizes before anything is published.
    if let Err(error) = verify_raw_members(&inspected.members, &inspected.manifest) {
        return Err(staging_directory.fail(error));
    }
    if let Err(error) = verify_payload(&staging, &inspected.manifest.entries) {
        return Err(staging_directory.fail(error));
    }

    let mut publication = PublicationGuard::new(&target_root, created_target_dirs);
    let payload = staging.join("data");
    let preparation = (|| {
        ensure_private_dir(&payload)?;
        prepare(
            &payload,
            Path::new(&inspected.manifest.source_home),
            &target_root,
        )?;
        // Owner edits may change bytes or add owned files, but cannot escape the
        // same bounded regular-file/directory policy applied to an archive.
        let prepared = inventory_data_root(&payload)?;
        let facts = validate_inventory_structure(&prepared)?;
        ensure_within_archive_limits(&facts, 0, "archive_prepared_payload_exceeds_limits")?;
        verify_payload(&staging, &prepared)?;
        Ok::<_, anyhow::Error>(prepared)
    })();
    let prepared = match preparation {
        Ok(prepared) => prepared,
        Err(error) => return Err(publication.fail(staging_directory.fail(error))),
    };
    if let Err(error) = publish_payload(
        &mut publication,
        &target_root,
        &staging,
        &prepared,
        publish_file,
    ) {
        let error = publication.fail(error);
        return Err(staging_directory.fail(error));
    }
    if let Err(error) = publication.sync_changed_directories() {
        let error = publication.fail(error);
        return Err(staging_directory.fail(error));
    }
    // Checked operation-owned scratch removal, then the final root sync that makes the
    // published tree and the removal durable. A cleanup failure rolls the publication
    // back with checked cleanup so the destination is retryable, and both failures are
    // reported instead of letting the guard's Drop swallow one.
    if let Err(error) = staging_directory.cleanup() {
        return Err(publication.fail(error));
    }
    // The scratch is gone: sync the target and the directory that held the scratch so
    // the removal itself is durable, in that deterministic order.
    if let Err(error) = sync_final_directories(&target_root, sync_directory) {
        return Err(publication.fail(error));
    }
    publication.disarm();

    Ok(RestoreOutcome {
        container,
        source_home: PathBuf::from(&inspected.manifest.source_home),
        coverage: inspected.manifest.coverage,
        limitations: inspected.manifest.limitations.clone(),
        file_count: inspected.manifest.file_count(),
        total_bytes: inspected.manifest.total_bytes,
    })
}

/// The destination the caller named, created when absent and returned under its resolved name.
///
/// A directory reached through a symbolic link is the same destination under another
/// name. The operator names the destination, so the owner resolves it instead of
/// refusing a legitimate one whose ancestor chain passes through a link, as the system
/// temporary directory does on macOS. Extraction and publication then use the resolved
/// name, which keeps every write confined to the destination the caller named. A name
/// that does not resolve to exactly one directory stays refused.
fn prepare_target_root(named: &Path, created: &mut BTreeSet<PathBuf>) -> Result<PathBuf> {
    ensure!(!named.as_os_str().is_empty(), "archive_target_invalid");
    if named.exists() {
        ensure!(named.is_dir(), "archive_target_not_directory");
        ensure!(
            fs::read_dir(named)
                .map_err(|_| anyhow!("archive_target_unreadable"))?
                .next()
                .is_none(),
            "archive_target_not_empty"
        );
    } else {
        // Record the missing chain before creating it, so the final sync plan persists
        // the new directory entries and not only their contents.
        let mut missing = Vec::new();
        let mut current = named.to_path_buf();
        while fs::symlink_metadata(&current).is_err() {
            missing.push(current.clone());
            match current.parent() {
                Some(parent) if !parent.as_os_str().is_empty() => current = parent.to_path_buf(),
                _ => break,
            }
        }
        fs::create_dir_all(named).map_err(|_| anyhow!("archive_target_unwritable"))?;
        // Record the created chain in the same resolved namespace the target uses, so a
        // bare relative target never contributes an empty sync path.
        for directory in missing {
            let resolved = directory.canonicalize().unwrap_or(directory);
            created.insert(resolved);
        }
    }
    let resolved = named
        .canonicalize()
        .map_err(|_| anyhow!("archive_target_invalid"))?;
    // The restored data home is private, including a pre-existing empty destination the
    // caller prepared.
    ensure_private_dir(&resolved).map_err(|_| anyhow!("archive_target_unwritable"))?;
    Ok(resolved)
}

/// Sync the target and the directory that held the scratch after its removal.
///
/// The deterministic order is the target first, then the sibling parent, so the removal
/// of the operation's scratch is durable; the seam makes both the ordering and a sync
/// fault observable without a real power loss.
fn sync_final_directories<S>(target: &Path, mut sync: S) -> Result<()>
where
    S: FnMut(&Path) -> Result<()>,
{
    for directory in [Some(target), target.parent()].into_iter().flatten() {
        if directory.as_os_str().is_empty() {
            continue;
        }
        sync(directory).map_err(|_| anyhow!("archive_target_unwritable"))?;
    }
    Ok(())
}

/// Create the operation's scratch directory beside the target root.
///
/// The scratch lives in the target's parent, outside the target namespace: every declared
/// payload path is relative to the target, so no payload name can alias the scratch,
/// regardless of the target filesystem's case or Unicode normalization. The suffix only
/// avoids directory entries that already exist in the parent.
fn create_sibling_scratch(target_root: &Path) -> Result<PathBuf> {
    let parent = target_root
        .parent()
        .ok_or_else(|| anyhow!("archive_target_invalid"))?;
    let mut suffix = 1_u32;
    loop {
        let candidate = parent.join(format!("{STAGING_DIRECTORY_BASE}-{suffix}"));
        match fs::create_dir(&candidate) {
            Ok(()) => {
                if harden_private_path(&candidate).is_err() {
                    let mut scratch = StagingDirectory {
                        path: candidate,
                        cleaned: false,
                    };
                    return Err(scratch.fail(anyhow!("archive_target_unwritable")));
                }
                return Ok(candidate);
            }
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                suffix += 1;
            }
            Err(_) => return Err(anyhow!("archive_target_unwritable")),
        }
    }
}

/// Read the archive only after its own byte limit has been applied to the file.
///
/// The read is additionally capped, so a file that grows after the metadata check
/// cannot make the importer allocate beyond the supported archive size.
fn read_archive_bounded(path: &Path, limits: &ZipExtractionLimits) -> Result<Vec<u8>> {
    let metadata = fs::metadata(path).map_err(|_| anyhow!("archive_unreadable"))?;
    ensure!(metadata.file_type().is_file(), "archive_unreadable");
    ensure!(
        metadata.len() <= limits.max_archive_bytes,
        "archive_extraction_refused"
    );
    let file = fs::File::open(path).map_err(|_| anyhow!("archive_unreadable"))?;
    let mut bytes = Vec::new();
    let mut bounded = file.take(limits.max_archive_bytes.saturating_add(1));
    bounded
        .read_to_end(&mut bytes)
        .map_err(|_| anyhow!("archive_unreadable"))?;
    ensure!(
        bytes.len() as u64 <= limits.max_archive_bytes,
        "archive_extraction_refused"
    );
    Ok(bytes)
}

fn inspect_archive(
    bytes: &[u8],
    container: ArchiveContainer,
    limits: &ZipExtractionLimits,
) -> Result<InspectedArchive> {
    match container {
        ArchiveContainer::Zip => inspect_zip(bytes, limits),
        ArchiveContainer::TarGz => inspect_tar_gz(bytes, limits),
    }
}

fn inspect_zip(bytes: &[u8], limits: &ZipExtractionLimits) -> Result<InspectedArchive> {
    validate_zip_structure(bytes, *limits).map_err(|_| anyhow!("archive_extraction_refused"))?;
    let mut archive = zip::ZipArchive::new(std::io::Cursor::new(bytes))
        .map_err(|_| anyhow!("archive_invalid"))?;
    ensure!(
        archive.len() <= limits.max_entries,
        "archive_extraction_refused"
    );

    let mut member = archive
        .by_name(MANIFEST_MEMBER)
        .map_err(|_| anyhow!("archive_manifest_missing"))?;
    ensure!(
        member.size() <= limits.max_total_bytes,
        "archive_extraction_refused"
    );
    let mut manifest_bytes = Vec::new();
    let mut bounded = (&mut member).take(limits.max_total_bytes.saturating_add(1));
    bounded
        .read_to_end(&mut manifest_bytes)
        .map_err(|_| anyhow!("archive_manifest_unreadable"))?;
    ensure!(
        manifest_bytes.len() as u64 <= limits.max_total_bytes,
        "archive_extraction_refused"
    );
    drop(member);

    let mut members = Vec::with_capacity(archive.len());
    for index in 0..archive.len() {
        let entry = archive
            .by_index(index)
            .map_err(|_| anyhow!("archive_invalid"))?;
        let name = std::str::from_utf8(entry.name_raw()).map_err(|_| anyhow!("archive_invalid"))?;
        members.push(name.trim_end_matches('/').to_string());
    }

    let manifest_member_bytes = manifest_bytes.len() as u64;
    let manifest = ArchiveManifest::from_bytes(&manifest_bytes, ArchiveContainer::Zip.extension())?;
    Ok(InspectedArchive {
        manifest,
        manifest_member_bytes,
        members,
    })
}

/// Find the manifest and collect the whole member inventory in one bounded pass.
///
/// A TAR manifest may follow other members, so the scan itself is the first thing a
/// hostile archive would grow. The physical decoded stream is bounded, one library
/// advance may buffer only the portable header/metadata budget, skipped payloads are
/// drained through the same stream, and the pass ends only after the GZIP trailer is
/// verified.
fn inspect_tar_gz(bytes: &[u8], limits: &ZipExtractionLimits) -> Result<InspectedArchive> {
    // Raw admission first: every TAR header, type and metadata size is bounded before the
    // library parses the stream, so GNU/PAX bodies and sparse descriptors cannot be
    // buffered ahead of the policy refusal.
    validate_tar_gz_structure(
        bytes,
        limits.max_total_bytes,
        limits.max_entries,
        limits.max_depth,
    )
    .map_err(|_| anyhow!("archive_extraction_refused"))?;
    let budget =
        decoded_tar_gz_budget(limits.max_total_bytes, limits.max_entries, limits.max_depth);
    let metadata_budget = decoded_tar_gz_metadata_budget(limits.max_depth);
    let mut stream = BoundedStream::new(tar_gz_decoder(bytes), budget);
    let progress = stream.progress();
    let mut manifest_bytes: Option<Vec<u8>> = None;
    let mut members = Vec::new();
    let mut scan_error: Option<anyhow::Error> = None;
    let mut entry_count = 0_usize;
    {
        let mut archive = tar::Archive::new(&mut stream);
        let mut entries = match archive.entries() {
            Ok(entries) => entries,
            Err(_) => return Err(anyhow!("archive_invalid")),
        };
        loop {
            let before = progress.consumed();
            let Some(entry) = entries.next() else {
                break;
            };
            if progress.consumed().saturating_sub(before) > metadata_budget {
                scan_error = Some(anyhow!("archive_extraction_refused"));
                break;
            }
            entry_count += 1;
            if entry_count > limits.max_entries {
                scan_error = Some(anyhow!("archive_extraction_refused"));
                break;
            }
            let mut entry = match entry {
                Ok(entry) => entry,
                Err(_) => {
                    scan_error = Some(anyhow!("archive_invalid"));
                    break;
                }
            };
            let path = match entry.path() {
                Ok(path) => path,
                Err(_) => {
                    scan_error = Some(anyhow!("archive_invalid"));
                    break;
                }
            };
            let name = match path.to_str() {
                Some(name) => name.trim_end_matches('/').to_string(),
                None => {
                    scan_error = Some(anyhow!("archive_extraction_refused"));
                    break;
                }
            };
            if name == MANIFEST_MEMBER {
                if entry.size() > limits.max_total_bytes {
                    scan_error = Some(anyhow!("archive_extraction_refused"));
                    break;
                }
                let mut buffer = Vec::new();
                let mut bounded = (&mut entry).take(limits.max_total_bytes.saturating_add(1));
                if bounded.read_to_end(&mut buffer).is_err() {
                    scan_error = Some(anyhow!("archive_manifest_unreadable"));
                    break;
                }
                if buffer.len() as u64 > limits.max_total_bytes {
                    scan_error = Some(anyhow!("archive_extraction_refused"));
                    break;
                }
                manifest_bytes = Some(buffer);
            } else if std::io::copy(&mut entry, &mut std::io::sink()).is_err() {
                // Drain each skipped member through the bounded stream so the next
                // advance carries only its own header metadata.
                scan_error = Some(anyhow!("archive_invalid"));
                break;
            }
            members.push(name);
        }
    }
    if stream.exceeded() {
        return Err(anyhow!("archive_extraction_refused"));
    }
    if let Some(error) = scan_error {
        return Err(error);
    }
    // The scan must reach the archive's valid end: the GZIP trailer is verified and
    // trailing material is refused, exactly as extraction does.
    finish_tar_gz(stream, bytes.len()).map_err(|_| anyhow!("archive_extraction_refused"))?;
    let manifest_bytes = manifest_bytes.ok_or_else(|| anyhow!("archive_manifest_missing"))?;
    let manifest_member_bytes = manifest_bytes.len() as u64;
    let manifest =
        ArchiveManifest::from_bytes(&manifest_bytes, ArchiveContainer::TarGz.extension())?;
    Ok(InspectedArchive {
        manifest,
        manifest_member_bytes,
        members,
    })
}

/// The raw member list must be exactly the manifest member plus every declared member.
///
/// This catches undeclared top-level members and container-level duplicates or case
/// aliases that extraction would collapse into one filesystem path.
fn verify_raw_members(members: &[String], manifest: &ArchiveManifest) -> Result<()> {
    let mut expected = BTreeSet::new();
    expected.insert(MANIFEST_MEMBER.to_string());
    for entry in &manifest.entries {
        expected.insert(format!("{DATA_PREFIX}{}", entry.path));
    }
    let mut exact = BTreeSet::new();
    let mut folded = BTreeSet::new();
    for member in members {
        let member = member.trim_end_matches('/');
        ensure!(
            exact.insert(member.to_string()),
            "archive_inventory_duplicate"
        );
        ensure!(
            folded.insert(member.to_lowercase()),
            "archive_inventory_case_collision"
        );
        ensure!(expected.contains(member), "archive_inventory_mismatch");
    }
    for name in &expected {
        ensure!(exact.contains(name.as_str()), "archive_inventory_mismatch");
    }
    Ok(())
}

/// Every declared member exists with the declared kind and size, and no undeclared
/// member is present under the payload root. A legitimately empty data home carries no
/// payload members.
fn verify_payload(staging: &Path, entries: &[InventoryEntry]) -> Result<()> {
    let payload = staging.join("data");
    if entries.is_empty() {
        return match fs::symlink_metadata(&payload) {
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Ok(metadata) if metadata.file_type().is_dir() => {
                ensure!(
                    fs::read_dir(&payload)
                        .map_err(|_| anyhow!("archive_inventory_mismatch"))?
                        .next()
                        .is_none(),
                    "archive_inventory_mismatch"
                );
                Ok(())
            }
            _ => Err(anyhow!("archive_inventory_mismatch")),
        };
    }
    ensure!(payload.is_dir(), "archive_payload_missing");
    for entry in entries {
        let path = payload.join(&entry.path);
        let metadata =
            fs::symlink_metadata(&path).map_err(|_| anyhow!("archive_inventory_mismatch"))?;
        ensure!(
            !metadata.file_type().is_symlink(),
            "archive_inventory_mismatch"
        );
        match entry.kind {
            InventoryKind::Directory => {
                ensure!(metadata.file_type().is_dir(), "archive_inventory_mismatch");
            }
            InventoryKind::File => {
                ensure!(metadata.file_type().is_file(), "archive_inventory_mismatch");
                ensure!(metadata.len() == entry.size, "archive_inventory_mismatch");
            }
        }
    }
    let mut actual = Vec::new();
    collect_payload_paths(&payload, &payload, &mut actual)?;
    let declared: BTreeSet<&str> = entries.iter().map(|entry| entry.path.as_str()).collect();
    ensure!(
        actual.iter().all(|path| declared.contains(path.as_str())),
        "archive_inventory_mismatch"
    );
    Ok(())
}

fn collect_payload_paths(root: &Path, directory: &Path, paths: &mut Vec<String>) -> Result<()> {
    for entry in fs::read_dir(directory).map_err(|_| anyhow!("archive_inventory_mismatch"))? {
        let entry = entry.map_err(|_| anyhow!("archive_inventory_mismatch"))?;
        let child = entry.path();
        let metadata =
            fs::symlink_metadata(&child).map_err(|_| anyhow!("archive_inventory_mismatch"))?;
        ensure!(
            !metadata.file_type().is_symlink(),
            "archive_inventory_mismatch"
        );
        let relative = child
            .strip_prefix(root)
            .map_err(|_| anyhow!("archive_inventory_mismatch"))?;
        let path = posix_relative(relative).map_err(|_| anyhow!("archive_inventory_mismatch"))?;
        if metadata.file_type().is_dir() {
            paths.push(path);
            collect_payload_paths(root, &child, paths)?;
        } else {
            ensure!(metadata.file_type().is_file(), "archive_inventory_mismatch");
            paths.push(path);
        }
    }
    Ok(())
}

/// Publish the verified payload into the target, recording every side effect it may
/// create before the effect happens.
///
/// Implicit parent directories are recorded and created here, so a failure at any point
/// — including after a rename or after private-path hardening — can roll back exactly the
/// operation's own output, and every created directory's parent entry is included in the
/// durability sync plan.
fn publish_payload<F>(
    guard: &mut PublicationGuard<'_>,
    target: &Path,
    staging: &Path,
    entries: &[InventoryEntry],
    mut publish_file: F,
) -> Result<()>
where
    F: FnMut(&Path, &Path) -> Result<()>,
{
    let extracted = staging.join("data");
    let mut protected = Vec::new();
    for entry in entries {
        if fs::symlink_metadata(extracted.join(&entry.path))?
            .permissions()
            .readonly()
        {
            protected.push(target.join(&entry.path));
        }
    }
    for entry in entries {
        let destination = target.join(&entry.path);
        let mut ancestor = destination.parent();
        while let Some(path) = ancestor {
            if path == target {
                break;
            }
            guard.create_private_directory(path)?;
            ancestor = path.parent();
        }
        guard.record(destination.clone(), entry.kind == InventoryKind::Directory);
        match entry.kind {
            InventoryKind::Directory => guard.create_private_directory(&destination)?,
            InventoryKind::File => {
                let source = extracted.join(&entry.path);
                // Moving a frozen revision member also needs write permission on
                // its operation-owned source directory. Restore protection only
                // after the complete publication; the recorded modes above win.
                if let Some(parent) = source.parent() {
                    make_operation_path_writable(parent)?;
                }
                make_operation_path_writable(&source)?;
                publish_file(&source, &destination)
                    .map_err(|_| anyhow!("archive_target_unwritable"))?;
            }
        }
    }
    // Publish privately while parents are writable; then restore protections the
    // owner verified in staging, deepest first. Archive bytes do not grant modes.
    protected.sort_by_key(|path| std::cmp::Reverse(path.components().count()));
    for path in protected {
        let mut permissions = fs::symlink_metadata(&path)?.permissions();
        permissions.set_readonly(true);
        fs::set_permissions(&path, permissions)?;
    }
    Ok(())
}

/// Publish one verified file privately: rename it into place and harden its permissions.
fn publish_file(source: &Path, destination: &Path) -> Result<()> {
    fs::rename(source, destination)?;
    harden_private_path(destination)?;
    Ok(())
}

/// Records every path publication may create and rolls them back with checked cleanup.
struct PublicationGuard<'a> {
    target: &'a Path,
    published: Vec<(PathBuf, bool)>,
    created: BTreeSet<PathBuf>,
    armed: bool,
}

impl<'a> PublicationGuard<'a> {
    fn new(target: &'a Path, created: BTreeSet<PathBuf>) -> Self {
        Self {
            target,
            published: Vec::new(),
            created,
            armed: true,
        }
    }

    fn record(&mut self, path: PathBuf, directory: bool) {
        self.published.push((path, directory));
    }

    #[cfg(test)]
    fn record_created(&mut self, path: PathBuf) {
        self.created.insert(path);
    }

    fn disarm(&mut self) {
        self.armed = false;
    }

    /// Create a missing directory chain privately, recording every directory this
    /// operation creates so its parent entry can be synced.
    fn create_private_directory(&mut self, path: &Path) -> Result<()> {
        self.create_missing_chain(path)?;
        harden_private_path(path).map_err(|_| anyhow!("archive_target_unwritable"))
    }

    fn create_missing_chain(&mut self, path: &Path) -> Result<()> {
        if fs::symlink_metadata(path).is_ok() {
            return Ok(());
        }
        if let Some(parent) = path.parent() {
            if !parent.as_os_str().is_empty() {
                self.create_missing_chain(parent)?;
            }
        }
        fs::create_dir(path).map_err(|_| anyhow!("archive_target_unwritable"))?;
        self.created.insert(path.to_path_buf());
        Ok(())
    }

    /// Every directory whose entries this operation changed: published directories,
    /// parents of published entries, parents of created directories, the target and the
    /// target's parent (which held the scratch).
    fn sync_plan(&self) -> BTreeSet<PathBuf> {
        let mut directories = BTreeSet::new();
        directories.insert(self.target.to_path_buf());
        if let Some(parent) = self.target.parent() {
            directories.insert(parent.to_path_buf());
        }
        for (path, directory) in &self.published {
            if *directory {
                directories.insert(path.clone());
            }
            if let Some(parent) = path.parent() {
                if !parent.as_os_str().is_empty() {
                    directories.insert(parent.to_path_buf());
                }
            }
        }
        for path in &self.created {
            if let Some(parent) = path.parent() {
                if !parent.as_os_str().is_empty() {
                    directories.insert(parent.to_path_buf());
                }
            }
        }
        directories
    }

    /// Sync the plan deepest first through the injected sync, so a fault is reported and
    /// ordering is observable in synthetic seams without a real power loss.
    fn sync_directories_with<S>(&self, mut sync: S) -> Result<()>
    where
        S: FnMut(&Path) -> Result<()>,
    {
        let mut ordered: Vec<PathBuf> = self.sync_plan().into_iter().collect();
        ordered.sort_by_key(|path| std::cmp::Reverse(path.components().count()));
        for directory in ordered {
            if directory.as_os_str().is_empty() {
                continue;
            }
            sync(&directory).map_err(|_| anyhow!("archive_target_unwritable"))?;
        }
        Ok(())
    }

    fn sync_changed_directories(&self) -> Result<()> {
        self.sync_directories_with(sync_directory)
    }

    /// Remove every recorded path and sync the directories that held them; report the
    /// first step that did not complete.
    ///
    /// Files are removed first, then directories deepest first, so a parent directory is
    /// never seen non-empty because of a child this rollback still has to visit. The
    /// syncs persist the removals, so a later retry starts from the same directory state
    /// the operation inherited.
    fn rollback_with<S>(&mut self, mut sync: S) -> Result<()>
    where
        S: FnMut(&Path) -> Result<()>,
    {
        self.armed = false;
        let mut failure: Option<anyhow::Error> = None;
        let mut files = Vec::new();
        let mut directories = BTreeSet::new();
        for (path, directory) in &self.published {
            // Rollback removes only paths this operation published below the target.
            if !path.starts_with(self.target) {
                continue;
            }
            if *directory {
                directories.insert(path.clone());
            } else {
                files.push(path.clone());
            }
        }
        // Directories this operation created below the target are its own output too,
        // even when they were only implicit parents of a published member.
        for path in &self.created {
            if path.starts_with(self.target) {
                directories.insert(path.clone());
            }
        }
        let mut ordered: Vec<PathBuf> = directories.into_iter().collect();
        ordered.sort_by_key(|path| std::cmp::Reverse(path.components().count()));
        let remove = |result: std::io::Result<()>, failure: &mut Option<anyhow::Error>| match result
        {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => {
                if failure.is_none() {
                    *failure = Some(error.into());
                }
            }
        };
        for path in &files {
            if let Some(parent) = path.parent() {
                let _ = make_operation_path_writable(parent);
            }
            let _ = make_operation_path_writable(path);
            remove(fs::remove_file(path), &mut failure);
        }
        for path in &ordered {
            if let Some(parent) = path.parent() {
                // Do not alter an unowned ancestor above the selected target.
                if parent.starts_with(self.target) {
                    let _ = make_operation_path_writable(parent);
                }
            }
            let _ = make_operation_path_writable(path);
            remove(fs::remove_dir(path), &mut failure);
        }
        if failure.is_none() {
            let mut changed = self.sync_plan();
            changed.insert(self.target.to_path_buf());
            let mut deepest: Vec<PathBuf> = changed.into_iter().collect();
            deepest.sort_by_key(|path| std::cmp::Reverse(path.components().count()));
            for directory in deepest {
                // Directories this rollback removed no longer exist; their surviving
                // parents are the entries that need the sync.
                if fs::symlink_metadata(&directory).is_err() {
                    continue;
                }
                match sync(&directory) {
                    Ok(()) => {}
                    Err(error) => {
                        if failure.is_none() {
                            failure = Some(error);
                        }
                        break;
                    }
                }
            }
        }
        match failure {
            Some(error) => Err(error),
            None => Ok(()),
        }
    }

    fn rollback(&mut self) -> Result<()> {
        self.rollback_with(sync_directory)
    }

    /// Return the publication failure after the checked rollback, naming any cleanup
    /// that could not complete instead of swallowing it.
    fn fail(&mut self, error: anyhow::Error) -> anyhow::Error {
        match self.rollback() {
            Ok(()) => error,
            Err(rollback) => anyhow!("{error}; archive_target_rollback_incomplete: {rollback}"),
        }
    }
}

impl Drop for PublicationGuard<'_> {
    fn drop(&mut self) {
        if self.armed {
            // Last resort for panics only; every normal failure path rolls back explicitly.
            let _ = self.rollback();
        }
    }
}

/// Removes the staging directory on every path; it is operation-owned scratch.
struct StagingDirectory {
    path: PathBuf,
    cleaned: bool,
}

impl StagingDirectory {
    /// Checked removal used on the success path and on every handled failure. A failure
    /// names the operation-owned scratch that remains, so the caller can recover it.
    fn cleanup(&mut self) -> Result<()> {
        self.cleanup_with(sync_directory)
    }

    fn cleanup_with<S>(&mut self, mut sync: S) -> Result<()>
    where
        S: FnMut(&Path) -> Result<()>,
    {
        self.cleaned = true;
        let removal = (|| {
            let metadata = fs::symlink_metadata(&self.path)?;
            ensure!(
                metadata.is_dir() && !metadata.file_type().is_symlink(),
                "operation scratch type changed"
            );
            remove_operation_tree(&self.path)
        })();
        removal.map_err(|error| {
            anyhow!(
                "archive_target_cleanup_failed: {} could not be removed: {error}",
                self.path.display()
            )
        })?;
        let parent = self
            .path
            .parent()
            .ok_or_else(|| anyhow!("archive_target_invalid"))?;
        sync(parent).map_err(|_| anyhow!("archive_target_cleanup_durability_unconfirmed"))
    }

    /// Return the failure after the checked cleanup, naming cleanup that did not
    /// complete instead of swallowing it.
    fn fail(&mut self, error: anyhow::Error) -> anyhow::Error {
        match self.cleanup() {
            Ok(()) => error,
            Err(cleanup) => anyhow!("{error}; archive_target_cleanup_incomplete: {cleanup}"),
        }
    }
}

impl Drop for StagingDirectory {
    fn drop(&mut self) {
        if !self.cleaned {
            // Last resort for panics only; every normal failure path cleans up explicitly.
            let _ = self.cleanup();
        }
    }
}

/// Only operation-owned paths are passed here. Never change a symlink referent.
fn make_operation_path_writable(path: &Path) -> Result<()> {
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error.into()),
    };
    if !metadata.file_type().is_symlink() && metadata.permissions().readonly() {
        harden_private_path(path)?;
        #[cfg(not(unix))]
        {
            let mut permissions = fs::symlink_metadata(path)?.permissions();
            permissions.set_readonly(false);
            fs::set_permissions(path, permissions)?;
        }
    }
    Ok(())
}

fn remove_operation_tree(path: &Path) -> Result<()> {
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error.into()),
    };
    if metadata.is_dir() && !metadata.file_type().is_symlink() {
        make_operation_path_writable(path)?;
        for entry in fs::read_dir(path)? {
            remove_operation_tree(&entry?.path())?;
        }
        fs::remove_dir(path)?;
    } else {
        make_operation_path_writable(path)?;
        fs::remove_file(path)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn file_entry(path: &str, size: u64) -> InventoryEntry {
        InventoryEntry {
            path: path.to_string(),
            kind: InventoryKind::File,
            size,
        }
    }

    fn directory_entry(path: &str) -> InventoryEntry {
        InventoryEntry {
            path: path.to_string(),
            kind: InventoryKind::Directory,
            size: 0,
        }
    }

    fn scratch(name: &str) -> PathBuf {
        let root = std::env::temp_dir().join(format!("lico-restore-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).expect("create scratch");
        root
    }

    #[test]
    fn publication_rollback_removes_prepared_readonly_members_and_reports_sync_failure() {
        for created in [false, true] {
            let root = scratch(if created {
                "prepared-rollback-created"
            } else {
                "prepared-rollback-existing"
            });
            let staging = root.join("staging");
            let target = root.join("target");
            fs::create_dir_all(staging.join("data/revision")).unwrap();
            fs::create_dir(&target).unwrap();
            fs::write(staging.join("data/revision/member"), b"x").unwrap();
            for relative in ["data/revision/member", "data/revision"] {
                let path = staging.join(relative);
                let mut permissions = fs::metadata(&path).unwrap().permissions();
                permissions.set_readonly(true);
                fs::set_permissions(path, permissions).unwrap();
            }
            let mut directories = BTreeSet::new();
            if created {
                directories.insert(target.clone());
            }
            let mut guard = PublicationGuard::new(&target, directories);
            publish_payload(
                &mut guard,
                &target,
                &staging,
                &[
                    directory_entry("revision"),
                    file_entry("revision/member", 1),
                ],
                publish_file,
            )
            .unwrap();
            assert!(
                fs::metadata(target.join("revision"))
                    .unwrap()
                    .permissions()
                    .readonly()
            );
            let error = guard
                .rollback_with(|_| Err(anyhow!("synthetic rollback sync failure")))
                .unwrap_err();
            assert_eq!(error.to_string(), "synthetic rollback sync failure");
            assert_eq!(target.exists(), !created);
            if !created {
                assert_eq!(fs::read_dir(&target).unwrap().count(), 0);
            }
            remove_operation_tree(&root).unwrap();
        }
    }

    #[test]
    fn scratch_is_created_beside_the_target_and_never_inside_it() {
        let root = scratch("scratch-sibling");
        let target = root.join("restored");
        fs::create_dir(&target).expect("create target");
        fs::create_dir(root.join(format!("{STAGING_DIRECTORY_BASE}-1"))).expect("existing sibling");
        let staging = create_sibling_scratch(&target).expect("create scratch");
        assert_eq!(staging.parent(), Some(root.as_path()));
        assert!(!staging.starts_with(&target));
        assert_eq!(
            staging.file_name().expect("scratch name"),
            format!("{STAGING_DIRECTORY_BASE}-2").as_str()
        );
        fs::remove_dir_all(root).expect("remove scratch");
    }

    #[test]
    fn sync_plan_lists_changed_directories_deepest_first_and_tracks_created_parents() {
        let target = PathBuf::from("/fixture/restored");
        let mut guard = PublicationGuard::new(&target, BTreeSet::new());
        guard.record(target.join("a/b.txt"), false);
        guard.record(target.join("a"), true);
        guard.record_created(target.join("a"));
        let mut seen = Vec::new();
        guard
            .sync_directories_with(|directory| {
                seen.push(directory.to_path_buf());
                Ok(())
            })
            .expect("sync plan runs");
        let depths: Vec<usize> = seen.iter().map(|path| path.components().count()).collect();
        assert!(
            depths.windows(2).all(|window| window[0] >= window[1]),
            "{seen:?}"
        );
        assert!(seen.contains(&target.join("a")), "{seen:?}");
        assert!(seen.contains(&PathBuf::from("/fixture")), "{seen:?}");
    }

    #[test]
    fn a_directory_sync_fault_is_reported() {
        let target = PathBuf::from("/fixture/restored");
        let guard = PublicationGuard::new(&target, BTreeSet::new());
        let error = guard
            .sync_directories_with(|_| Err(anyhow!("injected sync fault")))
            .expect_err("sync fault is reported");
        assert_eq!(error.to_string(), "archive_target_unwritable");
    }

    #[test]
    fn raw_member_inventory_must_match_the_declared_members_exactly() {
        let manifest = ArchiveManifest::new(
            "zip",
            Path::new("/synthetic/source-home"),
            RecoveryCoverage::Limited,
            Vec::new(),
            vec![file_entry("a.txt", 1), directory_entry("d")],
        )
        .expect("manifest");
        let exact = vec![
            MANIFEST_MEMBER.to_string(),
            "data/a.txt".to_string(),
            "data/d".to_string(),
        ];
        assert!(verify_raw_members(&exact, &manifest).is_ok());
        assert!(verify_raw_members(&[MANIFEST_MEMBER.to_string()], &manifest).is_err());

        let mut undeclared = exact.clone();
        undeclared.push("escaped.bin".to_string());
        assert!(verify_raw_members(&undeclared, &manifest).is_err());

        let mut duplicate = exact.clone();
        duplicate.push("data/d/".to_string());
        assert_eq!(
            verify_raw_members(&duplicate, &manifest)
                .expect_err("duplicate member")
                .to_string(),
            "archive_inventory_duplicate"
        );

        let mut aliased = exact.clone();
        aliased.push("data/A.TXT".to_string());
        assert_eq!(
            verify_raw_members(&aliased, &manifest)
                .expect_err("case alias")
                .to_string(),
            "archive_inventory_case_collision"
        );
    }

    #[test]
    fn publication_failure_after_a_rename_rolls_back_the_file_and_its_parent() {
        let root = scratch("publication-after-rename");
        fs::create_dir_all(root.join("staging/data/a")).expect("create staging");
        fs::write(root.join("staging/data/a/first.txt"), b"first").expect("first");
        let entries = vec![file_entry("a/first.txt", 5)];
        let mut guard = PublicationGuard::new(&root, BTreeSet::new());
        let result = publish_payload(
            &mut guard,
            &root,
            &root.join("staging"),
            &entries,
            |source, destination| {
                fs::rename(source, destination)?;
                // Simulates a failure after the rename, such as private-path hardening.
                Err(anyhow!("injected post-rename failure"))
            },
        );
        let error = guard.fail(result.expect_err("publication must fail"));
        assert_eq!(error.to_string(), "archive_target_unwritable");
        assert!(
            !root.join("a").exists(),
            "rollback removes the published file and its implicit parent"
        );
        drop(guard);
        fs::remove_dir_all(root).expect("remove scratch");
    }

    #[test]
    fn rollback_reports_a_cleanup_sync_fault() {
        let root = scratch("publication-sync-fault");
        fs::create_dir_all(root.join("staging/data/a")).expect("create staging");
        fs::write(root.join("staging/data/a/first.txt"), b"first").expect("first");
        let entries = vec![file_entry("a/first.txt", 5)];
        let mut guard = PublicationGuard::new(&root, BTreeSet::new());
        let result = publish_payload(
            &mut guard,
            &root,
            &root.join("staging"),
            &entries,
            |source, destination| {
                fs::rename(source, destination)?;
                Err(anyhow!("injected post-rename failure"))
            },
        );
        assert!(result.is_err());
        let error = guard
            .rollback_with(|_| Err(anyhow!("injected cleanup sync fault")))
            .expect_err("cleanup sync fault is reported");
        assert!(
            error.to_string().contains("injected cleanup sync fault"),
            "{error}"
        );
        assert!(!root.join("a").exists());
        drop(guard);
        fs::remove_dir_all(root).expect("remove scratch");
    }

    #[test]
    fn final_sync_orders_target_then_scratch_parent_and_reports_faults() {
        let target = PathBuf::from("/fixture/restored");
        let mut seen = Vec::new();
        sync_final_directories(&target, |directory| {
            seen.push(directory.to_path_buf());
            Ok(())
        })
        .expect("final sync runs");
        assert_eq!(seen, vec![target.clone(), PathBuf::from("/fixture")]);

        let error = sync_final_directories(&target, |_| Err(anyhow!("injected final sync fault")))
            .expect_err("a final sync fault is reported");
        assert_eq!(error.to_string(), "archive_target_unwritable");
    }

    #[test]
    fn a_failed_scratch_removal_is_reported_and_names_the_leftover() {
        let root = scratch("staging-removal-failure");
        let path = root.join("not-a-directory");
        fs::write(&path, b"file").expect("fixture");
        let mut staging = StagingDirectory {
            path: path.clone(),
            cleaned: false,
        };
        let error = staging.cleanup().expect_err("removal fails");
        assert!(
            error.to_string().contains("archive_target_cleanup_failed"),
            "{error}"
        );
        assert!(
            error.to_string().contains(&path.display().to_string()),
            "{error}"
        );
        fs::remove_dir_all(root).expect("remove scratch");
    }

    #[test]
    fn failed_operation_scratch_cleanup_syncs_after_removal_and_reports_sync_fault() {
        let root = scratch("staging-cleanup-sync");
        let path = root.join("scratch");
        fs::create_dir(&path).unwrap();
        let mut staging = StagingDirectory {
            path: path.clone(),
            cleaned: false,
        };
        let error = staging
            .cleanup_with(|parent| {
                assert_eq!(parent, root);
                assert!(
                    !path.exists(),
                    "sync must follow removal on failure paths too"
                );
                Err(anyhow!("injected parent sync fault"))
            })
            .unwrap_err();
        assert_eq!(
            error.to_string(),
            "archive_target_cleanup_durability_unconfirmed"
        );
        assert!(staging.cleaned);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn rollback_reports_state_it_cannot_remove() {
        let root = scratch("publication-unremovable");
        fs::create_dir_all(root.join("staging/data/a")).expect("create staging");
        fs::write(root.join("staging/data/a/first.txt"), b"first").expect("first");
        let entries = vec![file_entry("a/first.txt", 5)];
        let mut guard = PublicationGuard::new(&root, BTreeSet::new());
        let result = publish_payload(
            &mut guard,
            &root,
            &root.join("staging"),
            &entries,
            |source, destination| {
                fs::rename(source, destination)?;
                // A file the operation never recorded keeps the directory non-empty.
                fs::write(root.join("a/foreign.txt"), b"foreign")?;
                Err(anyhow!("injected post-rename failure"))
            },
        );
        let error = guard.fail(result.expect_err("publication must fail"));
        assert!(
            error
                .to_string()
                .contains("archive_target_rollback_incomplete"),
            "{error}"
        );
        drop(guard);
        fs::remove_dir_all(root).expect("remove scratch");
    }
}
