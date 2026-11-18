use super::support::TestRoot;
use serde_json::json;

#[test]
fn stable_accessors_derive_independent_owner_paths_without_storing_each_other() {
    let root = TestRoot::new("accessors");
    let store = crate::ClientStateStore::new(root.path().join("state")).unwrap();

    store
        .activity_log()
        .append("accessor.checked", json!({"target": "state"}))
        .unwrap();
    assert!(
        store
            .root()
            .join(crate::policy::ACTIVITY_DIR)
            .join(crate::policy::ACTIVITY_FILE)
            .is_file()
    );
    assert_eq!(
        store.snapshot_store().list(&json!({})).unwrap()["path"],
        crate::policy::SNAPSHOT_DIR
    );
}
