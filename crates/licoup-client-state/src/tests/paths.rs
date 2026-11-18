use std::fs;

use super::support::TestRoot;

#[test]
fn snapshot_identifiers_reject_traversal_and_unbounded_input() {
    let root = TestRoot::new("snapshot-id");
    assert!(crate::paths::snapshot_path(root.path(), "../../private").is_err());
    assert!(
        crate::paths::snapshot_path(
            root.path(),
            &format!(
                "snapshot-{}",
                "x".repeat(crate::policy::MAX_SNAPSHOT_ID_BYTES)
            )
        )
        .is_err()
    );
    assert!(crate::paths::snapshot_path(root.path(), "snapshot-safe_1").is_ok());
}

#[test]
fn internal_references_never_project_an_absolute_state_path() {
    let root = TestRoot::new("state-reference");
    let path = root.path().join("snapshot-safe.json");
    let reference = crate::paths::internal_state_reference("snapshots", &path);
    assert_eq!(reference, "snapshots/snapshot-safe.json");
    assert!(!reference.contains(root.path().to_str().unwrap()));
    assert_eq!(
        crate::paths::redacted_local_path(),
        crate::policy::REDACTED_LOCAL_PATH
    );
}

#[cfg(unix)]
#[test]
fn local_source_reader_rejects_a_symbolic_link_without_reading_the_referent() {
    use std::os::unix::fs::symlink;

    let root = TestRoot::new("source-link");
    let referent = root.path().join("referent");
    let source = root.path().join("source");
    fs::write(&referent, b"private-canary").unwrap();
    symlink(&referent, &source).unwrap();

    assert!(crate::paths::read_owned_local_text_bounded(&source, 1024).is_err());
    assert_eq!(fs::read(&referent).unwrap(), b"private-canary");
}

#[test]
fn local_source_reader_is_bounded_and_owner_checked() {
    let root = TestRoot::new("source-read");
    let source = root.path().join("source");
    fs::write(&source, b"bounded").unwrap();

    assert_eq!(
        crate::paths::read_owned_local_text_bounded(&source, 16).unwrap(),
        Some("bounded".to_string())
    );
    assert!(crate::paths::read_owned_local_text_bounded(&source, 3).is_err());
}
