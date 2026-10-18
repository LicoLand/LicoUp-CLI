use serde_json::{Value, json};
use std::fs;

use super::support::TestRoot;

#[test]
fn snapshot_store_lists_and_restores_without_projecting_local_paths() {
    let root = TestRoot::new("snapshots");
    let store = crate::ClientStateStore::new(root.path().join("state")).unwrap();
    let source = store.root().join("target-config.json");
    fs::write(&source, r#"{"before":true}"#).unwrap();
    let snapshot = store
        .snapshot_store()
        .capture("opencode", &source, json!({"operation": "test"}))
        .unwrap();
    fs::write(&source, r#"{"after":true}"#).unwrap();

    let listed = store
        .snapshot_store()
        .list(&json!({"target": "opencode"}))
        .unwrap();
    assert_eq!(listed["snapshots"].as_array().unwrap().len(), 1);
    assert_eq!(
        listed["snapshots"][0]["sourcePath"],
        crate::policy::REDACTED_LOCAL_PATH
    );
    assert!(
        listed["snapshots"][0]["snapshotPath"]
            .as_str()
            .unwrap()
            .starts_with("snapshots/snapshot-")
    );
    assert!(!listed.to_string().contains(root.path().to_str().unwrap()));

    let restored = store
        .snapshot_store()
        .restore(&snapshot.snapshot_id)
        .unwrap();
    assert_eq!(restored["status"], "restored");
    assert_eq!(restored["sourcePath"], crate::policy::REDACTED_LOCAL_PATH);
    assert_eq!(fs::read_to_string(&source).unwrap(), r#"{"before":true}"#);
}

#[test]
fn snapshot_capture_redacts_content_and_metadata_before_private_persistence() {
    let root = TestRoot::new("snapshot-redaction");
    let store = crate::ClientStateStore::new(root.path().join("state")).unwrap();
    let source = store.root().join("target-config.json");
    let snapshot_key_canary = ["snapshot", "key", "canary"].join("-");
    let url_canary = ["url", "canary"].join("-");
    let metadata_canary = ["metadata", "canary"].join("-");
    fs::write(
        &source,
        json!({
            "apiKey": snapshot_key_canary.clone(),
            "secretRef": "secret://local/ref",
            "webUrl": format!("https://example.invalid/?access_token={url_canary}"),
        })
        .to_string(),
    )
    .unwrap();

    let snapshot = store
        .snapshot_store()
        .capture(
            "opencode",
            &source,
            json!({"configPath": source, "token": metadata_canary.clone()}),
        )
        .unwrap();
    let snapshot_raw = fs::read_to_string(&snapshot.snapshot_path).unwrap();

    for canary in [&snapshot_key_canary, &url_canary, &metadata_canary] {
        assert!(!snapshot_raw.contains(canary));
    }
    assert!(snapshot_raw.contains(crate::policy::REDACTED_SECRET));
    assert!(snapshot_raw.contains(crate::policy::REDACTED_LOCAL_PATH));
    assert!(snapshot_raw.contains("secret://local/ref"));
    let snapshot_doc: Value = serde_json::from_str(&snapshot_raw).unwrap();
    assert_eq!(snapshot_doc["redaction"]["applied"], true);
}

#[test]
fn snapshot_restore_rejects_traversal_and_identity_mismatch() {
    let root = TestRoot::new("snapshot-identity");
    let store = crate::ClientStateStore::new(root.path().join("state")).unwrap();

    assert!(store.snapshot_store().restore("../../private").is_err());

    let mismatched = store
        .root()
        .join(crate::policy::SNAPSHOT_DIR)
        .join("snapshot-safe.json");
    crate::serialization::atomic_write_json(
        &mismatched,
        &json!({"snapshotId": "snapshot-other"}),
        crate::policy::MAX_SNAPSHOT_RECORD_BYTES,
    )
    .unwrap();
    assert!(store.snapshot_store().restore("snapshot-safe").is_err());
}

#[test]
fn snapshot_source_size_is_checked_before_reading_or_persisting() {
    let root = TestRoot::new("snapshot-bound");
    let store = crate::ClientStateStore::new(root.path().join("state")).unwrap();
    let source = store.root().join("oversized");
    let file = fs::File::create(&source).unwrap();
    file.set_len((crate::policy::MAX_SNAPSHOT_SOURCE_BYTES as u64) + 1)
        .unwrap();
    drop(file);

    assert!(
        store
            .snapshot_store()
            .capture("opencode", &source, json!({}))
            .is_err()
    );
    assert_eq!(
        fs::read_dir(store.root().join(crate::policy::SNAPSHOT_DIR))
            .unwrap()
            .count(),
        0
    );
}
