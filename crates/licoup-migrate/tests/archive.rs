//! Focused regression for the archive verbs.
//!
//! The verbs are the standalone tool's own entry points, and they delegate to the same
//! native recovery composition the installed client's `backup` command uses. These cases
//! assert the delegation's observable contract: both plaintext containers round-trip the
//! same logical payload, an owner refusal is passed through as a stable code, and a refusal
//! publishes nothing. The credential limitation is asserted as the owner reports it —
//! coverage is always `limited`, because key material never travels — and never as a
//! fabricated `complete`.
//!
//! A marker-only or synthetic root is never presented as a complete recovery here: the
//! credential limitation must remain visible on every archive.

mod support;

use std::collections::BTreeMap;
use std::fs;
use std::path::Path;

use licoup_migrate::archive::{
    self, ARCHIVE_CONTAINER_UNSUPPORTED, ARCHIVE_TARGET_NOT_EMPTY, ARCHIVE_WRITERS_RUNNING,
};
use licoup_migrate::error::ARCHIVE_INSIDE_DATA_ROOT;
use licoup_native::core::full_data_root_archive::{RecoveryCoverage, RecoveryLimitation};
use support::*;

const CREDENTIAL_DOMAIN: &str = "gateway-credential-custody";
const CREDENTIAL_INVENTORY: &str = "llm-api-key-inventory.json";

/// A synthetic root the archive owner can capture.
///
/// `inventory` arranges whether the root carries the non-secret credential inventory
/// document; the coverage stays `limited` in both cases, and only the limitation's reason
/// differs. Nothing here is a real secret.
fn exportable_root(root: &Path, inventory: bool) {
    write_file(
        root,
        "client-state/opaque-store.bin",
        b"opaque application state",
    );
    write_file(root, "client-state/cache/entry.txt", b"cache entry");
    write_file(root, "workspaces/demo/notes.md", b"# synthetic workspace\n");
    write_file(root, "empty.txt", b"");
    if inventory {
        write_file(
            root,
            CREDENTIAL_INVENTORY,
            br#"{"schemaVersion":"licoup.llm-api-key-inventory.v1","leaseDays":7,"entries":[]}"#,
        );
    }
}

/// Every regular file below one root, keyed by relative path.
fn files(root: &Path) -> BTreeMap<String, Vec<u8>> {
    root_files(root)
}

/// AC: both containers round-trip the same logical payload.
#[test]
fn both_containers_round_trip_the_same_logical_payload() {
    let root = TestRoot::new("round-trip");
    exportable_root(&root.join("source"), true);
    let source = root.join("source");
    let mut restored_payloads = Vec::new();

    for (name, container) in [("backup.zip", "zip"), ("backup.tar.gz", "tar.gz")] {
        let archive_path = root.join(&format!("archives/{name}"));
        let export = archive::export(&source, &archive_path, true).expect("export runs");
        assert_eq!(export.status, "exported");
        assert_eq!(export.container, container);
        assert_eq!(
            export.coverage,
            RecoveryCoverage::Limited,
            "credential key material never travels, so a capture is never complete"
        );
        let domains: Vec<&str> = export
            .limitations
            .iter()
            .map(|limitation| limitation.domain.as_str())
            .collect();
        assert_eq!(domains, vec![CREDENTIAL_DOMAIN]);

        // The owner's own coverage report is the second half of the oracle: the payload it
        // declared must be the payload that came back.
        let source_payload = files(&source);
        assert_eq!(export.file_count, source_payload.len());
        assert_eq!(
            export.total_bytes,
            source_payload
                .values()
                .map(|bytes| bytes.len() as u64)
                .sum::<u64>()
        );

        let target = root.join(&format!("restored/{container}"));
        let import = archive::import(&archive_path, &target).expect("import runs");
        assert_eq!(import.status, "imported");
        assert_eq!(import.container, container);
        assert_eq!(import.coverage, RecoveryCoverage::Limited);
        assert_eq!(import.file_count, export.file_count);
        assert_eq!(import.total_bytes, export.total_bytes);
        assert_eq!(
            import.limitations.len(),
            1,
            "the limitation travels with the archive"
        );
        assert_eq!(import.limitations[0].domain, CREDENTIAL_DOMAIN);

        // The restore is free to add the client-state collections its own composition
        // creates, so the oracle is that every captured file arrives with its bytes.
        let restored = files(&target);
        for (relative, bytes) in &source_payload {
            assert_eq!(
                restored.get(relative),
                Some(bytes),
                "the {container} restore carries {relative}"
            );
        }
        restored_payloads.push((container, restored_payloads_for(&source_payload, &restored)));
    }

    assert_eq!(
        restored_payloads[0].1, restored_payloads[1].1,
        "both restored roots carry the same logical payload"
    );
}

/// Restrict one restored listing to the captured files, so a test comparison never depends
/// on the composition's own bookkeeping documents.
fn restored_payloads_for(
    source: &BTreeMap<String, Vec<u8>>,
    restored: &BTreeMap<String, Vec<u8>>,
) -> BTreeMap<String, Vec<u8>> {
    source
        .keys()
        .map(|relative| {
            (
                relative.clone(),
                restored
                    .get(relative)
                    .unwrap_or_else(|| panic!("{relative} is restored"))
                    .clone(),
            )
        })
        .collect()
}

/// AC: a credential store that cannot travel is named, not hidden.
#[test]
fn a_credential_store_that_cannot_travel_is_reported_as_a_limitation() {
    let root = TestRoot::new("limited");
    exportable_root(&root.join("source"), false);
    let source = root.join("source");
    let archive_path = root.join("archives/limited.zip");

    let export = archive::export(&source, &archive_path, true).expect("export runs");
    assert_eq!(export.status, "exported");
    assert_eq!(
        export.coverage,
        RecoveryCoverage::Limited,
        "a missing credential store is never reported as complete recovery"
    );
    let domains: Vec<&str> = export
        .limitations
        .iter()
        .map(|limitation| limitation.domain.as_str())
        .collect();
    assert_eq!(domains, vec![CREDENTIAL_DOMAIN]);
    let reason: &str = &export.limitations[0].reason;
    assert!(
        reason.contains("absent from the captured root"),
        "the owner's own reason is reported: {reason}"
    );

    // The limitation is written into the archive, so a restore repeats it instead of
    // promising the credential back.
    let target = root.join("restored");
    let import = archive::import(&archive_path, &target).expect("import runs");
    assert_eq!(import.coverage, RecoveryCoverage::Limited);
    assert_eq!(import.limitations.len(), 1);
    assert_eq!(import.limitations[0].domain, CREDENTIAL_DOMAIN);

    // Everything that did travel still arrives intact.
    let restored = files(&target);
    for (relative, bytes) in files(&source) {
        assert_eq!(restored.get(&relative), Some(&bytes), "{relative} arrived");
    }
}

/// A present inventory is still metadata, never custody proof: coverage stays limited and
/// the limitation explains that the credential owner decides availability.
#[test]
fn a_present_inventory_document_does_not_complete_the_recovery() {
    let root = TestRoot::new("metadata-present");
    exportable_root(&root.join("source"), true);
    let source = root.join("source");
    let archive_path = root.join("archives/with-metadata.zip");

    let export = archive::export(&source, &archive_path, true).expect("export runs");
    assert_eq!(export.coverage, RecoveryCoverage::Limited);
    let limitation: &RecoveryLimitation = &export.limitations[0];
    assert_eq!(limitation.domain, CREDENTIAL_DOMAIN);
    assert!(
        limitation.reason.contains("travels as non-secret metadata"),
        "the present inventory is reported as metadata: {}",
        limitation.reason
    );
}

#[test]
fn capture_leaves_the_source_payload_untouched() {
    let root = TestRoot::new("source-preserved");
    exportable_root(&root.join("source"), true);
    let source = root.join("source");
    let before = files(&source);

    for name in ["backup.zip", "backup.tar.gz"] {
        archive::export(&source, &root.join(&format!("archives/{name}")), true)
            .expect("export runs");
        let after = files(&source);
        for (relative, bytes) in &before {
            assert_eq!(
                after.get(relative),
                Some(bytes),
                "{relative} is unchanged by capture"
            );
        }
    }
}

#[test]
fn the_container_is_inferred_from_the_archive_name() {
    let root = TestRoot::new("container");
    exportable_root(&root.join("source"), true);
    let source = root.join("source");

    let shorthand =
        archive::export(&source, &root.join("archives/backup.tgz"), true).expect("export runs");
    assert_eq!(shorthand.container, "tar.gz");

    let upper =
        archive::export(&source, &root.join("archives/BACKUP.ZIP"), true).expect("export runs");
    assert_eq!(upper.container, "zip");
}

#[test]
fn export_refuses_without_the_stopped_writer_statement_and_publishes_nothing() {
    let root = TestRoot::new("writers");
    exportable_root(&root.join("source"), true);
    let source = root.join("source");
    let archive_path = root.join("archives/unconfirmed.zip");

    let error = archive::export(&source, &archive_path, false).expect_err("refused");
    assert_eq!(error.code(), "archive_writers_running");
    assert!(
        !archive_path.exists(),
        "a refused export leaves no file that could be mistaken for a backup"
    );
    assert_eq!(ARCHIVE_WRITERS_RUNNING.code(), "archive_writers_running");
}

#[test]
fn export_refuses_a_name_that_is_not_a_plaintext_container() {
    let root = TestRoot::new("container-refusal");
    exportable_root(&root.join("source"), true);
    let source = root.join("source");
    let archive_path = root.join("archives/backup.bin");

    let error = archive::export(&source, &archive_path, true).expect_err("refused");
    assert_eq!(error.code(), ARCHIVE_CONTAINER_UNSUPPORTED.code());
    assert!(!archive_path.exists());
}

#[test]
fn export_refuses_a_missing_data_root() {
    let root = TestRoot::new("missing-root");
    let archive_path = root.join("archives/absent.zip");

    let error = archive::export(&root.join("absent"), &archive_path, true).expect_err("refused");
    assert_eq!(error.code(), "data_root_missing");
    assert!(!archive_path.exists());
}

#[test]
fn export_refuses_a_destination_inside_the_root_it_captures() {
    let root = TestRoot::new("inside-root");
    exportable_root(&root.join("source"), true);
    let source = root.join("source");
    let archive_path = source.join("backup.zip");

    let error = archive::export(&source, &archive_path, true).expect_err("refused");
    assert_eq!(error, ARCHIVE_INSIDE_DATA_ROOT);
    assert!(
        !archive_path.exists(),
        "a refused capture leaves no file and no captured copy of itself"
    );
    // The root the capture declined to describe is untouched, so it can still be captured
    // to a destination outside it.
    let outside = root.join("archives/backup.zip");
    let export = archive::export(&source, &outside, true).expect("export runs");
    assert_eq!(export.coverage, RecoveryCoverage::Limited);
    assert_eq!(export.file_count, files(&source).len());
}

#[test]
fn import_refuses_a_destination_that_is_not_empty() {
    let root = TestRoot::new("occupied-target");
    exportable_root(&root.join("source"), true);
    let source = root.join("source");
    let archive_path = root.join("archives/backup.zip");
    archive::export(&source, &archive_path, true).expect("export runs");

    let target = root.join("occupied");
    fs::create_dir_all(&target).expect("occupied destination");
    write_file(&target, "keep.txt", b"existing content");

    let error = archive::import(&archive_path, &target).expect_err("refused");
    assert_eq!(error.code(), ARCHIVE_TARGET_NOT_EMPTY.code());
    assert_eq!(error.code(), "archive_target_not_empty");
    assert_eq!(
        fs::read(target.join("keep.txt")).expect("existing entry survives"),
        b"existing content"
    );
    assert_eq!(
        files(&target).len(),
        1,
        "a refused import publishes nothing into the destination"
    );
}

#[test]
fn import_refuses_bytes_that_are_not_an_archive_and_publishes_nothing() {
    let root = TestRoot::new("not-an-archive");
    write_file(
        root.path(),
        "archives/broken.zip",
        b"this is not a plaintext archive",
    );
    let archive_path = root.join("archives/broken.zip");
    let target = root.join("restored");

    let error = archive::import(&archive_path, &target).expect_err("refused");
    assert_eq!(
        error.code(),
        "archive_extraction_refused",
        "the owner's own refusal code is passed through unchanged"
    );
    assert!(
        files(&target).is_empty(),
        "a refused import publishes nothing into the destination"
    );
}

#[test]
fn import_refuses_a_missing_archive() {
    let root = TestRoot::new("missing-archive");
    let target = root.join("restored");

    let error = archive::import(&root.join("archives/absent.zip"), &target).expect_err("refused");
    assert_eq!(error.code(), "archive_unreadable");
    assert!(files(&target).is_empty());
}

#[test]
fn reports_serialize_as_camel_case_json_with_a_stable_status() {
    let root = TestRoot::new("json");
    exportable_root(&root.join("source"), true);
    let source = root.join("source");
    let archive_path = root.join("archives/backup.tar.gz");
    let export = archive::export(&source, &archive_path, true).expect("export runs");
    let import = archive::import(&archive_path, &root.join("restored")).expect("import runs");

    let exported = serde_json::to_value(&export).expect("export report serializes");
    assert_eq!(exported["status"], "exported");
    for key in [
        "container",
        "coverage",
        "limitations",
        "fileCount",
        "totalBytes",
    ] {
        assert!(exported.get(key).is_some(), "{key} is part of the report");
    }
    assert!(exported.get("file_count").is_none(), "keys are camelCase");
    assert_eq!(exported["coverage"], "limited");
    assert_eq!(exported["limitations"][0]["domain"], CREDENTIAL_DOMAIN);

    let imported = serde_json::to_value(&import).expect("import report serializes");
    assert_eq!(imported["status"], "imported");
    assert_eq!(imported["container"], "tar.gz");
    assert_eq!(imported["coverage"], "limited");
    assert!(imported.get("relocated").is_some());
    assert!(imported.get("verifiedWorkflowRevisions").is_some());

    // The two report types stay distinct so a caller cannot confuse the verbs.
    let _: archive::ExportReport = export;
    let _: archive::ImportReport = import;
}
