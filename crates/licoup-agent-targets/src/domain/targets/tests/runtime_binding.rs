use super::super::available_runtime_executable;
use super::test_support::port;
use licoup_client_state::{ClientStateStore, TARGET_DISCOVERY_CACHE_SCHEMA, TargetRouteRecord};
use serde_json::Map;
use std::fs;
use std::sync::Mutex;

static PORTABLE_OVERRIDE_TEST: Mutex<()> = Mutex::new(());

#[test]
fn runtime_binding_rejects_unknown_targets() {
    assert!(available_runtime_executable(&port(), "unknown-target").is_none());
}

#[test]
fn runtime_binding_reuses_each_explicit_root_without_cross_root_routes() {
    let _serial = PORTABLE_OVERRIDE_TEST.lock().unwrap();
    let parent = std::env::temp_dir().join(format!(
        "lico-target-binding-roots-{}",
        uuid::Uuid::new_v4()
    ));
    let first_root = parent.join("first");
    let second_root = parent.join("second");
    let first_binary = parent.join("fixture-bin/first");
    let second_binary = parent.join("fixture-bin/second");
    fs::create_dir_all(first_binary.parent().unwrap()).unwrap();
    fs::write(&first_binary, b"").unwrap();
    fs::write(&second_binary, b"").unwrap();

    let previous =
        licoup_foundation::platform::paths::set_portable_data_dir_override(Some(first_root));
    let first_store = ClientStateStore::portable().unwrap();
    first_store
        .write_target_routes(&[route(&first_binary)])
        .unwrap();
    assert_eq!(
        available_runtime_executable(&port(), "codex"),
        Some(fs::canonicalize(&first_binary).unwrap())
    );

    licoup_foundation::platform::paths::set_portable_data_dir_override(Some(second_root));
    let second_store = ClientStateStore::portable().unwrap();
    second_store
        .write_target_routes(&[route(&second_binary)])
        .unwrap();
    assert_eq!(
        available_runtime_executable(&port(), "codex"),
        Some(fs::canonicalize(&second_binary).unwrap())
    );

    licoup_foundation::platform::paths::set_portable_data_dir_override(previous);
    let _ = fs::remove_dir_all(parent);
}

fn route(binary: &std::path::Path) -> TargetRouteRecord {
    TargetRouteRecord {
        schema_version: TARGET_DISCOVERY_CACHE_SCHEMA.to_string(),
        target: "codex".to_string(),
        binary_path: Some(binary.to_string_lossy().into_owned()),
        config_path: None,
        scan_source: "fixture".to_string(),
        runtime_ready: true,
        cached_at_epoch_seconds: 1,
        extension: Map::new(),
    }
}
