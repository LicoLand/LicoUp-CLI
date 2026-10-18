//! Conversion behaviour through the client's own owner.
//!
//! The tool does not implement any domain move, so what these cases prove is the
//! contract around the owner: a conversion is reported per domain, a refusal is refused,
//! nothing outside the root is touched, and a second run does not claim new work.

mod support;

use licoup_native::platform::llm_api_key_vault::{
    LegacyCredentialMigrationDisposition, PlatformLlmApiKeyVault,
};
use std::path::{Path, PathBuf};
use support::run_tool as run;

fn scratch(name: &str) -> PathBuf {
    let base = std::env::temp_dir()
        .canonicalize()
        .expect("canonical temporary directory");
    let root = base.join(format!("licoup-convert-{name}-{}", std::process::id()));
    if root.exists() {
        std::fs::remove_dir_all(&root).expect("clear scratch root");
    }
    std::fs::create_dir_all(&root).expect("create scratch root");
    root
}

fn fingerprint(root: &Path) -> Vec<(String, u64)> {
    let mut entries = Vec::new();
    collect(root, root, &mut entries);
    entries.sort();
    entries
}

fn collect(root: &Path, directory: &Path, entries: &mut Vec<(String, u64)>) {
    let Ok(read) = std::fs::read_dir(directory) else {
        return;
    };
    for entry in read.filter_map(|entry| entry.ok()) {
        let path = entry.path();
        let relative = path
            .strip_prefix(root)
            .map(|value| value.to_string_lossy().into_owned())
            .unwrap_or_default();
        let metadata = std::fs::symlink_metadata(&path).expect("metadata");
        if metadata.is_dir() {
            entries.push((format!("{relative}/"), 0));
            collect(root, &path, entries);
        } else {
            entries.push((relative, metadata.len()));
        }
    }
}

#[test]
fn convert_reports_every_owed_domain_and_claims_no_more_than_the_owner() {
    let root = scratch("reported");
    let (code, report) = run(&[
        "convert",
        "--data-root",
        root.to_str().expect("utf-8 root"),
        "--writers-stopped",
    ]);
    assert_eq!(report["frontierId"], "licoup-state-0.3.0");

    let domains = report["domains"].as_array().expect("domains");
    assert!(!domains.is_empty(), "a fresh root owes work");
    for domain in domains {
        let outcome = domain["outcome"].as_str().expect("outcome");
        assert!(
            [
                "converted",
                "already-current",
                "pending-authorization",
                "still-owed"
            ]
            .contains(&outcome),
            "unexpected outcome {outcome}"
        );
    }

    // Nothing is reported as complete unless the client's owner claimed it: the owed
    // list and the outcomes must agree.
    let still_owed: Vec<&str> = report["stillOwed"]
        .as_array()
        .expect("stillOwed")
        .iter()
        .filter_map(|value| value.as_str())
        .collect();
    for domain in domains {
        let id = domain["domainId"].as_str().expect("domain id");
        let outcome = domain["outcome"].as_str().expect("outcome");
        let unresolved = matches!(outcome, "still-owed" | "pending-authorization");
        assert_eq!(
            still_owed.contains(&id),
            unresolved,
            "{id} is reported as {outcome} but its owed state disagrees"
        );
    }

    // A conversion the owner could not finish is never reported as a success, in the
    // report or in the exit status a script reads.
    let status = report["status"].as_str().expect("status");
    if still_owed.is_empty() {
        assert_eq!(code, 0);
        assert_eq!(status, "converted");
    } else {
        assert_eq!(code, 1, "an unfinished conversion exits non-zero: {report}");
        assert_ne!(status, "converted");
        assert!(
            ["pendingAuthorization", "partial"].contains(&status),
            "unexpected status {status}"
        );
    }
}

#[test]
fn a_fresh_conversion_reports_the_platform_credential_owners_verdict() {
    let root = scratch("owed");
    let (code, report) = run(&[
        "convert",
        "--data-root",
        root.to_str().expect("utf-8 root"),
        "--writers-stopped",
    ]);
    let still_owed = report["stillOwed"].as_array().expect("stillOwed");
    let requires_authorization = PlatformLlmApiKeyVault::legacy_credential_migration_disposition()
        .expect("platform credential disposition")
        == LegacyCredentialMigrationDisposition::RequiresAuthorization;
    assert_eq!(!still_owed.is_empty(), requires_authorization);
    assert_eq!(code, if requires_authorization { 1 } else { 0 });
    assert_eq!(
        report["status"],
        if requires_authorization {
            "pendingAuthorization"
        } else {
            "converted"
        },
        "the report follows the credential owner's platform verdict: {report}"
    );
}

#[test]
fn convert_without_the_operators_statement_is_refused_and_mutates_nothing() {
    let root = scratch("unconfirmed");
    std::fs::write(root.join("untouched.txt"), b"source").expect("seed");
    let before = fingerprint(&root);

    let (code, report) = run(&["convert", "--data-root", root.to_str().expect("utf-8 root")]);
    assert_eq!(code, 2, "the statement is required before anything runs");
    assert_eq!(report["status"], "refused");
    assert_eq!(report["error"], "maintenance_confirmation_required");
    assert_eq!(
        before,
        fingerprint(&root),
        "a refused conversion must not open or move anything"
    );
}

#[test]
fn a_second_run_reports_no_new_conversion() {
    let root = scratch("idempotent");
    let (_, first) = run(&[
        "convert",
        "--data-root",
        root.to_str().expect("utf-8"),
        "--writers-stopped",
    ]);
    let first_converted = first["domains"]
        .as_array()
        .expect("domains")
        .iter()
        .filter(|domain| domain["outcome"] == "converted")
        .count();
    assert!(first_converted > 0, "the first run converts something");

    let (second_code, second) = run(&[
        "convert",
        "--data-root",
        root.to_str().expect("utf-8"),
        "--writers-stopped",
    ]);
    let second_converted = second["domains"]
        .as_array()
        .expect("domains")
        .iter()
        .filter(|domain| domain["outcome"] == "converted")
        .count();
    assert_eq!(
        second_converted, 0,
        "a completed conversion must not be claimed twice"
    );
    if second["stillOwed"]
        .as_array()
        .expect("stillOwed")
        .is_empty()
    {
        assert_eq!(second_code, 0);
    }
}

#[test]
fn convert_refuses_a_missing_root_and_writes_nothing() {
    let root = scratch("refused");
    let absent = root.join("absent");
    let before = fingerprint(&root);

    let (code, report) = run(&[
        "convert",
        "--data-root",
        absent.to_str().expect("utf-8"),
        "--writers-stopped",
    ]);
    assert_eq!(code, 1, "a refused conversion exits non-zero");
    assert_eq!(report["status"], "refused");
    assert_eq!(report["error"], "data_root_missing");
    assert_eq!(
        before,
        fingerprint(&root),
        "a refused conversion must not create the root it was pointed at"
    );
}

#[test]
fn conversion_stays_inside_the_root_it_was_given() {
    let root = scratch("contained");
    let outside = scratch("contained-outside");
    let marker = outside.join("must-survive.txt");
    std::fs::write(&marker, b"untouched").expect("write marker");
    let before = fingerprint(&outside);

    let (code, report) = run(&[
        "convert",
        "--data-root",
        root.to_str().expect("utf-8 root"),
        "--writers-stopped",
    ]);
    // The exit status follows the report: a root that still owes the protected credential
    // domain has not finished converting, and a root that owes nothing exits zero.
    if report["stillOwed"]
        .as_array()
        .expect("stillOwed")
        .is_empty()
    {
        assert_eq!(code, 0);
    } else {
        assert_eq!(code, 1);
    }
    assert_eq!(
        before,
        fingerprint(&outside),
        "a conversion must not touch anything outside its root"
    );
    assert_eq!(std::fs::read(&marker).expect("marker"), b"untouched");
}
