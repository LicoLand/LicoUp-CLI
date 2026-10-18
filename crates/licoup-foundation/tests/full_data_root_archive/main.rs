//! Full data-root archive acceptance.
//!
//! Positive adversarial coverage for the whole owner:
//!
//! 1. both plaintext containers carry the same logical payload and restore every
//!    declared member, while an independent canary copy proves the source is untouched;
//! 2. the manifest is validated — portable POSIX grammar, container, totals, aliases,
//!    structure — before any member is written;
//! 3. Windows-style escapes, duplicate or case-colliding aliases, structural conflicts,
//!    container mismatches, undeclared payload and missing or resized members are refused
//!    without writing outside the destination;
//! 4. a payload that legitimately uses the staging directory name survives in both
//!    containers, and a failed restore leaves the destination retryable;
//! 5. export refuses a home its own importer could not accept (depth, count, portable
//!    names), captures privately and atomically, and never follows a dangling destination
//!    symlink or truncates a hard-linked source alias;
//! 6. archive and manifest reads are bounded before decompression, and a legitimately
//!    empty home round-trips.
//!
//! Everything here runs on synthetic roots under a disposable directory. No real data
//! root, operating-system credential prompt, platform key or installed client is touched.

use std::collections::BTreeMap;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

use licoup_foundation::core::full_data_root_archive::{
    ARCHIVE_LAYOUT, ArchiveContainer, ExportOutcome, ExportRequest, RecoveryCoverage,
    RestoreRequest, export_data_root, restore_data_root,
};

mod owner_preparation;
mod transport_integrity;

/// The manifest member every archive carries first. The owner's own constant is crate
/// private, so these fixtures name the fixed member explicitly.
const MANIFEST_MEMBER: &str = "licoup-data-root.json";

/// The credential inventory document, at the data-root-relative path its owner reads.
const CREDENTIAL_INVENTORY: &str = "llm-api-key-inventory.json";

fn scratch(name: &str) -> PathBuf {
    // A canonical base for the disposable roots under test. The owner resolves a named
    // destination through its existing ancestors, so a base reached through a system
    // link is usable; resolving it once here keeps each fixture's own path stable.
    let base = std::env::temp_dir()
        .canonicalize()
        .expect("canonical temporary directory");
    let root = base.join(format!("licoup-full-archive-{name}-{}", std::process::id()));
    if root.exists() {
        fs::remove_dir_all(&root).expect("clear scratch root");
    }
    fs::create_dir_all(&root).expect("create scratch root");
    root
}

/// A synthetic complete root: canonical conversation database, settings, package
/// records and the credential inventory at the path its owner reads.
fn synthetic_data_root(name: &str) -> PathBuf {
    let root = scratch(name);
    write(
        &root.join("client-state/conversations/conversations.sqlite3"),
        b"SQLite format 3\0synthetic canonical store",
    );
    write(
        &root.join("client-state/conversations/migration-v5.complete"),
        b"v5",
    );
    write(
        &root.join("client-state/appearance-preferences.json"),
        br#"{"theme":"dark"}"#,
    );
    write(
        &root.join("client-state/agent-tool-allowlists.json"),
        br#"{"allow":[]}"#,
    );
    write(
        &root.join(CREDENTIAL_INVENTORY),
        br#"{"providers":["synthetic-exportable"]}"#,
    );
    write(&root.join(".licoup-workspace.json"), br#"{"revision":1}"#);
    write(
        &root.join("adaptive-flywheel.toml"),
        b"[flywheel]\nrevision=1\n",
    );
    write(
        &root.join("group-conversations/group-1.json"),
        br#"{"id":"group-1"}"#,
    );
    root
}

fn write(path: &Path, bytes: &[u8]) {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).expect("create parent");
    }
    fs::write(path, bytes).expect("write fixture");
}

fn read(path: &Path) -> Vec<u8> {
    fs::read(path).expect("read fixture")
}

/// An independent, byte-for-byte copy of a root used as the preservation oracle.
fn copy_root(source: &Path, destination: &Path) {
    fs::create_dir_all(destination).expect("create canary");
    for entry in fs::read_dir(source).expect("read canary source") {
        let entry = entry.expect("canary entry");
        let metadata = fs::symlink_metadata(entry.path()).expect("canary metadata");
        if metadata.file_type().is_symlink() {
            continue;
        }
        let target = destination.join(entry.file_name());
        if metadata.is_dir() {
            copy_root(&entry.path(), &target);
        } else {
            fs::copy(entry.path(), &target).expect("copy canary member");
        }
    }
}

/// The complete logical payload of a root: every regular file's bytes and every
/// directory, keyed by relative path. Symbolic links are absent by construction.
fn payload(root: &Path) -> BTreeMap<String, Option<Vec<u8>>> {
    let mut map = BTreeMap::new();
    collect_payload(root, root, &mut map);
    map
}

fn collect_payload(root: &Path, directory: &Path, map: &mut BTreeMap<String, Option<Vec<u8>>>) {
    for entry in fs::read_dir(directory).expect("read payload") {
        let entry = entry.expect("payload entry");
        let metadata = fs::symlink_metadata(entry.path()).expect("payload metadata");
        if metadata.file_type().is_symlink() {
            continue;
        }
        let relative = entry
            .path()
            .strip_prefix(root)
            .expect("payload relative path")
            .to_string_lossy()
            .replace('\\', "/");
        if metadata.is_dir() {
            map.insert(relative, None);
            collect_payload(root, &entry.path(), map);
        } else {
            map.insert(
                relative,
                Some(fs::read(entry.path()).expect("payload bytes")),
            );
        }
    }
}

fn export(root: &Path, archive: &Path) -> ExportOutcome {
    export_data_root(&ExportRequest {
        data_root: root.to_path_buf(),
        archive_path: archive.to_path_buf(),
        writers_stopped: true,
    })
    .expect("export succeeds")
}

fn restore(
    archive: &Path,
    target: &Path,
) -> licoup_foundation::core::full_data_root_archive::RestoreOutcome {
    restore_data_root(&RestoreRequest {
        archive_path: archive.to_path_buf(),
        target_root: target.to_path_buf(),
    })
    .expect("restore succeeds")
}

/// One refusal assertion: the operation must fail with exactly the owner's typed code.
fn refusal<T: std::fmt::Debug>(result: anyhow::Result<T>, code: &str) {
    let error = result.expect_err("operation must be refused");
    assert_eq!(error.to_string(), code);
}

fn entry_json(path: &str, kind: &str, size: u64) -> serde_json::Value {
    serde_json::json!({ "path": path, "kind": kind, "size": size })
}

fn manifest_json(container: &str, entries: Vec<serde_json::Value>) -> serde_json::Value {
    let total_bytes: u64 = entries
        .iter()
        .filter(|entry| entry["kind"] == "file")
        .map(|entry| entry["size"].as_u64().unwrap_or(0))
        .sum();
    serde_json::json!({
        "layout": ARCHIVE_LAYOUT,
        "container": container,
        "source_home": "/synthetic/source-home",
        "created_at_unix": 0,
        "coverage": "limited",
        "limitations": [],
        "entries": entries,
        "total_bytes": total_bytes,
    })
}

fn write_zip_fixture(path: &Path, manifest: &serde_json::Value, members: &[(&str, &[u8])]) {
    let mut writer = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
    let options = zip::write::SimpleFileOptions::default()
        .compression_method(zip::CompressionMethod::Deflated);
    writer
        .start_file(MANIFEST_MEMBER, options)
        .expect("add manifest");
    writer
        .write_all(&serde_json::to_vec(manifest).expect("encode manifest"))
        .expect("write manifest");
    for (name, data) in members {
        writer.start_file(*name, options).expect("add member");
        writer.write_all(data).expect("write member");
    }
    let bytes = writer.finish().expect("finish zip").into_inner();
    fs::write(path, bytes).expect("write archive fixture");
}

fn tar_gz_bytes(
    manifest: &serde_json::Value,
    members: &[(&str, &[u8], tar::EntryType)],
    members_before_manifest: usize,
) -> Vec<u8> {
    let mut tar_bytes = Vec::new();
    {
        let mut builder = tar::Builder::new(&mut tar_bytes);
        for index in 0..members_before_manifest {
            append_tar_member(
                &mut builder,
                &format!("data/padding-{index:05}/"),
                tar::EntryType::Directory,
                &[],
                None,
            );
        }
        let manifest_bytes = serde_json::to_vec(manifest).expect("encode manifest");
        append_tar_member(
            &mut builder,
            MANIFEST_MEMBER,
            tar::EntryType::Regular,
            &manifest_bytes,
            None,
        );
        for (name, data, entry_type) in members {
            append_tar_member(&mut builder, name, *entry_type, data, None);
        }
        builder.finish().expect("finish tar");
    }
    let mut gz_bytes = Vec::new();
    {
        let mut encoder =
            flate2::write::GzEncoder::new(&mut gz_bytes, flate2::Compression::default());
        encoder.write_all(&tar_bytes).expect("compress payload");
        encoder.finish().expect("finish gzip");
    }
    gz_bytes
}

fn write_tar_gz_fixture(
    path: &Path,
    manifest: &serde_json::Value,
    members: &[(&str, &[u8], tar::EntryType)],
) {
    fs::write(path, tar_gz_bytes(manifest, members, 0)).expect("write archive fixture");
}

fn append_tar_member<W: Write>(
    builder: &mut tar::Builder<W>,
    name: &str,
    entry_type: tar::EntryType,
    data: &[u8],
    link: Option<&str>,
) {
    let mut header = tar::Header::new_gnu();
    header.set_entry_type(entry_type);
    header.set_size(data.len() as u64);
    header.set_mode(if entry_type == tar::EntryType::Directory {
        0o700
    } else {
        0o600
    });
    if let Some(link) = link {
        header.set_link_name(link).expect("set link name");
    }
    header.set_cksum();
    builder
        .append_data(&mut header, name, data)
        .expect("append member");
}

#[test]
fn both_containers_restore_equivalent_application_facts() {
    let source = synthetic_data_root("roundtrip");
    let canary = scratch("roundtrip-canary");
    copy_root(&source, &canary);
    let work = scratch("roundtrip-out");

    let zip_archive = work.join("complete.zip");
    let tar_archive = work.join("complete.tar.gz");

    let zip_outcome = export(&source, &zip_archive);
    let tar_outcome = export(&source, &tar_archive);

    assert_eq!(zip_outcome.container, ArchiveContainer::Zip);
    assert_eq!(tar_outcome.container, ArchiveContainer::TarGz);
    // The logical source home travels as provenance through both containers.
    assert_eq!(zip_outcome.source_home, source);
    assert_eq!(tar_outcome.source_home, source);
    // The credential metadata travels, but platform-held key material never does: the
    // archive is always a limited recovery, never a claim that keys are available.
    assert_eq!(zip_outcome.coverage, RecoveryCoverage::Limited);
    assert_eq!(tar_outcome.coverage, RecoveryCoverage::Limited);
    assert_eq!(zip_outcome.limitations.len(), 1);
    assert!(
        zip_outcome.limitations[0]
            .reason
            .contains("non-secret metadata"),
        "{:?}",
        zip_outcome.limitations
    );
    assert_eq!(zip_outcome.file_count, tar_outcome.file_count);
    assert_eq!(zip_outcome.total_bytes, tar_outcome.total_bytes);

    let zip_target = work.join("restored-zip");
    let tar_target = work.join("restored-tar");
    let zip_restore = restore(&zip_archive, &zip_target);
    let tar_restore = restore(&tar_archive, &tar_target);

    assert_eq!(zip_restore.file_count, tar_restore.file_count);
    assert_eq!(zip_restore.coverage, RecoveryCoverage::Limited);
    assert_eq!(zip_restore.source_home, source);
    assert_eq!(tar_restore.source_home, source);
    assert_eq!(payload(&zip_target), payload(&source));
    assert_eq!(payload(&tar_target), payload(&source));

    // Independent canary: capture and both restores left the source byte-identical.
    assert_eq!(payload(&source), payload(&canary));
}

#[test]
fn archive_origin_provenance_is_required_and_preserved() {
    let source = synthetic_data_root("provenance");
    let work = scratch("provenance-out");
    let archive = work.join("provenance.zip");
    let outcome = export(&source, &archive);
    assert_eq!(outcome.source_home, source);

    // The origin is part of the manifest itself, so an import reports the captured
    // home even when the importer cannot know it from the file system.
    let restored = restore(&archive, &work.join("restored"));
    assert_eq!(restored.source_home, source);

    // A manifest that does not name an absolute origin cannot be rebased later and is
    // refused before any destination is created.
    let mut forged_manifest = manifest_json("zip", Vec::new());
    forged_manifest["source_home"] = serde_json::json!("relative/source-home");
    let forged = work.join("forged.zip");
    write_zip_fixture(&forged, &forged_manifest, &[]);
    let target = work.join("forged-target");
    refusal(
        restore_data_root(&RestoreRequest {
            archive_path: forged,
            target_root: target.clone(),
        }),
        "archive_source_home_invalid",
    );
    assert!(!target.exists(), "a refused restore creates no destination");
}

#[test]
fn running_writer_is_refused_without_modifying_the_source() {
    let source = synthetic_data_root("writer");

    // A writer holds the application's own admission lock.
    let lock_path = source.join("client-state/migrations/admission.lock");
    fs::create_dir_all(lock_path.parent().expect("lock parent")).expect("lock dir");
    let held = fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(&lock_path)
        .expect("open lock");
    fs2::FileExt::lock_exclusive(&held).expect("hold admission lock");

    let canary = scratch("writer-canary");
    copy_root(&source, &canary);
    let work = scratch("writer-out");
    let archive = work.join("refused.zip");

    refusal(
        export_data_root(&ExportRequest {
            data_root: source.clone(),
            archive_path: archive.clone(),
            writers_stopped: true,
        }),
        "archive_writers_running",
    );

    assert!(
        !archive.exists(),
        "a refused capture must not leave an archive behind"
    );
    fs2::FileExt::unlock(&held).expect("release lock");

    // The source is byte-identical to the independent canary after the refusal.
    assert_eq!(payload(&source), payload(&canary));

    // With the writer stopped the same request succeeds.
    let outcome = export(&source, &archive);
    assert_eq!(outcome.coverage, RecoveryCoverage::Limited);
}

#[test]
fn capture_requires_the_writer_statement_and_an_existing_root() {
    let source = synthetic_data_root("preconditions");
    let canary = scratch("preconditions-canary");
    copy_root(&source, &canary);
    let work = scratch("preconditions-out");

    refusal(
        export_data_root(&ExportRequest {
            data_root: source.clone(),
            archive_path: work.join("no-statement.zip"),
            writers_stopped: false,
        }),
        "archive_writers_running",
    );

    refusal(
        export_data_root(&ExportRequest {
            data_root: work.join("absent"),
            archive_path: work.join("absent.zip"),
            writers_stopped: true,
        }),
        "data_root_missing",
    );

    refusal(
        export_data_root(&ExportRequest {
            data_root: source.clone(),
            archive_path: work.join("complete.rar"),
            writers_stopped: true,
        }),
        "archive_container_unsupported",
    );

    assert_eq!(payload(&source), payload(&canary));
}

#[test]
fn export_refuses_a_destination_inside_the_root_it_captures() {
    let source = synthetic_data_root("inside-root");
    let work = scratch("inside-root-out");
    let baseline = export(&source, &work.join("baseline.zip"));

    let direct = source.join("backup.zip");
    refusal(
        export_data_root(&ExportRequest {
            data_root: source.clone(),
            archive_path: direct.clone(),
            writers_stopped: true,
        }),
        "archive_path_inside_data_root",
    );
    assert!(!direct.exists(), "a refused capture publishes no archive");

    // A nested destination is refused before its parents are created inside the root, so
    // the root the capture declined to describe is untouched and can still be captured to
    // a destination the caller names outside it.
    let nested = source.join("nested/deeper/backup.tar.gz");
    refusal(
        export_data_root(&ExportRequest {
            data_root: source.clone(),
            archive_path: nested.clone(),
            writers_stopped: true,
        }),
        "archive_path_inside_data_root",
    );
    assert!(
        !source.join("nested").exists() && !nested.exists(),
        "no parent directory is created inside the captured root"
    );

    let after = export(&source, &work.join("after.zip"));
    assert_eq!(after.file_count, baseline.file_count);
    assert_eq!(after.total_bytes, baseline.total_bytes);
}

#[cfg(unix)]
#[test]
fn export_refuses_a_destination_reached_into_the_root_through_a_link() {
    let source = synthetic_data_root("linked-root");
    let work = scratch("linked-root-out");

    let link = work.join("root-link");
    std::os::unix::fs::symlink(&source, &link).expect("link to the captured root");
    let linked = link.join("nested/backup.zip");

    refusal(
        export_data_root(&ExportRequest {
            data_root: source.clone(),
            archive_path: linked.clone(),
            writers_stopped: true,
        }),
        "archive_path_inside_data_root",
    );
    assert!(
        !source.join("nested").exists() && !linked.exists(),
        "nothing is created inside the captured root through the link"
    );
}

#[cfg(unix)]
#[test]
fn export_refuses_a_dangling_symlink_destination_into_the_root() {
    let source = synthetic_data_root("dangling-destination");
    let canary = scratch("dangling-destination-canary");
    copy_root(&source, &canary);
    let work = scratch("dangling-destination-out");

    let destination = work.join("dest.zip");
    let escaped = source.join("created.zip");
    std::os::unix::fs::symlink(&escaped, &destination).expect("dangling link into the root");

    refusal(
        export_data_root(&ExportRequest {
            data_root: source.clone(),
            archive_path: destination.clone(),
            writers_stopped: true,
        }),
        "archive_destination_unsafe",
    );
    assert!(
        !escaped.exists(),
        "the writer never follows a dangling destination link into the source"
    );
    assert!(
        fs::symlink_metadata(&destination)
            .expect("destination metadata")
            .file_type()
            .is_symlink(),
        "the refused destination is left as it was"
    );
    assert_eq!(payload(&source), payload(&canary));
}

#[test]
fn refused_capture_preserves_an_existing_output_and_its_source_alias() {
    let source = synthetic_data_root("hard-link-output");
    let hold = source.join("client-state/hold.bin");
    write(&hold, b"hold bytes");
    let canary = scratch("hard-link-output-canary");
    copy_root(&source, &canary);
    let work = scratch("hard-link-output-out");

    let alias = work.join("existing.zip");
    fs::hard_link(&hold, &alias).expect("hard link the output name to a source file");

    // A refused capture leaves both names byte-identical.
    refusal(
        export_data_root(&ExportRequest {
            data_root: source.clone(),
            archive_path: work.join("complete.rar"),
            writers_stopped: true,
        }),
        "archive_container_unsupported",
    );
    assert_eq!(read(&hold), b"hold bytes");
    assert_eq!(read(&alias), b"hold bytes");

    // A successful capture into the aliased name replaces only that directory entry;
    // the source file and the rest of the source are untouched.
    let outcome = export(&source, &alias);
    assert_eq!(outcome.coverage, RecoveryCoverage::Limited);
    assert_eq!(read(&hold), b"hold bytes");
    assert_ne!(read(&alias), b"hold bytes");
    assert_eq!(payload(&source), payload(&canary));

    let target = work.join("restored");
    restore(&alias, &target);
    assert_eq!(read(&target.join("client-state/hold.bin")), b"hold bytes");
}

#[cfg(unix)]
#[test]
fn export_writes_a_private_archive_and_restore_keeps_private_state() {
    use std::os::unix::fs::PermissionsExt;

    let source = synthetic_data_root("private");
    let work = scratch("private-out");
    let archive = work.join("private.zip");
    export(&source, &archive);
    assert_eq!(
        fs::symlink_metadata(&archive)
            .expect("archive metadata")
            .permissions()
            .mode()
            & 0o777,
        0o600,
        "the archive is private"
    );

    let target = work.join("restored");
    restore(&archive, &target);
    assert_eq!(
        fs::symlink_metadata(&target)
            .expect("target metadata")
            .permissions()
            .mode()
            & 0o777,
        0o700,
        "the restored root is private"
    );
    assert_eq!(
        fs::symlink_metadata(target.join("client-state"))
            .expect("directory metadata")
            .permissions()
            .mode()
            & 0o777,
        0o700,
        "a restored directory is private"
    );
    assert_eq!(
        fs::symlink_metadata(target.join(CREDENTIAL_INVENTORY))
            .expect("file metadata")
            .permissions()
            .mode()
            & 0o777,
        0o600,
        "a restored file is private"
    );
}

#[cfg(unix)]
#[test]
fn capture_skips_symbolic_links_in_the_source_root() {
    let source = synthetic_data_root("symlinks");
    let work = scratch("symlinks-out");
    let baseline = export(&source, &work.join("baseline.zip"));

    let outside = scratch("symlinks-outside");
    write(&outside.join("secret.txt"), b"outside the captured root");
    std::os::unix::fs::symlink(outside.join("secret.txt"), source.join("linked-secret.txt"))
        .expect("link to an outside file");
    std::os::unix::fs::symlink(&outside, source.join("linked-directory"))
        .expect("link to an outside directory");

    let archive = work.join("with-links.zip");
    let outcome = export(&source, &archive);
    assert_eq!(
        outcome.file_count, baseline.file_count,
        "a symbolic link is not a captured member"
    );
    assert_eq!(outcome.total_bytes, baseline.total_bytes);

    let target = work.join("restored");
    restore(&archive, &target);
    assert!(
        !target.join("linked-secret.txt").exists()
            && !target.join("linked-directory").exists()
            && !target.join("linked-directory/secret.txt").exists(),
        "nothing the links pointed at is restored"
    );
}

#[test]
fn restore_refuses_a_non_empty_target_and_a_missing_payload() {
    let source = synthetic_data_root("target");
    let work = scratch("target-out");
    let archive = work.join("complete.zip");
    export(&source, &archive);

    let occupied = work.join("occupied");
    write(&occupied.join("keep.txt"), b"existing");
    refusal(
        restore_data_root(&RestoreRequest {
            archive_path: archive.clone(),
            target_root: occupied.clone(),
        }),
        "archive_target_not_empty",
    );
    assert_eq!(
        read(&occupied.join("keep.txt")),
        b"existing",
        "a refused restore must not disturb the target"
    );

    let decoy = work.join("decoy.zip");
    let mut writer = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
    let options = zip::write::SimpleFileOptions::default()
        .compression_method(zip::CompressionMethod::Deflated);
    writer
        .start_file("data/notes.txt", options)
        .expect("add member");
    writer.write_all(b"no manifest").expect("write member");
    let bytes = writer.finish().expect("finish zip").into_inner();
    fs::write(&decoy, bytes).expect("write decoy");

    let decoy_target = work.join("decoy-target");
    refusal(
        restore_data_root(&RestoreRequest {
            archive_path: decoy,
            target_root: decoy_target.clone(),
        }),
        "archive_manifest_missing",
    );
    assert!(
        !decoy_target.exists(),
        "an archive refused on its manifest never creates a destination"
    );
}

#[test]
fn restore_refuses_windows_style_manifest_escapes_in_both_containers() {
    let work = scratch("winescape");
    let bad = "..\\escaped.bin";
    let escaped = work.join("escaped.bin");

    let zip_archive = work.join("escape.zip");
    write_zip_fixture(
        &zip_archive,
        &manifest_json("zip", vec![entry_json(bad, "file", 4)]),
        &[("data/..\\escaped.bin", b"evil")],
    );
    let zip_target = work.join("escape-zip-target");
    refusal(
        restore_data_root(&RestoreRequest {
            archive_path: zip_archive,
            target_root: zip_target.clone(),
        }),
        "archive_inventory_path_invalid",
    );
    assert!(
        !zip_target.exists(),
        "a manifest refusal never creates the destination"
    );

    let tar_archive = work.join("escape.tar.gz");
    write_tar_gz_fixture(
        &tar_archive,
        &manifest_json("tar.gz", vec![entry_json(bad, "file", 4)]),
        &[("data/..\\escaped.bin", b"evil", tar::EntryType::Regular)],
    );
    let tar_target = work.join("escape-tar-target");
    refusal(
        restore_data_root(&RestoreRequest {
            archive_path: tar_archive,
            target_root: tar_target.clone(),
        }),
        "archive_inventory_path_invalid",
    );
    assert!(
        !tar_target.exists(),
        "a manifest refusal never creates the destination"
    );
    assert!(
        !escaped.exists(),
        "no Windows-style manifest path reaches the destination parent"
    );
}

#[test]
fn restore_refuses_drive_unc_aliased_and_malformed_manifest_paths() {
    let work = scratch("grammar");
    let cases: [&str; 12] = [
        "C:\\evil.bin",
        "\\\\server\\share\\evil.bin",
        "a:b.txt",
        "a\u{0}b.txt",
        "..\\..\\evil.bin",
        "/absolute.txt",
        "",
        ".",
        "..",
        "a/./b.txt",
        "a//b.txt",
        "a/../b.txt",
    ];
    for (index, path) in cases.iter().enumerate() {
        let archive = work.join(format!("grammar-{index}.zip"));
        write_zip_fixture(
            &archive,
            &manifest_json("zip", vec![entry_json(path, "file", 0)]),
            &[],
        );
        let target = work.join(format!("grammar-{index}-target"));
        refusal(
            restore_data_root(&RestoreRequest {
                archive_path: archive,
                target_root: target.clone(),
            }),
            "archive_inventory_path_invalid",
        );
        assert!(
            !target.exists(),
            "a manifest refusal never creates the destination"
        );
    }
}

#[test]
fn restore_refuses_aliases_conflicts_and_inconsistent_sizes() {
    let work = scratch("aliases");

    let duplicate = work.join("duplicate.zip");
    write_zip_fixture(
        &duplicate,
        &manifest_json(
            "zip",
            vec![
                entry_json("a.txt", "file", 1),
                entry_json("a.txt", "file", 1),
            ],
        ),
        &[],
    );
    refusal(
        restore_data_root(&RestoreRequest {
            archive_path: duplicate,
            target_root: work.join("duplicate-target"),
        }),
        "archive_inventory_duplicate",
    );

    let case_collision = work.join("case.zip");
    write_zip_fixture(
        &case_collision,
        &manifest_json(
            "zip",
            vec![
                entry_json("A.txt", "file", 1),
                entry_json("a.txt", "file", 1),
            ],
        ),
        &[],
    );
    refusal(
        restore_data_root(&RestoreRequest {
            archive_path: case_collision,
            target_root: work.join("case-target"),
        }),
        "archive_inventory_case_collision",
    );

    let conflict = work.join("conflict.zip");
    write_zip_fixture(
        &conflict,
        &manifest_json(
            "zip",
            vec![entry_json("a", "file", 1), entry_json("a/b", "file", 1)],
        ),
        &[],
    );
    refusal(
        restore_data_root(&RestoreRequest {
            archive_path: conflict,
            target_root: work.join("conflict-target"),
        }),
        "archive_inventory_conflict",
    );

    let directory_size = work.join("directory-size.zip");
    write_zip_fixture(
        &directory_size,
        &manifest_json("zip", vec![entry_json("d", "directory", 7)]),
        &[],
    );
    refusal(
        restore_data_root(&RestoreRequest {
            archive_path: directory_size,
            target_root: work.join("directory-size-target"),
        }),
        "archive_inventory_size_invalid",
    );

    let mut total_mismatch = manifest_json("zip", vec![entry_json("a.txt", "file", 2)]);
    total_mismatch["total_bytes"] = serde_json::json!(3);
    let total_archive = work.join("total.zip");
    write_zip_fixture(&total_archive, &total_mismatch, &[]);
    refusal(
        restore_data_root(&RestoreRequest {
            archive_path: total_archive,
            target_root: work.join("total-target"),
        }),
        "archive_inventory_size_invalid",
    );

    let container_mismatch = work.join("container.zip");
    write_zip_fixture(
        &container_mismatch,
        &manifest_json("tar.gz", vec![entry_json("a.txt", "file", 0)]),
        &[],
    );
    refusal(
        restore_data_root(&RestoreRequest {
            archive_path: container_mismatch,
            target_root: work.join("container-target"),
        }),
        "archive_container_mismatch",
    );
}

#[test]
fn restore_refuses_unlisted_payload_and_missing_or_resized_members() {
    let work = scratch("inventory");

    let unlisted_zip = work.join("unlisted.zip");
    write_zip_fixture(
        &unlisted_zip,
        &manifest_json("zip", vec![]),
        &[("data/extra.txt", b"extra")],
    );
    refusal(
        restore_data_root(&RestoreRequest {
            archive_path: unlisted_zip,
            target_root: work.join("unlisted-zip-target"),
        }),
        "archive_extraction_refused",
    );

    let unlisted_tar = work.join("unlisted.tar.gz");
    write_tar_gz_fixture(
        &unlisted_tar,
        &manifest_json("tar.gz", vec![]),
        &[("data/extra.txt", b"extra", tar::EntryType::Regular)],
    );
    refusal(
        restore_data_root(&RestoreRequest {
            archive_path: unlisted_tar,
            target_root: work.join("unlisted-tar-target"),
        }),
        "archive_extraction_refused",
    );

    let missing = work.join("missing.zip");
    write_zip_fixture(
        &missing,
        &manifest_json("zip", vec![entry_json("payload.txt", "file", 4)]),
        &[],
    );
    refusal(
        restore_data_root(&RestoreRequest {
            archive_path: missing,
            target_root: work.join("missing-target"),
        }),
        "archive_inventory_mismatch",
    );

    let resized = work.join("resized.zip");
    write_zip_fixture(
        &resized,
        &manifest_json("zip", vec![entry_json("payload.txt", "file", 4)]),
        &[("data/payload.txt", b"abc")],
    );
    refusal(
        restore_data_root(&RestoreRequest {
            archive_path: resized,
            target_root: work.join("resized-target"),
        }),
        "archive_inventory_mismatch",
    );
}

#[test]
fn restore_preserves_a_payload_named_like_the_staging_directory_in_both_containers() {
    let source = synthetic_data_root("staging-collision");
    write(
        &source.join(".licoup-restore-staging/keep.bin"),
        b"staging-name payload",
    );
    write(
        &source.join(".licoup-restore-staging-2/keep.bin"),
        b"second staging-name payload",
    );
    let work = scratch("staging-collision-out");

    for name in ["collision.zip", "collision.tar.gz"] {
        let archive = work.join(name);
        export(&source, &archive);
        let target = work.join(format!("{name}-target"));
        restore(&archive, &target);

        assert_eq!(
            read(&target.join(".licoup-restore-staging/keep.bin")),
            b"staging-name payload",
            "{name}: the staging-named payload survives"
        );
        assert_eq!(
            read(&target.join(".licoup-restore-staging-2/keep.bin")),
            b"second staging-name payload",
            "{name}: the second staging-named payload survives"
        );
        assert!(
            !target.join(".licoup-restore-staging-3").exists(),
            "{name}: no scratch directory leaks into the restored root"
        );
        assert_eq!(payload(&target), payload(&source));
    }
}

#[test]
fn restore_preserves_a_case_variant_staging_named_payload_in_both_containers() {
    let source = synthetic_data_root("staging-case");
    write(
        &source.join(".LICOUP-RESTORE-STAGING/keep.bin"),
        b"case-variant staging payload",
    );
    write(
        &source.join(".licoup-restore-staging-2/keep.bin"),
        b"second staging-name payload",
    );
    let work = scratch("staging-case-out");

    for name in ["case.zip", "case.tar.gz"] {
        let archive = work.join(name);
        export(&source, &archive);
        let target = work.join(format!("{name}-target"));
        restore(&archive, &target);

        assert_eq!(
            read(&target.join(".LICOUP-RESTORE-STAGING/keep.bin")),
            b"case-variant staging payload",
            "{name}: the case-variant staging-named payload survives"
        );
        assert_eq!(
            read(&target.join(".licoup-restore-staging-2/keep.bin")),
            b"second staging-name payload"
        );
        assert!(
            !target.join(".licoup-restore-staging-3").exists(),
            "{name}: no scratch directory leaks into the restored root"
        );
        assert_eq!(payload(&target), payload(&source));
    }
}

#[test]
fn restore_refuses_an_undeclared_top_level_member_in_both_containers() {
    let work = scratch("top-level-member");
    // The member count and byte total match the declared payload, so only the raw
    // inventory name comparison can catch the undeclared top-level file.
    let entries = vec![
        entry_json("a.txt", "file", 4),
        entry_json("b.txt", "file", 4),
    ];

    let zip_archive = work.join("top-level.zip");
    write_zip_fixture(
        &zip_archive,
        &manifest_json("zip", entries.clone()),
        &[("data/a.txt", b"aaaa"), ("escaped.bin", b"bbbb")],
    );
    let zip_target = work.join("top-level-zip-target");
    refusal(
        restore_data_root(&RestoreRequest {
            archive_path: zip_archive,
            target_root: zip_target.clone(),
        }),
        "archive_inventory_mismatch",
    );
    assert!(fs::read_dir(&zip_target).expect("target").next().is_none());

    let tar_archive = work.join("top-level.tar.gz");
    write_tar_gz_fixture(
        &tar_archive,
        &manifest_json("tar.gz", entries),
        &[
            ("data/a.txt", b"aaaa", tar::EntryType::Regular),
            ("escaped.bin", b"bbbb", tar::EntryType::Regular),
        ],
    );
    let tar_target = work.join("top-level-tar-target");
    refusal(
        restore_data_root(&RestoreRequest {
            archive_path: tar_archive,
            target_root: tar_target.clone(),
        }),
        "archive_inventory_mismatch",
    );
    assert!(fs::read_dir(&tar_target).expect("target").next().is_none());
}

#[test]
fn restore_refuses_tar_duplicate_members_that_extraction_would_collapse() {
    let work = scratch("tar-duplicate");
    let archive = work.join("duplicate.tar.gz");
    // Two declared directories and two raw members for the first one: the filesystem
    // would collapse them into one directory, so only the raw inventory can refuse it.
    write_tar_gz_fixture(
        &archive,
        &manifest_json(
            "tar.gz",
            vec![
                entry_json("d", "directory", 0),
                entry_json("e", "directory", 0),
            ],
        ),
        &[
            ("data/d/", &[], tar::EntryType::Directory),
            ("data/d/", &[], tar::EntryType::Directory),
        ],
    );
    let target = work.join("duplicate-target");
    refusal(
        restore_data_root(&RestoreRequest {
            archive_path: archive,
            target_root: target.clone(),
        }),
        "archive_inventory_duplicate",
    );
    assert!(fs::read_dir(&target).expect("target").next().is_none());
}

#[test]
fn restore_refuses_a_forged_complete_manifest() {
    let work = scratch("forged-complete");
    let archive = work.join("complete.zip");
    let mut manifest = manifest_json("zip", vec![]);
    manifest["coverage"] = serde_json::json!("complete");
    write_zip_fixture(&archive, &manifest, &[]);

    let target = work.join("complete-target");
    refusal(
        restore_data_root(&RestoreRequest {
            archive_path: archive,
            target_root: target.clone(),
        }),
        "archive_coverage_unproven",
    );
    assert!(
        !target.exists(),
        "a refused manifest never creates the destination"
    );
}

#[test]
fn restore_refuses_a_tar_directory_with_a_body() {
    let work = scratch("tar-directory-body");
    let archive = work.join("body.tar.gz");
    // A declared directory whose raw member declares a body: a legitimate archive never
    // carries one, and counting it as a member only would let it bypass the byte policy.
    let manifest = manifest_json(
        "tar.gz",
        vec![
            entry_json("d", "directory", 0),
            entry_json("e", "directory", 0),
        ],
    );
    let mut tar_bytes = Vec::new();
    {
        let mut builder = tar::Builder::new(&mut tar_bytes);
        append_tar_member(
            &mut builder,
            MANIFEST_MEMBER,
            tar::EntryType::Regular,
            &serde_json::to_vec(&manifest).expect("encode manifest"),
            None,
        );
        append_tar_member(
            &mut builder,
            "data/d/",
            tar::EntryType::Directory,
            &[],
            None,
        );
        let body = vec![0_u8; 4096];
        append_tar_member(
            &mut builder,
            "data/e/",
            tar::EntryType::Directory,
            &body,
            None,
        );
        builder.finish().expect("finish tar");
    }
    let mut gz_bytes = Vec::new();
    {
        let mut encoder =
            flate2::write::GzEncoder::new(&mut gz_bytes, flate2::Compression::default());
        encoder.write_all(&tar_bytes).expect("compress payload");
        encoder.finish().expect("finish gzip");
    }
    fs::write(&archive, gz_bytes).expect("write archive fixture");

    let target = work.join("body-target");
    refusal(
        restore_data_root(&RestoreRequest {
            archive_path: archive,
            target_root: target.clone(),
        }),
        "archive_extraction_refused",
    );
    assert!(
        !target.exists(),
        "the directory body is refused by raw admission before the destination is created"
    );
}

#[test]
fn restore_bounds_tar_metadata_before_the_library_buffers_it() {
    let work = scratch("tar-metadata-bound");
    // One declared member with a path far beyond the portable component length: the
    // declared payload budget plus its format overhead must refuse the long-name
    // metadata before the TAR library buffers it.
    let long_name = "l".repeat(32 * 1024);
    let member_name = format!("data/{long_name}");
    let archive = work.join("metadata.tar.gz");
    write_tar_gz_fixture(
        &archive,
        &manifest_json("tar.gz", vec![entry_json(&long_name, "file", 4)]),
        &[(member_name.as_str(), b"abcd", tar::EntryType::Regular)],
    );
    let target = work.join("metadata-target");
    refusal(
        restore_data_root(&RestoreRequest {
            archive_path: archive,
            target_root: target.clone(),
        }),
        "archive_extraction_refused",
    );
    assert!(
        !target.exists(),
        "the per-member metadata bound refuses before the destination is created"
    );
}

#[test]
fn restore_preserves_a_payload_named_like_the_scratch_under_unicode_folding_in_both_containers() {
    // U+017F folds to 's' on the default macOS filesystem, so an in-target scratch could
    // alias this legitimate payload name even though lowercase differs. The scratch now
    // lives outside the target namespace, so the payload survives in both containers.
    let source = synthetic_data_root("staging-unicode");
    write(
        &source.join(".licoup-re\u{17f}tore-staging/keep.bin"),
        b"unicode staging payload",
    );
    let work = scratch("staging-unicode-out");

    for name in ["unicode.zip", "unicode.tar.gz"] {
        let archive = work.join(name);
        export(&source, &archive);
        let target = work.join(format!("{name}-target"));
        restore(&archive, &target);

        assert_eq!(
            read(&target.join(".licoup-re\u{17f}tore-staging/keep.bin")),
            b"unicode staging payload",
            "{name}: the Unicode staging-named payload survives"
        );
        assert_eq!(payload(&target), payload(&source));
    }
}

#[test]
fn restore_refuses_a_truncated_tar_gz_container() {
    let source = synthetic_data_root("truncated");
    let work = scratch("truncated-out");
    let archive = work.join("complete.tar.gz");
    export(&source, &archive);
    let mut bytes = fs::read(&archive).expect("read archive");
    bytes.truncate(bytes.len() - 4);
    let truncated = work.join("truncated.tar.gz");
    fs::write(&truncated, &bytes).expect("write truncated archive");

    let target = work.join("truncated-target");
    refusal(
        restore_data_root(&RestoreRequest {
            archive_path: truncated,
            target_root: target.clone(),
        }),
        "archive_extraction_refused",
    );
    assert!(
        !target.exists(),
        "a truncated container never creates the destination"
    );
}

#[test]
fn restore_refuses_trailing_material_after_the_gzip_member() {
    let source = synthetic_data_root("trailing");
    let work = scratch("trailing-out");
    let archive = work.join("complete.tar.gz");
    export(&source, &archive);
    let mut bytes = fs::read(&archive).expect("read archive");
    bytes.extend_from_slice(b"trailing");
    let trailing = work.join("trailing.tar.gz");
    fs::write(&trailing, &bytes).expect("write archive with trailing material");

    let target = work.join("trailing-target");
    refusal(
        restore_data_root(&RestoreRequest {
            archive_path: trailing,
            target_root: target.clone(),
        }),
        "archive_extraction_refused",
    );
    assert!(!target.exists());
}

#[test]
fn restore_refuses_a_zip_directory_member_with_a_body() {
    let work = scratch("zip-directory-body");
    let manifest = manifest_json("zip", vec![entry_json("d", "directory", 0)]);
    let mut writer = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
    let options = zip::write::SimpleFileOptions::default()
        .compression_method(zip::CompressionMethod::Deflated);
    writer
        .start_file(MANIFEST_MEMBER, options)
        .expect("add manifest");
    writer
        .write_all(&serde_json::to_vec(&manifest).expect("encode manifest"))
        .expect("write manifest");
    // Directory-mode external attributes, so this fixture reaches the directory body
    // guard instead of being refused as a regular file with a directory name.
    let directory_options = zip::write::SimpleFileOptions::default()
        .compression_method(zip::CompressionMethod::Deflated)
        .unix_permissions(0o040755);
    writer
        .start_file("data/d/", directory_options)
        .expect("add directory member");
    writer.write_all(b"body").expect("write directory body");
    let mut bytes = writer.finish().expect("finish zip").into_inner();
    let central = {
        let mut archive = zip::ZipArchive::new(std::io::Cursor::new(&bytes)).unwrap();
        archive.by_index(1).unwrap().central_header_start() as usize
    };
    // unix_permissions masks file type bits; use actual directory attributes.
    bytes[central + 38..central + 42].copy_from_slice(&((0o040755_u32 << 16) | 16).to_le_bytes());
    {
        let mut archive = zip::ZipArchive::new(std::io::Cursor::new(&bytes)).unwrap();
        assert_eq!(
            archive.by_index(1).unwrap().unix_mode().unwrap() & 0o170000,
            0o040000
        );
    }
    let archive = work.join("directory-body.zip");
    fs::write(&archive, bytes).expect("write archive fixture");

    let target = work.join("directory-body-target");
    refusal(
        restore_data_root(&RestoreRequest {
            archive_path: archive,
            target_root: target.clone(),
        }),
        "archive_extraction_refused",
    );
    assert!(fs::read_dir(&target).expect("target").next().is_none());
}

#[test]
fn restore_refuses_a_member_hidden_behind_the_tar_terminator() {
    let work = scratch("hidden-member");
    let manifest = manifest_json("tar.gz", vec![entry_json("a.txt", "file", 1)]);
    let mut tar_bytes = Vec::new();
    {
        let mut builder = tar::Builder::new(&mut tar_bytes);
        append_tar_member(
            &mut builder,
            MANIFEST_MEMBER,
            tar::EntryType::Regular,
            &serde_json::to_vec(&manifest).expect("encode manifest"),
            None,
        );
        append_tar_member(
            &mut builder,
            "data/a.txt",
            tar::EntryType::Regular,
            b"a",
            None,
        );
        builder.finish().expect("finish tar");
    }
    // `finish` wrote the zero terminator; append an undeclared member behind it inside
    // the same GZIP member.
    let mut hidden = [0_u8; 512];
    hidden[..10].copy_from_slice(b"hidden.txt");
    hidden[124..136].copy_from_slice(b"00000000004\0");
    hidden[156] = b'0';
    let mut unsigned = 0_u64;
    for (index, byte) in hidden.iter().enumerate() {
        let value = if (148..156).contains(&index) {
            b' '
        } else {
            *byte
        };
        unsigned += u64::from(value);
    }
    hidden[148..156].copy_from_slice(format!("{unsigned:06o}\0 ").as_bytes());
    tar_bytes.extend_from_slice(&hidden);
    tar_bytes.extend_from_slice(b"evil");
    tar_bytes.extend_from_slice(&[0_u8; 508]);
    let mut gz_bytes = Vec::new();
    {
        let mut encoder =
            flate2::write::GzEncoder::new(&mut gz_bytes, flate2::Compression::default());
        encoder.write_all(&tar_bytes).expect("compress payload");
        encoder.finish().expect("finish gzip");
    }
    let archive = work.join("hidden.tar.gz");
    fs::write(&archive, gz_bytes).expect("write archive fixture");

    let target = work.join("hidden-target");
    refusal(
        restore_data_root(&RestoreRequest {
            archive_path: archive,
            target_root: target.clone(),
        }),
        "archive_extraction_refused",
    );
    assert!(
        !target.exists(),
        "a hidden member is refused before the destination is created"
    );
}

#[test]
fn restore_accepts_a_new_bare_relative_target() {
    let source = synthetic_data_root("relative-target");
    let work = scratch("relative-target-out");
    let archive = work.join("complete.zip");
    export(&source, &archive);

    let relative = PathBuf::from(format!("lico-relative-target-{}", std::process::id()));
    struct Cleanup(PathBuf);
    impl Drop for Cleanup {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
            if let Ok(entries) = fs::read_dir(".") {
                for entry in entries.flatten() {
                    let name = entry.file_name();
                    if name
                        .to_string_lossy()
                        .starts_with(".licoup-restore-staging")
                    {
                        let _ = fs::remove_dir_all(entry.path());
                    }
                }
            }
        }
    }
    let _cleanup = Cleanup(relative.clone());

    let restored = restore(&archive, &relative);
    assert_eq!(restored.coverage, RecoveryCoverage::Limited);
    assert_eq!(payload(&relative), payload(&source));
}

#[test]
fn a_failed_restore_leaves_the_destination_retryable() {
    let source = synthetic_data_root("retry");
    let work = scratch("retry-out");
    let good = work.join("good.zip");
    export(&source, &good);

    let bad = work.join("bad.zip");
    write_zip_fixture(
        &bad,
        &manifest_json("zip", vec![]),
        &[("data/extra.txt", b"extra")],
    );

    let target = work.join("restored");
    refusal(
        restore_data_root(&RestoreRequest {
            archive_path: bad,
            target_root: target.clone(),
        }),
        "archive_extraction_refused",
    );
    assert!(
        fs::read_dir(&target).expect("target").next().is_none(),
        "a refused restore publishes nothing"
    );
    assert!(
        !target.join(".licoup-restore-staging").exists(),
        "the operation's staging is cleaned up"
    );

    // The same destination accepts a complete archive afterwards.
    restore(&good, &target);
    assert_eq!(payload(&target), payload(&source));
}

#[test]
fn empty_home_round_trips_in_both_containers() {
    let source = scratch("empty-home");
    let work = scratch("empty-home-out");

    for name in ["empty.zip", "empty.tar.gz"] {
        let archive = work.join(name);
        let outcome = export(&source, &archive);
        assert_eq!(outcome.file_count, 0);
        // Missing credential metadata is reported, and no minimum file count is invented.
        assert_eq!(outcome.coverage, RecoveryCoverage::Limited);

        let target = work.join(format!("{name}-target"));
        let restored = restore(&archive, &target);
        assert_eq!(restored.file_count, 0);
        assert!(
            fs::read_dir(&target).expect("target").next().is_none(),
            "{name}: an empty home restores to an empty root"
        );
    }
}

#[test]
fn export_refuses_a_depth_its_own_importer_cannot_extract() {
    let work = scratch("depth-out");

    // 32 path components plus the `data/` prefix is one deeper than the extractor allows.
    let too_deep = scratch("depth-too-deep");
    let mut current = too_deep.clone();
    for index in 0..31 {
        current = current.join(format!("d{index}"));
    }
    write(&current.join("leaf.bin"), b"leaf");
    let refused_archive = work.join("too-deep.zip");
    refusal(
        export_data_root(&ExportRequest {
            data_root: too_deep,
            archive_path: refused_archive.clone(),
            writers_stopped: true,
        }),
        "archive_export_limits_exceeded",
    );
    assert!(!refused_archive.exists());

    // The supported depth round-trips completely.
    let supported = scratch("depth-supported");
    let mut current = supported.clone();
    for index in 0..30 {
        current = current.join(format!("d{index}"));
    }
    write(&current.join("leaf.bin"), b"leaf");
    let archive = work.join("supported.zip");
    export(&supported, &archive);
    let target = work.join("supported-target");
    restore(&archive, &target);
    assert_eq!(payload(&target), payload(&supported));
}

#[test]
fn export_refuses_a_member_count_its_own_importer_cannot_accept() {
    let crowded = scratch("crowded");
    for index in 0..10_000_u32 {
        fs::create_dir(crowded.join(format!("d{index:05}"))).expect("create directory");
    }
    let work = scratch("crowded-out");
    let archive = work.join("crowded.zip");
    refusal(
        export_data_root(&ExportRequest {
            data_root: crowded,
            archive_path: archive.clone(),
            writers_stopped: true,
        }),
        "archive_export_limits_exceeded",
    );
    assert!(!archive.exists());
}

#[test]
fn export_refuses_a_home_with_non_portable_member_names() {
    for name in ["colon", "backslash"] {
        let source = scratch(&format!("non-portable-{name}"));
        let file = if name == "colon" {
            "a:b.txt"
        } else {
            "a\\b.txt"
        };
        write(&source.join(file), b"host-dependent name");
        let work = scratch(&format!("non-portable-{name}-out"));
        let archive = work.join("refused.zip");
        refusal(
            export_data_root(&ExportRequest {
                data_root: source,
                archive_path: archive.clone(),
                writers_stopped: true,
            }),
            "data_root_path_not_portable",
        );
        assert!(!archive.exists());
    }
}

#[test]
fn coverage_never_completes_from_credential_metadata() {
    let work = scratch("custody");

    let present = synthetic_data_root("custody-present");
    let present_outcome = export(&present, &work.join("present.zip"));
    assert_eq!(present_outcome.coverage, RecoveryCoverage::Limited);
    assert_eq!(present_outcome.limitations.len(), 1);
    assert_eq!(
        present_outcome.limitations[0].domain,
        "gateway-credential-custody"
    );
    assert!(
        present_outcome.limitations[0]
            .reason
            .contains("non-secret metadata")
    );

    // Empty or malformed metadata is still not key availability.
    for (name, bytes) in [("empty", b"".as_slice()), ("malformed", b"{not json")] {
        let root = synthetic_data_root(&format!("custody-{name}"));
        fs::write(root.join(CREDENTIAL_INVENTORY), bytes).expect("write metadata");
        let outcome = export(&root, &work.join(format!("{name}.zip")));
        assert_eq!(
            outcome.coverage,
            RecoveryCoverage::Limited,
            "{name} metadata must not complete the recovery"
        );
    }

    let absent = synthetic_data_root("custody-absent");
    fs::remove_file(absent.join(CREDENTIAL_INVENTORY)).expect("remove metadata");
    let absent_archive = work.join("absent.zip");
    let absent_outcome = export(&absent, &absent_archive);
    assert_eq!(absent_outcome.coverage, RecoveryCoverage::Limited);
    assert!(
        absent_outcome.limitations[0].reason.contains("absent"),
        "{:?}",
        absent_outcome.limitations
    );

    let target = work.join("absent-target");
    let restored = restore(&absent_archive, &target);
    assert_eq!(restored.coverage, RecoveryCoverage::Limited);
    assert_eq!(restored.limitations, absent_outcome.limitations);
}

#[test]
fn restore_refuses_a_manifest_at_the_extraction_limits() {
    let work = scratch("manifest-limits");

    // Member depth counts the `data/` prefix: 32 components is already too deep.
    let deep_path = (0..32)
        .map(|index| format!("d{index}"))
        .collect::<Vec<_>>()
        .join("/");
    let deep = work.join("deep.zip");
    write_zip_fixture(
        &deep,
        &manifest_json("zip", vec![entry_json(&deep_path, "file", 0)]),
        &[],
    );
    refusal(
        restore_data_root(&RestoreRequest {
            archive_path: deep,
            target_root: work.join("deep-target"),
        }),
        "archive_extraction_refused",
    );

    // The manifest member counts too: 10,000 declared entries already exceed the policy.
    let entries: Vec<serde_json::Value> = (0..10_000_u32)
        .map(|index| entry_json(&format!("f{index:05}.bin"), "file", 0))
        .collect();
    let crowded = work.join("crowded-manifest.zip");
    write_zip_fixture(&crowded, &manifest_json("zip", entries), &[]);
    refusal(
        restore_data_root(&RestoreRequest {
            archive_path: crowded,
            target_root: work.join("crowded-target"),
        }),
        "archive_extraction_refused",
    );
}

#[cfg(unix)]
#[test]
fn restore_refuses_an_oversized_archive_before_reading_it() {
    let limits = licoup_foundation::core::safe_archive::default_zip_extraction_limits();
    let work = scratch("oversized");

    for name in ["oversized.zip", "oversized.tar.gz"] {
        let archive = work.join(name);
        let file = fs::File::create(&archive).expect("create sparse archive");
        file.set_len(limits.max_archive_bytes + 1)
            .expect("size sparse archive");
        drop(file);

        let target = work.join(format!("{name}-target"));
        refusal(
            restore_data_root(&RestoreRequest {
                archive_path: archive.clone(),
                target_root: target.clone(),
            }),
            "archive_extraction_refused",
        );
        assert!(
            !target.exists(),
            "{name}: a refused import never creates the destination"
        );
        assert_eq!(
            fs::metadata(&archive).expect("archive metadata").len(),
            limits.max_archive_bytes + 1,
            "{name}: the archive is refused by metadata, not by reading it"
        );
    }
}

#[test]
fn restore_bounds_a_tar_manifest_scan_before_finding_a_late_manifest() {
    let work = scratch("tar-scan");
    let manifest = manifest_json("tar.gz", vec![]);
    let archive = work.join("late-manifest.tar.gz");
    fs::write(&archive, tar_gz_bytes(&manifest, &[], 10_000)).expect("write archive fixture");

    let target = work.join("late-manifest-target");
    refusal(
        restore_data_root(&RestoreRequest {
            archive_path: archive,
            target_root: target.clone(),
        }),
        "archive_extraction_refused",
    );
    assert!(!target.exists());
}

#[cfg(unix)]
#[test]
fn restore_accepts_a_destination_below_a_linked_ancestor() {
    let source = synthetic_data_root("linked-target");
    let work = scratch("linked-target-out");
    let archive = work.join("complete.zip");
    export(&source, &archive);

    let real = work.join("real");
    fs::create_dir_all(&real).expect("create the real destination parent");
    let linked = work.join("linked");
    std::os::unix::fs::symlink(&real, &linked).expect("link to the destination parent");
    let target = linked.join("restored");

    let outcome = restore(&archive, &target);

    assert_eq!(outcome.coverage, RecoveryCoverage::Limited);
    assert!(
        target.join(".licoup-workspace.json").is_file(),
        "the restored root is reachable through the name the caller used"
    );
    assert_eq!(payload(&real.join("restored")), payload(&source));
    assert!(
        !real.join("restored/.licoup-restore-staging").exists(),
        "the staging area is removed after publication"
    );
}

#[test]
fn restore_refuses_a_linked_member_inside_an_archive() {
    let work = scratch("linked-member");

    for (name, bytes) in [
        ("linked-member.zip", zip_with_a_linked_member()),
        ("linked-member.tar.gz", tar_gz_with_a_linked_member()),
    ] {
        let archive = work.join(name);
        fs::write(&archive, bytes).expect("write fixture archive");
        let target = work.join(format!("{name}-target"));

        refusal(
            restore_data_root(&RestoreRequest {
                archive_path: archive,
                target_root: target.clone(),
            }),
            "archive_extraction_refused",
        );
        assert!(
            fs::symlink_metadata(target.join("data/link")).is_err(),
            "a refused restore publishes no member"
        );
        assert!(
            !work.join("escaped-target").exists(),
            "nothing is written where the linked member points"
        );
    }
}

/// A ZIP whose payload holds one member that is a symbolic link.
fn zip_with_a_linked_member() -> Vec<u8> {
    let manifest = manifest_json("zip", vec![]);
    let mut writer = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
    let options = zip::write::SimpleFileOptions::default()
        .compression_method(zip::CompressionMethod::Deflated);
    writer
        .start_file(MANIFEST_MEMBER, options)
        .expect("add manifest");
    writer
        .write_all(&serde_json::to_vec(&manifest).expect("encode manifest"))
        .expect("write manifest");
    writer
        .add_directory("data/", options)
        .expect("add payload directory");
    writer
        .add_symlink("data/link", "escaped-target", options)
        .expect("add linked member");
    writer.finish().expect("finish zip").into_inner()
}

/// A TAR.GZ whose payload holds one member that is a symbolic link.
fn tar_gz_with_a_linked_member() -> Vec<u8> {
    let manifest = manifest_json("tar.gz", vec![]);
    let mut tar_bytes = Vec::new();
    {
        let mut builder = tar::Builder::new(&mut tar_bytes);
        let manifest_bytes = serde_json::to_vec(&manifest).expect("encode manifest");
        append_tar_member(
            &mut builder,
            MANIFEST_MEMBER,
            tar::EntryType::Regular,
            &manifest_bytes,
            None,
        );
        append_tar_member(&mut builder, "data/", tar::EntryType::Directory, &[], None);
        append_tar_member(
            &mut builder,
            "data/link",
            tar::EntryType::Symlink,
            &[],
            Some("escaped-target"),
        );
        builder.finish().expect("finish tar");
    }
    let mut gz_bytes = Vec::new();
    {
        let mut encoder =
            flate2::write::GzEncoder::new(&mut gz_bytes, flate2::Compression::default());
        encoder.write_all(&tar_bytes).expect("compress payload");
        encoder.finish().expect("finish gzip");
    }
    gz_bytes
}
