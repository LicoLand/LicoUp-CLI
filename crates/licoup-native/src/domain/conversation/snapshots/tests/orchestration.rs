use super::*;

#[test]
fn collect_creates_empty_collection_when_no_native_history_matches() {
    let state = temp_dir("empty-state");
    let home = temp_dir("empty-home");

    let result = collect(&json!({
        "stateRoot": display_path(&state),
        "homeDir": display_path(&home),
        "agent": "codex",
        "topic": "missing topic"
    }))
    .unwrap();

    assert_eq!(result["ok"], true);
    assert_eq!(result["status"], "empty");
    assert_eq!(result["selectedCount"], 0);
    let collection_path = PathBuf::from(result["collectionPath"].as_str().unwrap());
    let collection = read_json_or_default(&collection_path, || json!({})).unwrap();
    assert_eq!(collection["state"], "empty");
    assert_eq!(collection["topicKey"], "missing-topic");
}

#[test]
fn archive_collect_derives_profile_scans_targets_and_writes_destination() {
    let state = temp_dir("keyword-archive-state");
    let home = temp_dir("keyword-archive-home");
    let destination = temp_dir("keyword-archive-destination");
    let manual_history = temp_dir("keyword-archive-history");
    fs::write(
        manual_history.join("manual-codex-history.jsonl"),
        [
            r#"{"sessionId":"pactium-session","role":"user","content":"Pactium archive keyword"}"#,
            r#"{"sessionId":"pact-session","role":"user","content":"Pact archive keyword"}"#,
            r#"{"sessionId":"agent-studio-session","role":"user","content":"agentstudio archive keyword"}"#,
        ]
        .join("\n"),
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

    let result = archive_collect(&json!({
        "stateRoot": display_path(&state),
        "homeDir": display_path(&home),
        "agent": "codex",
        "keywords": "Pactium,Pact,Agent Studio",
        "path": display_path(&destination),
        "archiveParallelism": 1
    }))
    .unwrap();

    assert_eq!(result["ok"], true);
    assert_eq!(result["status"], "archived");
    assert_eq!(result["entry"], "keyword-archive");
    assert_eq!(result["keywordCount"], 3);
    assert_eq!(result["selectedCount"], 3);
    assert_eq!(result["documentCount"], 3);
    assert!(result.get("validation").is_none());
    assert_eq!(result["targetScan"]["includedAgents"][0], "codex");
    let collection_path = PathBuf::from(result["collectionPath"].as_str().unwrap());
    assert!(collection_path.exists());
    let archives = result["archives"].as_array().unwrap();
    assert_eq!(archives[0]["keyword"], "Pactium");
    assert_eq!(archives[0]["folderName"], "pactium");
    assert_eq!(archives[1]["keyword"], "Pact");
    assert_eq!(archives[1]["folderName"], "pact");
    assert_eq!(archives[2]["keyword"], "Agent Studio");
    assert_eq!(archives[2]["folderName"], "agent-studio");
    assert!(destination.join("pactium").join(COLLECTION_JSON).exists());
    assert!(destination.join("pact").join(COLLECTION_JSON).exists());
    assert!(
        destination
            .join("agent-studio")
            .join(COLLECTION_JSON)
            .exists()
    );
    assert!(
        !destination
            .join("collections")
            .join("pactium")
            .join("pact")
            .exists()
    );
    assert!(PathBuf::from(archives[2]["documents"]["summary"].as_str().unwrap()).exists());
}
