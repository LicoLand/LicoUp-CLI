//! Behaviour of the standalone tool against a real client projection.
//!
//! The oracle is a comparison with the client's own owners, not a snapshot of this
//! tool's output: the same root is read twice, once through the library call the tool
//! makes and once through the binary it ships, and the two must agree. A tool that
//! derived a version locally would fail here as soon as the client's frontier moved.

mod support;

use std::path::{Path, PathBuf};
use support::run_tool as run;

fn scratch(name: &str) -> PathBuf {
    let base = std::env::temp_dir()
        .canonicalize()
        .expect("canonical temporary directory");
    let root = base.join(format!("licoup-migrate-{name}-{}", std::process::id()));
    if root.exists() {
        std::fs::remove_dir_all(&root).expect("clear scratch root");
    }
    std::fs::create_dir_all(&root).expect("create scratch root");
    root
}

fn root_fingerprint(root: &Path) -> Vec<(String, u64)> {
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
fn inspect_agrees_with_the_client_projection_on_every_domain() {
    let root = scratch("inspect");

    let (code, report) = run(&["inspect", "--data-root", root.to_str().expect("utf-8 root")]);
    assert_eq!(code, 0, "inspect must succeed on an existing root");
    assert_eq!(report["status"], "inspected");

    // The oracle: the same root read through the client's own owners.
    let frontier = licoup_native::domain::client_state_migration::frontier_projection_struct()
        .expect("frontier");
    let expected = licoup_native::domain::client_state_migration::domain_state_projection(&root)
        .expect("state");

    assert_eq!(
        report["frontierId"].as_str().expect("frontier id"),
        frontier.frontier_id,
        "the tool must report the client's frontier, not one of its own"
    );

    let reported = report["domains"].as_array().expect("domains");
    assert_eq!(
        reported.len(),
        expected.len(),
        "every domain the client reports must appear"
    );
    for state in &expected {
        let entry = reported
            .iter()
            .find(|entry| entry["domainId"] == state.domain_id.as_str())
            .unwrap_or_else(|| panic!("{} is missing from the report", state.domain_id));
        // The report mirrors the client's own authority states instead of flattening
        // them: an absent root is absent, and a refused domain names its owner code.
        let effective = match &state.authority {
            licoup_native::domain::client_state_migration::DomainAuthority::Known { version } => {
                assert_eq!(entry["authority"]["state"], "known");
                assert_eq!(entry["authority"]["version"], *version);
                Some(*version)
            }
            licoup_native::domain::client_state_migration::DomainAuthority::Absent => {
                assert_eq!(entry["authority"]["state"], "absent");
                Some(0)
            }
            licoup_native::domain::client_state_migration::DomainAuthority::Refused { code } => {
                assert_eq!(entry["authority"]["state"], "refused");
                assert_eq!(entry["authority"]["code"], code.as_str());
                None
            }
        };
        assert_eq!(entry["effectiveVersion"].as_u64(), effective.map(u64::from));
        assert_eq!(entry["targetSchemaVersion"], state.target_schema_version);
        let target = frontier
            .domains
            .iter()
            .find(|domain| domain.domain_id == state.domain_id)
            .expect("declared target");
        assert_eq!(
            entry["atTarget"],
            effective == Some(target.target_schema_version)
        );
    }
}

#[test]
fn plan_marks_the_domains_only_the_client_can_convert() {
    let root = scratch("plan");
    let (code, report) = run(&["plan", "--data-root", root.to_str().expect("utf-8 root")]);
    assert_eq!(code, 0);
    assert_eq!(report["status"], "planned");

    let domains = report["domains"].as_array().expect("domains");
    assert!(!domains.is_empty(), "a fresh root owes work");
    let owner_only: Vec<&str> = domains
        .iter()
        .filter(|domain| {
            domain["steps"]
                .as_array()
                .is_some_and(|steps| steps.iter().any(|step| step["ownerOnly"] == true))
        })
        .filter_map(|domain| domain["domainId"].as_str())
        .collect();
    assert!(
        owner_only.contains(&"canonical-conversation"),
        "the conversation import belongs to the client's owner"
    );
    assert!(
        owner_only.contains(&"adaptive-flywheel"),
        "the strategy-store ladder belongs to the client's owner"
    );
}

#[test]
fn a_missing_root_is_refused_and_writes_nothing() {
    let root = scratch("missing");
    let absent = root.join("absent");
    let before = root_fingerprint(&root);

    let (code, report) = run(&["inspect", "--data-root", absent.to_str().expect("utf-8")]);
    assert_eq!(code, 1, "a refused read exits non-zero");
    assert_eq!(report["status"], "refused");
    assert_eq!(report["error"], "data_root_missing");
    assert_eq!(
        before,
        root_fingerprint(&root),
        "a refused read must not touch the filesystem"
    );
}

#[test]
fn a_file_where_a_root_is_expected_is_refused() {
    let root = scratch("not-a-directory");
    let file = root.join("plain.txt");
    std::fs::write(&file, b"not a root").expect("write file");

    let (code, report) = run(&["inspect", "--data-root", file.to_str().expect("utf-8")]);
    assert_eq!(code, 1);
    assert_eq!(report["error"], "data_root_not_directory");
}

#[test]
fn an_unknown_target_is_refused() {
    let root = scratch("unknown-target");
    let (code, report) = run(&[
        "plan",
        "--data-root",
        root.to_str().expect("utf-8 root"),
        "--target",
        "no-such-target",
    ]);
    assert_eq!(code, 1);
    assert_eq!(report["error"], "migration_target_unsupported");
}

#[test]
fn usage_failures_exit_two_and_name_the_problem() {
    let (code, report) = run(&["migrate", "--data-root", "/tmp"]);
    assert_eq!(code, 2);
    assert_eq!(report["error"], "unknown_verb migrate");

    let (code, report) = run(&["inspect"]);
    assert_eq!(code, 2);
    assert_eq!(report["error"], "data_root_required");

    let (code, report) = run(&["inspect", "--data-root", "/tmp", "--watch"]);
    assert_eq!(code, 2);
    assert_eq!(report["error"], "unknown_option --watch");
}
