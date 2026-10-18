use super::support::*;
use std::fs;

#[test]
fn cli_dispatches_state_targets_and_mobile_relay() {
    let dir = temp_cli_dir("dispatch");
    {
        let _guard = cli_env_guard();
        let _portable = set_portable_dir(&dir);
        let set_state = execute_cli(vec![
            "state".into(),
            "set".into(),
            "targets".into(),
            r#"{"items": []}"#.into(),
        ])
        .unwrap();
        assert_eq!(json_payload(&set_state)["ok"], true);

        let get_state = execute_cli(vec!["state".into(), "get".into(), "targets".into()]).unwrap();
        let got = json_payload(&get_state);
        assert_eq!(got["collection"], "targets");

        let activities = execute_cli(vec![
            "activity".into(),
            "list".into(),
            "--limit".into(),
            "5".into(),
        ])
        .unwrap();
        assert!(json_payload(&activities)["ok"].as_bool().unwrap_or(false));

        let list_targets = execute_cli(vec![
            "targets".into(),
            "scan".into(),
            "--state-root".into(),
            dir.join("client-state").display().to_string(),
        ])
        .unwrap();
        assert_eq!(json_payload(&list_targets)["ok"], true);
        let inspect_target =
            execute_cli(vec!["targets".into(), "inspect".into(), "opencode".into()]).unwrap();
        assert_eq!(
            json_payload(&inspect_target)["target"]["target"],
            "opencode"
        );

        let secret_store: std::sync::Arc<
            dyn licoup_native::platform::secure_mesh_secret_store::SecureMeshSecretStore,
        > = std::sync::Arc::new(
            licoup_native::platform::secure_mesh_secret_store::EphemeralSecretStore::new(),
        );
        let relay_config =
            licoup_native::domain::mobile_relay::with_mobile_relay_secret_store_override(
                secret_store,
                || {
                    execute_cli(vec![
                        "mobile".into(),
                        "relay".into(),
                        "config".into(),
                        "set".into(),
                        "--station-base-url".into(),
                        "https://relay.example.test/".into(),
                    ])
                },
            )
            .unwrap();
        assert_eq!(
            json_payload(&relay_config)["config"]["stationBaseUrl"],
            "https://relay.example.test"
        );
    }
}

#[test]
fn cli_wraps_client_conversation_results_for_the_desktop_runner() {
    let dir = temp_cli_dir("client-conversation-cli");
    {
        let _guard = cli_env_guard();
        let _portable = set_portable_dir(&dir);
        let result = execute_cli(vec![
            "conversation".into(),
            "execute".into(),
            "--stdin-json".into(),
            r#"{"action":"conversation.list","includeArchived":false}"#.into(),
        ])
        .unwrap();
        let payload = json_payload(&result);
        assert_eq!(payload["ok"], true);
        let conversations = payload["result"].as_array().unwrap();
        assert_eq!(
            conversations.len(),
            1,
            "startup pins exactly one canonical Local group"
        );
        assert_eq!(conversations[0]["id"], "lico-group-default");
        assert_eq!(conversations[0]["title"], "Local");
        assert_eq!(conversations[0]["isGroup"], true);
        assert_eq!(conversations[0]["pinned"], true);
        assert_eq!(conversations[0]["archived"], false);
        assert_eq!(conversations[0]["eventCount"], 0);
        assert_eq!(conversations[0]["membershipCount"], 1);
        assert_eq!(conversations[0]["revision"], 1);
        assert!(
            conversations[0].get("updatedAtUnixMs").is_some(),
            "canonical Local projection keeps its recency fact"
        );
    }
    let _ = fs::remove_dir_all(dir);
}
