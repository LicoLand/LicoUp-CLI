use super::super::ownership;
use super::support::test_store;
use crate::domain::agent_hub::contract::{InstallOwnership, LIFECYCLE_AVAILABLE, OWNERSHIP_OWNED};
use licoup_foundation::platform::file_security::{atomic_write_private_text, ensure_private_dir};

fn owned_codex() -> InstallOwnership {
    InstallOwnership {
        agent_id: "codex".to_string(),
        channel_id: "homebrew".to_string(),
        channel_kind: "homebrew".to_string(),
        package_coordinate: "codex".to_string(),
        installed_version: "0.41.0".to_string(),
        ownership: OWNERSHIP_OWNED.to_string(),
        lifecycle: LIFECYCLE_AVAILABLE.to_string(),
    }
}

#[test]
fn save_persists_toml_and_roundtrips() {
    let store = test_store("toml-roundtrip");
    ownership::save(&store, owned_codex()).unwrap();
    let path = ownership::hub_dir(&store).join("ownership.toml");
    assert!(path.is_file());
    assert!(!ownership::hub_dir(&store).join("ownership.json").is_file());
    let raw = std::fs::read_to_string(&path).unwrap();
    assert!(raw.contains("agent_id = \"codex\""));
    assert!(!raw.contains("agentId"));
    let loaded = ownership::load(&store).unwrap();
    assert_eq!(loaded.len(), 1);
    assert_eq!(loaded[0].agent_id, "codex");
    assert_eq!(loaded[0].channel_id, "homebrew");
    assert_eq!(loaded[0].installed_version, "0.41.0");
}

#[test]
fn load_rewrites_json_ownership_to_toml() {
    let store = test_store("json-to-toml");
    let hub = ownership::hub_dir(&store);
    ensure_private_dir(&hub).unwrap();
    atomic_write_private_text(
        &hub.join("ownership.json"),
        r#"{
  "items": [
    {
      "agentId": "codex",
      "channelId": "homebrew",
      "channelKind": "homebrew",
      "packageCoordinate": "codex",
      "installedVersion": "0.41.0",
      "ownership": "owned",
      "lifecycle": "available"
    }
  ]
}"#,
    )
    .unwrap();
    let loaded = ownership::load(&store).unwrap();
    assert_eq!(loaded.len(), 1);
    assert_eq!(loaded[0].agent_id, "codex");
    assert_eq!(loaded[0].package_coordinate, "codex");
    assert!(hub.join("ownership.toml").is_file());
    assert!(!hub.join("ownership.json").is_file());
}

#[test]
fn toml_wins_and_leftover_json_is_removed() {
    let store = test_store("toml-wins");
    ownership::save(&store, owned_codex()).unwrap();
    let hub = ownership::hub_dir(&store);
    atomic_write_private_text(
        &hub.join("ownership.json"),
        r#"{"items":[{"agentId":"cursor","channelId":"npm","channelKind":"npm","packageCoordinate":"cursor-agent","installedVersion":"1.0.0","ownership":"owned","lifecycle":"available"}]}"#,
    )
    .unwrap();
    let loaded = ownership::load(&store).unwrap();
    assert_eq!(loaded.len(), 1);
    assert_eq!(loaded[0].agent_id, "codex");
    assert!(!hub.join("ownership.json").is_file());
}
