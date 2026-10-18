use serde_json::json;

use super::support::{PortableDataDirOverrideGuard, TestRoot};
use crate::{ClientStateStore, TARGET_DISCOVERY_CACHE_COLLECTION, TargetRouteRecord};

fn route(target: &str, binary_path: Option<&str>) -> TargetRouteRecord {
    TargetRouteRecord {
        schema_version: "licoup.target-discovery-cache.v1".to_string(),
        target: target.to_string(),
        binary_path: binary_path.map(str::to_string),
        config_path: None,
        scan_source: "executable-path".to_string(),
        runtime_ready: true,
        cached_at_epoch_seconds: 1,
        extension: serde_json::Map::new(),
    }
}

#[test]
fn first_portable_launch_creates_only_fresh_canonical_state() {
    let root = TestRoot::new("portable");
    let portable_root = root.path().join("portable-data");
    let _override = PortableDataDirOverrideGuard::set(portable_root.clone());

    let store = crate::ClientStateStore::portable().unwrap();

    assert_eq!(
        store.root(),
        portable_root.join(crate::policy::CLIENT_STATE_DIR)
    );
    assert!(store.root().join(crate::policy::SNAPSHOT_DIR).is_dir());
    assert!(store.root().join(crate::policy::ACTIVITY_DIR).is_dir());
    for collection in crate::policy::COLLECTIONS {
        let document = store.read_collection(collection).unwrap();
        assert_eq!(document["items"], json!([]));
    }
}

#[test]
fn collections_normalize_objects_and_scalar_item_lists_independently() {
    let root = TestRoot::new("collections");
    let store = crate::ClientStateStore::new(root.path().join("state")).unwrap();

    let object = store
        .write_collection("settings", json!({"items": [{"key": "theme"}]}))
        .unwrap();
    let scalar = store
        .write_collection("targets", json!([{"target": "opencode"}]))
        .unwrap();

    assert_eq!(object["collection"], "settings");
    assert_eq!(scalar["collection"], "targets");
    assert_eq!(scalar["items"][0]["target"], "opencode");
    assert!(store.read_collection("unsupported-collection").is_err());
}

#[test]
fn target_routes_decode_typed_records_and_round_trip_unknown_fields() {
    let root = TestRoot::new("target-routes");
    let store = ClientStateStore::new(root.path().join("state")).unwrap();
    let codex_binary = root
        .path()
        .join("fixture-bin/codex")
        .to_string_lossy()
        .into_owned();
    let document = json!({
        "schemaVersion": "v0.0.1:schema:definition-1",
        "collection": TARGET_DISCOVERY_CACHE_COLLECTION,
        "items": [
            {
                "schemaVersion": "licoup.target-discovery-cache.v1",
                "target": "codex",
                "binaryPath": codex_binary,
                "scanSource": "executable-path",
                "runtimeReady": true,
                "cachedAtEpochSeconds": 1,
                "futureExtension": { "nested": true }
            },
            {
                "schemaVersion": "licoup.target-discovery-cache.v1",
                "target": "cursor",
                "scanSource": "manual",
                "runtimeReady": false,
                "cachedAtEpochSeconds": 2,
                "extra": "kept"
            }
        ]
    });
    store
        .write_collection(TARGET_DISCOVERY_CACHE_COLLECTION, document)
        .unwrap();

    let first = store.read_target_routes().unwrap();
    assert_eq!(first.len(), 2);
    assert_eq!(first[0].target, "codex");
    assert_eq!(first[1].target, "cursor");
    assert_eq!(first[0].binary_path.as_deref(), Some(codex_binary.as_str()));
    assert!(first[0].runtime_ready);
    assert_eq!(
        first[0].extension.get("futureExtension"),
        Some(&json!({ "nested": true }))
    );
    assert_eq!(first[1].extension.get("extra"), Some(&json!("kept")));

    let _ = store.read_target_routes().unwrap();
    let _ = store.read_target_routes().unwrap();
    assert_eq!(store.target_index_parse_count(), 1);
    assert_eq!(store.target_index_invalidation_count(), 0);

    let codex = store.target_route("codex").unwrap().unwrap();
    assert_eq!(codex.binary_path.as_deref(), Some(codex_binary.as_str()));
    assert!(store.target_route("missing").unwrap().is_none());
    assert_eq!(store.target_index_parse_count(), 1);

    store.write_target_routes(&first).unwrap();
    let round_trip = store
        .read_collection(TARGET_DISCOVERY_CACHE_COLLECTION)
        .unwrap();
    assert_eq!(
        round_trip["items"][0]["futureExtension"],
        json!({ "nested": true })
    );
    assert_eq!(round_trip["items"][1]["extra"], json!("kept"));
    assert_eq!(store.target_index_parse_count(), 1);
}

#[test]
fn target_routes_reparse_exactly_once_when_the_file_generation_changes() {
    let root = TestRoot::new("target-routes-external");
    let state = root.path().join("state");
    let codex_binary = root
        .path()
        .join("fixture-bin/codex")
        .to_string_lossy()
        .into_owned();
    let cursor_binary = root
        .path()
        .join("fixture-bin/cursor-longer")
        .to_string_lossy()
        .into_owned();
    let owner = ClientStateStore::new(state.clone()).unwrap();
    owner
        .write_target_routes(&[route("codex", Some(codex_binary.as_str()))])
        .unwrap();

    let reader = ClientStateStore::new(state.clone()).unwrap();
    assert_eq!(
        reader.target_route("codex").unwrap().unwrap().target,
        "codex"
    );
    assert_eq!(reader.target_index_parse_count(), 1);
    assert_eq!(reader.target_index_invalidation_count(), 0);
    let _ = reader.target_route("codex").unwrap();
    assert_eq!(reader.target_index_parse_count(), 1);

    owner
        .write_target_routes(&[route("cursor", Some(cursor_binary.as_str()))])
        .unwrap();
    assert!(reader.target_route("codex").unwrap().is_none());
    assert_eq!(
        reader.target_route("cursor").unwrap().unwrap().target,
        "cursor"
    );
    assert_eq!(reader.target_index_parse_count(), 2);
    assert_eq!(reader.target_index_invalidation_count(), 1);
}

#[test]
fn target_routes_reject_malformed_documents_without_side_effects() {
    let root = TestRoot::new("target-routes-malformed");
    let store = ClientStateStore::new(root.path().join("state")).unwrap();
    store
        .write_collection(
            TARGET_DISCOVERY_CACHE_COLLECTION,
            json!({ "items": [{ "target": "codex" }] }),
        )
        .unwrap();
    assert!(store.read_target_routes().is_err());
    assert!(store.target_route("codex").is_err());
    assert!(store.read_collection("unsupported-collection").is_err());
}

#[test]
fn target_routes_never_leak_across_store_roots() {
    let root = TestRoot::new("target-routes-cross-root");
    let codex_binary = root
        .path()
        .join("fixture-bin/codex")
        .to_string_lossy()
        .into_owned();
    let first = ClientStateStore::new(root.path().join("first")).unwrap();
    first
        .write_target_routes(&[route("codex", Some(codex_binary.as_str()))])
        .unwrap();
    let second = ClientStateStore::new(root.path().join("second")).unwrap();
    assert!(second.target_route("codex").unwrap().is_none());
    assert_eq!(second.read_target_routes().unwrap(), Vec::new());
    assert_eq!(second.target_index_parse_count(), 1);
}

#[test]
fn duplicate_target_routes_preserve_first_document_match() {
    let root = TestRoot::new("target-routes-duplicate");
    let first_binary = root
        .path()
        .join("fixture-bin/first")
        .to_string_lossy()
        .into_owned();
    let second_binary = root
        .path()
        .join("fixture-bin/second")
        .to_string_lossy()
        .into_owned();
    let store = ClientStateStore::new(root.path().join("state")).unwrap();
    store
        .write_target_routes(&[
            route("codex", Some(first_binary.as_str())),
            route("codex", Some(second_binary.as_str())),
        ])
        .unwrap();

    assert_eq!(
        store
            .target_route("codex")
            .unwrap()
            .unwrap()
            .binary_path
            .as_deref(),
        Some(first_binary.as_str())
    );
}
