use serde_json::json;
use std::fs;

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

#[test]
fn retention_keeps_a_complete_latest_suffix_through_the_published_store() {
    let root = TestRoot::new("retention");
    let store = ClientStateStore::new(root.path().join("state")).unwrap();
    let saved = store
        .write_collection_retaining_latest_items(
            "agent-usage-reports",
            json!([{ "index": 1 }, { "index": 2 }, { "index": 3 }]),
            2,
        )
        .unwrap();

    assert_eq!(saved["items"], json!([{ "index": 2 }, { "index": 3 }]));
    assert_eq!(store.read_collection("agent-usage-reports").unwrap(), saved);
}

#[test]
fn read_only_projection_does_not_create_a_missing_state_root() {
    let root = TestRoot::new("read-only-missing");
    let state = root.path().join("absent-state");
    let store = ClientStateStore::open_read_only(state.clone());

    assert_eq!(
        store.read_collection("settings").unwrap()["items"],
        json!([])
    );
    assert!(!state.exists());
}

/// The published store validated the path and then read the path again. A
/// document replaced by a symlink after that check must be refused, not
/// followed: the client's read opens the document itself with no-follow and
/// validates the opened descriptor.
///
/// The swap happens inside the reader's metadata-to-open window through the
/// foundation's test observation point, so this test exercises the race itself
/// instead of installing the link before the read begins.
#[cfg(unix)]
#[test]
fn a_collection_read_refuses_a_symlink_swapped_in_place() {
    use licoup_foundation::platform::file_security::observe_before_private_file_open;
    use std::os::unix::fs::PermissionsExt;
    use std::os::unix::fs::symlink;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};

    let root = TestRoot::new("collections-symlink");
    let state = root.path().join("state");
    let store = ClientStateStore::new(state.clone()).unwrap();
    let path = store.collection_path("settings").unwrap();
    let hostile = root.path().join("hostile.json");
    fs::write(
        &hostile,
        br#"{"schemaVersion":"v0.0.1:schema:definition-1","collection":"settings","items":[{"stolen":true}]}"#,
    )
    .unwrap();
    // Make the referent pass every metadata check: refusal must come from the
    // no-follow open itself, not from a permissive referent mode.
    fs::set_permissions(&hostile, fs::Permissions::from_mode(0o600)).unwrap();

    // The reader inspects the valid document first. The observer runs after
    // that inspection and before the no-follow open, which is exactly the
    // window the published path-and-read sequence left open.
    let swapped = Arc::new(AtomicBool::new(false));
    let path_for_observer = path.clone();
    let hostile_for_observer = hostile.clone();
    let swapped_for_observer = Arc::clone(&swapped);
    let _observer = observe_before_private_file_open(move |opening| {
        if opening == path_for_observer {
            fs::remove_file(&path_for_observer).unwrap();
            symlink(&hostile_for_observer, &path_for_observer).unwrap();
            swapped_for_observer.store(true, Ordering::SeqCst);
        }
    });

    let error = store.read_collection("settings").unwrap_err();
    assert!(
        swapped.load(Ordering::SeqCst),
        "the observer must replace the document between inspection and open"
    );
    assert!(
        fs::symlink_metadata(&path)
            .unwrap()
            .file_type()
            .is_symlink(),
        "the document path must hold the swapped symlink"
    );
    assert!(
        error.to_string().contains("could not be opened"),
        "the no-follow open must refuse the swapped symlink, got: {error}"
    );
}

/// Owner-only mode is a per-document property, not only a root property. The
/// published store did not check it on read; the client now does.
#[cfg(unix)]
#[test]
fn a_collection_read_refuses_a_document_whose_permissions_are_not_private() {
    use std::os::unix::fs::PermissionsExt;

    let root = TestRoot::new("collections-mode");
    let store = ClientStateStore::new(root.path().join("state")).unwrap();
    store
        .write_collection("settings", json!({"items": []}))
        .unwrap();
    let path = store.collection_path("settings").unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).unwrap();

    let error = store.read_collection("settings").unwrap_err();
    assert!(
        error.to_string().contains("permissions are insecure"),
        "a widening of the document mode must be refused, got: {error}"
    );
}

/// The migration probe reads through the same hardened boundary, so a
/// symlinked legacy document is refused before it can be adopted.
#[cfg(unix)]
#[test]
fn migration_probe_refuses_a_symlinked_collection() {
    use std::os::unix::fs::symlink;

    let root = TestRoot::new("migration-symlink");
    let data_home = root.path().join("home");
    let store = ClientStateStore::new(data_home.join(crate::policy::CLIENT_STATE_DIR)).unwrap();
    store
        .write_collection("settings", json!({"items": []}))
        .unwrap();
    let path = store.collection_path("settings").unwrap();
    let hostile = root.path().join("hostile.json");
    fs::write(&hostile, br#"{"items":[{"stolen":true}]}"#).unwrap();
    fs::remove_file(&path).unwrap();
    symlink(&hostile, &path).unwrap();

    assert!(crate::probe_collections(&data_home).is_err());
}

/// Every publishing write confirms the containing directory after the store's
/// rename. The thread-local observation point proves the client actually takes
/// the durability step, rather than only documenting it.
#[test]
fn every_collection_write_confirms_parent_durability() {
    let root = TestRoot::new("collections-durability");
    let store = ClientStateStore::new(root.path().join("state")).unwrap();
    // `new` creates the fresh canonical collections and syncs each one.
    let _ = crate::collections::take_parent_sync_count();

    store
        .write_collection("settings", json!({"items": []}))
        .unwrap();
    assert_eq!(crate::collections::take_parent_sync_count(), 1);

    store
        .write_collection_retaining_latest_items("agent-usage-reports", json!([{ "index": 1 }]), 4)
        .unwrap();
    assert_eq!(crate::collections::take_parent_sync_count(), 1);

    store.write_target_routes(&[]).unwrap();
    assert_eq!(crate::collections::take_parent_sync_count(), 1);
}
