use super::super::RuntimeAdapter;
use super::super::artifact::{runtime_artifact_digest, runtime_executable};
use super::super::params::timestamp;
use licoup_client_state::{ClientStateStore, TARGET_DISCOVERY_CACHE_SCHEMA, TargetRouteRecord};
use serde_json::Map;
use std::fs;

#[test]
fn runtime_artifact_digest_tracks_the_opened_file_identity_and_content() {
    super::compose();
    let root = std::env::temp_dir().join(format!(
        "lico-runtime-artifact-{}-{}",
        std::process::id(),
        timestamp()
    ));
    fs::create_dir_all(&root).unwrap();
    let executable = root.join("runtime-canary");
    fs::write(&executable, b"accepted-runtime").unwrap();
    let first = runtime_artifact_digest(&executable).unwrap();
    fs::write(&executable, b"different-runtime").unwrap();
    let second = runtime_artifact_digest(&executable).unwrap();
    let _ = fs::remove_dir_all(root);

    assert!(first.starts_with("sha256:"));
    assert_ne!(first, second);
}

#[test]
fn kilo_default_command_uses_the_native_discovery_binding_for_group_turns() {
    super::compose();
    let root = std::env::temp_dir().join(format!(
        "lico-runtime-binding-{}-{}",
        std::process::id(),
        timestamp()
    ));
    let executable = root.join("extension/bin/kilo");
    fs::create_dir_all(executable.parent().unwrap()).unwrap();
    fs::write(&executable, b"fixture").unwrap();
    let previous =
        licoup_foundation::platform::paths::set_portable_data_dir_override(Some(root.clone()));
    let store = ClientStateStore::portable().unwrap();
    store
        .write_target_routes(&[TargetRouteRecord {
            schema_version: TARGET_DISCOVERY_CACHE_SCHEMA.to_string(),
            target: "kilo-code".to_string(),
            binary_path: Some(executable.to_string_lossy().into_owned()),
            config_path: None,
            scan_source: "fixture-extension".to_string(),
            runtime_ready: true,
            cached_at_epoch_seconds: 1,
            extension: Map::new(),
        }])
        .unwrap();

    let resolved = runtime_executable(
        &crate::target_port::agent_target_port(),
        RuntimeAdapter::KiloCode,
        "kilo",
    )
    .unwrap();
    licoup_foundation::platform::paths::set_portable_data_dir_override(previous);

    assert_eq!(
        resolved,
        fs::canonicalize(&executable)
            .unwrap()
            .to_string_lossy()
            .into_owned()
    );
    let _ = fs::remove_dir_all(root);
}

#[test]
fn explicit_relative_command_is_not_replaced_by_discovery() {
    super::compose();
    assert_eq!(
        runtime_executable(
            &crate::target_port::agent_target_port(),
            RuntimeAdapter::KiloCode,
            "custom-kilo",
        )
        .unwrap(),
        "custom-kilo"
    );
}
