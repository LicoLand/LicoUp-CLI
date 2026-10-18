use super::*;

#[test]
fn collect_scans_user_added_target_history_roots() {
    let state = temp_dir("manual-history-state");
    let home = temp_dir("manual-history-home");
    let manual_history = temp_dir("manual-history-root");
    fs::write(
        manual_history.join("manual-codex-history.jsonl"),
        r#"{"sessionId":"manual-session","role":"user","content":"Manual archive root topic"}"#,
    )
    .unwrap();
    let store = ClientStateStore::new(state.clone()).unwrap();
    store
        .write_collection(
            TARGETS_COLLECTION,
            json!({
                "items": [{
                    "target": "codex",
                    "manual": true,
                    "historyRoots": [display_path(&manual_history)]
                }]
            }),
        )
        .unwrap();

    let result = collect(&json!({
        "stateRoot": display_path(&state),
        "homeDir": display_path(&home),
        "agent": "codex",
        "topic": "manual archive root"
    }))
    .unwrap();

    assert_eq!(result["ok"], true);
    assert_eq!(result["status"], "materialized");
    assert_eq!(result["selectedCount"], 1);
    assert_eq!(result["written"][0]["selection"]["mode"], "deterministic");
    let collection_path = PathBuf::from(result["collectionPath"].as_str().unwrap());
    let collection = read_json_or_default(&collection_path, || json!({})).unwrap();
    assert!(
        collection["sources"]
            .as_array()
            .unwrap()
            .iter()
            .any(|source| {
                source["scope"] == "manual-target-history-root"
                    && source["historyRoot"] == display_path(&manual_history)
            })
    );
    assert_eq!(
        collection["conversations"][0]["sourcePath"],
        display_path(&manual_history.join("manual-codex-history.jsonl"))
    );
}

#[test]
fn archive_target_scan_accepts_desktop_preflight_json() {
    let scan = json!({
        "ok": true,
        "source": "desktop-preflight",
        "candidates": [{
            "target": "codex",
            "label": "Codex",
            "status": "detected",
            "historyRoots": ["test-data/codex-history"]
        }]
    });

    let result = archive_target_scan(&json!({
        "targetScanJson": scan.to_string()
    }))
    .unwrap();

    assert_eq!(result["source"], "desktop-preflight");
    assert_eq!(result["candidates"][0]["target"], "codex");
}
