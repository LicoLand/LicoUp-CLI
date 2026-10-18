use super::support::*;

#[test]
fn explicit_state_roots_isolate_model_registry_for_fresh_and_retained_usage() {
    let fixture = temp_dir("usage-registry-state-scope");
    let history = fixture.join("history");
    let first = fixture.join("first");
    let second = fixture.join("second");
    fs::create_dir_all(&history).unwrap();
    fs::write(history.join("rollout.jsonl"), [
        json!({"timestamp":"2026-07-10T10:00:00Z","type":"session_meta","payload":{"id":"synthetic-registry-session"}}).to_string(),
        json!({"timestamp":"2026-07-10T10:00:00Z","type":"event_msg","payload":{"type":"task_started","turn_id":"synthetic-turn"}}).to_string(),
        json!({"timestamp":"2026-07-10T10:00:00Z","type":"turn_context","payload":{"turn_id":"synthetic-turn","model":"scoped-selector","effort":"high"}}).to_string(),
        token_event("2026-07-10T10:00:01Z", (6, 2, 4), (6, 2, 4)),
    ].join("\n") + "\n").unwrap();
    let write_catalog = |root: &PathBuf, id: &str| {
        fs::create_dir_all(root.join("model-registry")).unwrap();
        fs::write(
            root.join("model-registry/catalog.json"),
            json!({
                "source":"synthetic-public-catalog",
                "fetchedAt":"0",
                "skippedEntries":0,
                "catalog": {
                    "models": {id: {"name":"scoped-selector"}},
                    "providers": {}
                }
            })
            .to_string(),
        )
        .unwrap();
    };
    write_catalog(&first, "example/model-1");
    let first_report = agent_usage::scan(&scan_params(&history, &first)).unwrap();
    assert_eq!(
        first_report["agents"][0]["history"]["dailyUsage"][0]["modelTokenUsage"]["example/model-1"]
            ["totalTokens"],
        10
    );

    let second_report = agent_usage::scan(&scan_params(&history, &second)).unwrap();
    assert_eq!(second_report["modelRegistryRevision"], "");
    assert_eq!(
        second_report["agents"][0]["history"]["dailyUsage"][0]["modelTokenUsage"]["scoped-selector"]
            ["totalTokens"],
        10
    );

    write_catalog(&second, "example/model-2");
    for (root, id) in [(&second, "example/model-2"), (&first, "example/model-1")] {
        let retained = agent_usage::report(&json!({"stateRoot":root.to_string_lossy()})).unwrap();
        assert!(
            !retained["modelRegistryRevision"]
                .as_str()
                .unwrap()
                .is_empty()
        );
        assert_eq!(
            retained["reports"][0]["agents"][0]["history"]["dailyUsage"][0]["modelTokenUsage"][id]
                ["totalTokens"],
            10
        );
    }
    fs::remove_dir_all(fixture).unwrap();
}

#[test]
fn retained_reports_keep_only_current_contract_and_sort_by_timestamp() {
    let state_root = temp_dir("usage-report-current-contract-state");
    let store = ClientStateStore::new(state_root.clone()).unwrap();
    store
        .write_collection(
            "agent-usage-reports",
            json!({
                "items": [
                    {
                        "schemaVersion": 7,
                        "mode": "local-token-usage",
                        "tokenSourceMode": "native-metadata-first-incremental",
                        "generatedAt": "2026-07-10T12:00:00Z",
                        "summary": {"totalTokens": 12},
                        "agents": []
                    },
                    {
                        "schemaVersion": 7,
                        "mode": "invalid-mode",
                        "tokenSourceMode": "native-metadata-first-incremental",
                        "generatedAt": "2026-07-11T12:00:00Z",
                        "summary": {"totalTokens": 999},
                        "agents": []
                    },
                    {
                        "schemaVersion": 7,
                        "mode": "local-token-usage",
                        "tokenSourceMode": "native-metadata-first-incremental",
                        "generatedAt": "2026-07-09T12:00:00Z",
                        "summary": {"totalTokens": 9},
                        "agents": []
                    }
                ]
            }),
        )
        .unwrap();

    let listed = agent_usage::report(&json!({
        "stateRoot": state_root.to_string_lossy(),
        "limit": 10
    }))
    .unwrap();
    let reports = listed["reports"].as_array().unwrap();
    assert_eq!(reports.len(), 2);
    assert_eq!(reports[0]["summary"]["totalTokens"], 12);
    assert_eq!(reports[1]["summary"]["totalTokens"], 9);

    let retained = store.read_collection("agent-usage-reports").unwrap();
    let items = retained["items"].as_array().unwrap();
    assert_eq!(items.len(), 2);
    assert!(items.iter().all(|item| item["schemaVersion"] == 7));
    assert!(items.iter().all(|item| item["mode"] == "local-token-usage"));
    assert!(
        items
            .iter()
            .all(|item| { item["tokenSourceMode"] == "native-metadata-first-incremental" })
    );
    assert_eq!(items[0]["summary"]["totalTokens"], 9);
    assert_eq!(items[1]["summary"]["totalTokens"], 12);
    assert!(!retained.to_string().contains("invalid-mode"));
}
