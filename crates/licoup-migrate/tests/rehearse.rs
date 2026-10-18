//! Focused regression for the rehearsal entry.
//!
//! The rehearsal is driven through the shipped binary so the verb, its arguments and its
//! report are exercised together. The source root is the frozen `v0.2.1` released fixture,
//! not a hand-written approximation, and the suite therefore runs under the planned
//! candidate identity (see `tests/support/mod.rs`).
//!
//! Two acceptance criteria:
//!
//! * a released-format source root is converted, exported to both plaintext containers
//!   and restored, and the report names every stage as observed while the roots those
//!   stages produced are on disk and the source fingerprint is unchanged;
//! * a source that is not at the released format and a missing source are refused with a
//!   typed code before anything is written.
//!
//! The oracle is deliberately the disk: every stage the report claims is compared with the
//! root that stage names, so a report that claims a stage it did not run fails here.

mod support;

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use licoup_native::core::full_data_root_archive::ADMISSION_LOCK_PATH;
use licoup_native::platform::llm_api_key_vault::{
    LegacyCredentialMigrationDisposition, PlatformLlmApiKeyVault,
};
use support::*;

/// The stage list the report must carry on every run.
const STAGES: &[&str] = &[
    "source-observed",
    "copied",
    "converted",
    "archive-zip",
    "restore-zip",
    "reopen-zip",
    "archive-tar-gz",
    "restore-tar-gz",
    "reopen-tar-gz",
];

/// One file body, described as `<length>:<fold>` so a comparison names a difference
/// without printing a store's contents.
fn body(listing: &BTreeMap<String, Vec<u8>>, relative: &str) -> String {
    let bytes = listing.get(relative).expect("a listed fixture entry");
    format!("{}:{:016x}", bytes.len(), fold(bytes))
}

/// Every regular file's body descriptor, keyed by relative path.
fn body_listing(root: &Path) -> BTreeMap<String, String> {
    root_files(root)
        .into_iter()
        .map(|(relative, bytes)| {
            let described = format!("{}:{:016x}", bytes.len(), fold(&bytes));
            (relative, described)
        })
        .collect()
}

/// A stable fold over one file body.
fn fold(bytes: &[u8]) -> u64 {
    let mut digest = 0xcbf2_9ce4_8422_2325_u64;
    for byte in bytes {
        digest = (digest ^ u64::from(*byte)).wrapping_mul(0x0000_0100_0000_01b3);
    }
    digest
}

/// One root's fingerprint, computed the way the rehearsal reports it.
struct ObservedFingerprint {
    entries: u64,
    digest: String,
}

fn fingerprint_of(root: &Path) -> ObservedFingerprint {
    let mut entries: Vec<(String, char, u64)> = Vec::new();
    collect_fingerprint(root, root, &mut entries);
    entries.sort();
    let mut digest = 0xcbf2_9ce4_8422_2325_u64;
    for (relative, kind, size) in &entries {
        for byte in relative.as_bytes() {
            digest = (digest ^ u64::from(*byte)).wrapping_mul(0x0000_0100_0000_01b3);
        }
        digest = (digest ^ u64::from(*kind as u8)).wrapping_mul(0x0000_0100_0000_01b3);
        for byte in size.to_le_bytes() {
            digest = (digest ^ u64::from(byte)).wrapping_mul(0x0000_0100_0000_01b3);
        }
    }
    ObservedFingerprint {
        entries: entries.len() as u64,
        digest: format!("{digest:016x}"),
    }
}

fn collect_fingerprint(root: &Path, directory: &Path, entries: &mut Vec<(String, char, u64)>) {
    let Ok(read) = fs::read_dir(directory) else {
        return;
    };
    for entry in read.filter_map(Result::ok) {
        let path = entry.path();
        let Ok(metadata) = fs::symlink_metadata(&path) else {
            continue;
        };
        let relative = path
            .strip_prefix(root)
            .expect("entry below the root")
            .to_string_lossy()
            .replace('\\', "/");
        if relative == ADMISSION_LOCK_PATH {
            continue;
        }
        if metadata.is_dir() {
            entries.push((relative, 'd', 0));
            collect_fingerprint(root, &path, entries);
        } else if metadata.is_file() {
            entries.push((relative, 'f', metadata.len()));
        }
    }
}

/// Seed the released root and arrange additional user content on top of it.
fn released_source(root: &TestRoot) -> PathBuf {
    assert_candidate_identity();
    let source = root.released_source();
    seed_released_root(&source);
    write_file(
        &source,
        "workspaces/demo/notes.md",
        b"# synthetic workspace\n",
    );
    write_file(&source, "empty.txt", b"");
    source
}

fn rehearse_arguments<'a>(source: &'a str, work: &'a str, extra: &[&'a str]) -> Vec<&'a str> {
    let mut arguments = vec![
        "rehearse",
        "--data-root",
        source,
        "--work-root",
        work,
        "--writers-stopped",
    ];
    arguments.extend_from_slice(extra);
    arguments
}

/// The rehearsal converts the released root and round-trips it through both containers.
/// The disk, not the report, is the oracle.
#[test]
fn the_rehearsal_converts_and_round_trips_both_containers() {
    let root = TestRoot::new("complete");
    let source = released_source(&root);
    let source_before = root_files(&source);

    let (code, report) = run_tool(&rehearse_arguments(
        source.to_str().expect("utf-8 source"),
        root.work().to_str().expect("utf-8 work"),
        &["--keep-work-root"],
    ));

    assert_eq!(code, 0, "a complete rehearsal exits zero: {report}");
    assert_eq!(report["status"], "complete");
    assert_eq!(report["recoveryComplete"], true);
    assert!(
        report["notRun"].as_array().expect("notRun").is_empty(),
        "nothing is left unrun: {report}"
    );
    assert!(
        report["stillOwed"]
            .as_array()
            .expect("stillOwed")
            .is_empty(),
        "the owner accounted for every declared domain: {report}"
    );
    assert!(
        report["converted"]
            .as_array()
            .expect("converted")
            .iter()
            .any(|domain| domain == "adaptive-flywheel"),
        "the released strategy store was moved to the current format: {report}"
    );
    let requires_authorization = PlatformLlmApiKeyVault::legacy_credential_migration_disposition()
        .expect("platform credential disposition")
        == LegacyCredentialMigrationDisposition::RequiresAuthorization;
    assert_eq!(
        report["pendingAuthorization"]
            .as_array()
            .expect("pendingAuthorization")
            .iter()
            .any(|domain| domain == "gateway-credential-custody"),
        requires_authorization,
        "the report follows the credential owner's platform verdict: {report}"
    );

    // Every stage is named exactly once, in the declared order.
    let stages: Vec<&str> = report["stages"]
        .as_array()
        .expect("stages")
        .iter()
        .map(|stage| stage["stage"].as_str().expect("stage name"))
        .collect();
    assert_eq!(stages, STAGES);

    // Each claimed stage names a root that is on disk, and the fingerprint it reports is
    // the fingerprint of that root as this suite computes it.
    let source_fingerprint = &report["sourceFingerprint"];
    assert_eq!(
        source_fingerprint["digest"].as_str(),
        Some(fingerprint_of(&source).digest.as_str()),
        "the report states the source's own fingerprint"
    );
    for stage in report["stages"].as_array().expect("stages") {
        let name = stage["stage"].as_str().expect("stage name");
        assert_eq!(stage["outcome"], "observed", "{name} ran: {stage}");
        let named = stage["root"]
            .as_str()
            .unwrap_or_else(|| panic!("{name} names the root it produced"));
        let path = Path::new(named);
        assert!(
            path.exists(),
            "{name} claims a root that is absent: {named}"
        );
        let fingerprint = &stage["rootFingerprint"];
        assert!(
            fingerprint["digest"]
                .as_str()
                .is_some_and(|value| value.len() == 16),
            "{name} reports a fingerprint for its root: {stage}"
        );
        if name == "copied" {
            let source_shape = fingerprint_of(&source);
            assert_eq!(
                fingerprint["entries"].as_u64(),
                Some(source_shape.entries),
                "the copy holds the released root's entry count"
            );
            assert_eq!(
                fingerprint["digest"].as_str(),
                Some(source_shape.digest.as_str()),
                "the copy is the released root byte for byte"
            );
            continue;
        }
        if path.is_file() {
            // An archive stage's product is the archive file itself.
            continue;
        }
        let on_disk = fingerprint_of(path);
        assert_eq!(
            fingerprint["entries"].as_u64(),
            Some(on_disk.entries),
            "{name} counts the entries of the root it names"
        );
        assert_eq!(
            fingerprint["digest"].as_str(),
            Some(on_disk.digest.as_str()),
            "{name} describes the root it left on disk"
        );
    }

    // The two containers each left the converted root and a restored copy, and the archive
    // stage names the container the owner actually wrote.
    let observed: BTreeMap<&str, &str> = report["stages"]
        .as_array()
        .expect("stages")
        .iter()
        .map(|stage| {
            (
                stage["stage"].as_str().expect("stage name"),
                stage["observed"].as_str().expect("observed"),
            )
        })
        .collect();
    assert!(observed["archive-zip"].contains("zip container"));
    assert!(observed["archive-tar-gz"].contains("tar.gz container"));
    assert!(observed["reopen-zip"].contains("converted nothing"));
    assert!(observed["reopen-tar-gz"].contains("converted nothing"));

    // The converted root the caller keeps is a real converted root: user content travelled
    // into it untouched, and both restores carry it back. The conversion's own records and
    // the recovery rewrite legitimately change owner-managed documents, so those are read
    // back through their owners instead of being compared as bytes.
    let converted = body_listing(&root.work().join("converted"));
    let restored_zip = body_listing(&root.work().join("restored/zip"));
    let restored_tar = body_listing(&root.work().join("restored/tar.gz"));
    let source_after = root_files(&source);
    for (relative, bytes) in &source_before {
        assert_eq!(
            source_after.get(relative),
            Some(bytes),
            "{relative} is untouched by the rehearsal"
        );
    }
    for relative in [
        "workspaces/demo/notes.md",
        "empty.txt",
        "llm-api-key-inventory.json",
    ] {
        let described = body(&source_before, relative);
        assert_eq!(
            converted.get(relative),
            Some(&described),
            "{relative} travelled into the converted root"
        );
        assert_eq!(
            restored_zip.get(relative),
            Some(&described),
            "{relative} came back from the zip restore"
        );
        assert_eq!(
            restored_tar.get(relative),
            Some(&described),
            "{relative} came back from the tar.gz restore"
        );
    }
    assert_eq!(
        restored_zip, restored_tar,
        "both containers carry the same logical payload"
    );

    // The converted store in each restored root is a database the client's own owner can
    // read, not a file that merely exists.
    for container in ["zip", "tar.gz"] {
        let database = root
            .work()
            .join("restored")
            .join(container)
            .join(RELEASED_STRATEGY_DATABASE);
        let connection = rusqlite::Connection::open_with_flags(
            &database,
            rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
        )
        .expect("the restored strategy store opens read-only");
        let version: String = connection
            .query_row(
                "SELECT value FROM strategy_meta WHERE key='version'",
                [],
                |row| row.get(0),
            )
            .expect("the restored store carries its format version");
        assert_eq!(
            version, "3",
            "the {container} restore carries the converted strategy format"
        );
    }
}

/// The source root is not the subject: a rehearsal that keeps its working root still
/// leaves the named source byte-identical, and one that does not keep it removes only its
/// own working root.
#[test]
fn the_named_source_is_read_only_and_the_working_root_is_disposable() {
    let root = TestRoot::new("disposable");
    let source = released_source(&root);
    let before = root_files(&source);

    let (code, report) = run_tool(&rehearse_arguments(
        source.to_str().expect("utf-8 source"),
        root.work().to_str().expect("utf-8 work"),
        &[],
    ));
    assert_eq!(code, 0, "{report}");
    assert_eq!(report["workRootRetained"], false);
    assert!(
        !root.work().exists(),
        "the disposable working root is removed when the caller does not keep it"
    );
    assert_eq!(before, root_files(&source), "the source is unchanged");

    assert_eq!(
        report["workRoot"].as_str(),
        Some(root.work().to_str().expect("utf-8 work path"))
    );
}

/// A source that already declares this binary's own target frontier is refused by code,
/// with nothing written outside the disposable working directory.
#[test]
fn a_source_that_is_not_at_the_released_format_is_refused_before_anything_is_written() {
    let root = TestRoot::new("wrong-shape");
    let source = released_source(&root);
    licoup_native::domain::client_state_migration::admit(&source).expect("admission");
    let before = root_files(&source);

    let (code, report) = run_tool(&rehearse_arguments(
        source.to_str().expect("utf-8 source"),
        root.work().to_str().expect("utf-8 work"),
        &["--keep-work-root"],
    ));

    assert_eq!(code, 1, "a refused rehearsal exits non-zero: {report}");
    assert_eq!(report["status"], "refused");
    assert_eq!(report["error"], "source_root_already_at_target");
    let shape = &report["observedShape"];
    assert_eq!(shape["present"], true);
    assert_eq!(shape["directory"], true);
    assert_eq!(
        shape["frontierId"], shape["targetFrontierId"],
        "the source already declares this binary's own target: {report}"
    );
    assert!(shape["entries"].as_u64().is_some_and(|entries| entries > 0));
    assert!(
        !root.work().exists(),
        "a refusal writes nothing outside its own working directory: it writes none at all"
    );
    assert_eq!(before, root_files(&source), "the source is unchanged");
}

/// A missing source is refused by code and nothing is created.
#[test]
fn a_missing_source_is_refused_and_creates_nothing() {
    let root = TestRoot::new("missing");
    let absent = root.released_source();

    let (code, report) = run_tool(&rehearse_arguments(
        absent.to_str().expect("utf-8 source"),
        root.work().to_str().expect("utf-8 work"),
        &["--keep-work-root"],
    ));

    assert_eq!(code, 1);
    assert_eq!(report["status"], "refused");
    assert_eq!(report["error"], "source_root_missing");
    let shape = &report["observedShape"];
    assert_eq!(shape["present"], false, "the observed shape is the absence");
    assert_eq!(shape["entries"], 0);
    assert!(
        shape.get("frontierId").is_none(),
        "no root declares no frontier"
    );
    assert!(!absent.exists(), "the refused root is not created");
    assert!(!root.work().exists(), "no working root is created");
}

/// A directory that carries no released shape at all is refused rather than rehearsed into
/// a root no release ever produced.
#[test]
fn a_source_without_a_released_shape_is_refused() {
    let root = TestRoot::new("empty-source");
    fs::create_dir_all(root.released_source()).expect("empty source");

    let (code, report) = run_tool(&rehearse_arguments(
        root.released_source().to_str().expect("utf-8 source"),
        root.work().to_str().expect("utf-8 work"),
        &["--keep-work-root"],
    ));

    assert_eq!(code, 1);
    assert_eq!(report["status"], "refused");
    assert_eq!(report["error"], "source_root_shape_absent");
    let shape = &report["observedShape"];
    assert_eq!(shape["present"], true);
    assert_eq!(shape["directory"], true);
    assert_eq!(
        shape["entries"], 0,
        "an empty directory carries no released shape: {report}"
    );
    assert!(!root.work().exists());
}

/// The parser refuses a rehearsal that omits the working root or the stopping statement
/// before the tool reads anything at all.
#[test]
fn the_rehearsal_verb_requires_its_working_root_and_the_stopping_statement() {
    let root = TestRoot::new("usage");
    let source = released_source(&root);
    let source = source.to_str().expect("utf-8 source").to_string();
    let work = root.work().to_str().expect("utf-8 work").to_string();

    let (code, report) = run_tool(&["rehearse", "--data-root", &source, "--writers-stopped"]);
    assert_eq!(
        code, 2,
        "a missing working root is a usage refusal: {report}"
    );
    assert_eq!(report["error"], "work_root_required");

    let (code, report) = run_tool(&["rehearse", "--data-root", &source, "--work-root", &work]);
    assert_eq!(code, 2, "a missing statement is a usage refusal: {report}");
    assert_eq!(report["error"], "maintenance_confirmation_required");
    assert!(!root.work().exists(), "nothing ran");
}

/// The help text names the verb and the two arguments a rehearsal cannot be run without.
#[test]
fn the_help_text_names_the_rehearsal_and_its_inputs() {
    let output = std::process::Command::new(tool_binary())
        .arg("--help")
        .output()
        .expect("run the tool");
    let help = String::from_utf8(output.stdout).expect("utf-8 help");
    assert!(output.status.success());
    for token in [
        "rehearse --data-root <path> --work-root <directory> --writers-stopped",
        "--work-root <path>",
        "--keep-work-root",
    ] {
        assert!(help.contains(token), "the help text names {token}: {help}");
    }
}
