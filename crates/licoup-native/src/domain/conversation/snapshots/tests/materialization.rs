use super::*;

#[test]
fn collect_materializes_matching_codex_jsonl_snapshot() {
    let state = temp_dir("collect-state");
    let home = temp_dir("collect-home");
    let codex = home.join(".codex");
    fs::create_dir_all(&codex).unwrap();
    fs::write(
        codex.join("history.jsonl"),
        [
            r#"{"sessionId":"session-1","role":"user","content":"Investigate Codex Spark billing"}"#,
            r#"{"sessionId":"session-1","role":"assistant","content":"Billing answer"}"#,
            r#"{"sessionId":"session-2","role":"user","content":"Unrelated topic"}"#,
        ]
        .join("\n"),
    )
    .unwrap();

    let result = collect(&json!({
        "stateRoot": display_path(&state),
        "homeDir": display_path(&home),
        "agent": "codex",
        "topic": "codex spark"
    }))
    .unwrap();

    assert_eq!(result["ok"], true);
    assert_eq!(result["status"], "materialized");
    assert_eq!(result["selectedCount"], 1);
    let written = result["written"].as_array().unwrap();
    let raw_path = PathBuf::from(written[0]["rawContentPath"].as_str().unwrap());
    let raw = fs::read_to_string(raw_path).unwrap();
    assert!(raw.contains("Investigate Codex Spark billing"));
    assert!(!raw.contains("Unrelated topic"));
    let collection_path = PathBuf::from(result["collectionPath"].as_str().unwrap());
    let collection = read_json_or_default(&collection_path, || json!({})).unwrap();
    assert_eq!(collection["conversations"].as_array().unwrap().len(), 1);
}

#[test]
fn archive_collect_materializes_snapshots_in_parallel() {
    let state = temp_dir("parallel-archive-state");
    let home = temp_dir("parallel-archive-home");
    let destination = temp_dir("parallel-archive-destination");
    let codex = home.join(".codex");
    fs::create_dir_all(&codex).unwrap();
    fs::write(
        codex.join("history.jsonl"),
        [
            r#"{"sessionId":"parallel-one","role":"user","content":"Pact parallel archive first"}"#,
            r#"{"sessionId":"parallel-two","role":"user","content":"Pact parallel archive second"}"#,
        ]
        .join("\n"),
    )
    .unwrap();

    let result = archive_collect(&json!({
        "stateRoot": display_path(&state),
        "homeDir": display_path(&home),
        "agent": "codex",
        "keywords": "Pact",
        "path": display_path(&destination),
        "archiveParallelism": 2
    }))
    .unwrap();

    assert_eq!(result["status"], "archived");
    assert_eq!(result["selectedCount"], 2);
    let index_path = PathBuf::from(result["documents"]["conversationIndex"].as_str().unwrap());
    let records = read_index_records(&index_path).unwrap();
    assert_eq!(records.len(), 2);
    assert_eq!(records[0]["archive_status"], "new");
    assert!(records.iter().any(|record| {
        record["native_session_id"] == "parallel-one"
            && PathBuf::from(record["snapshot_path"].as_str().unwrap()).exists()
    }));
    assert!(records.iter().any(|record| {
        record["native_session_id"] == "parallel-two"
            && PathBuf::from(record["snapshot_path"].as_str().unwrap()).exists()
    }));
}

#[test]
fn codex_rollout_raw_export_filters_by_payload_session_id() {
    let dir = temp_dir("codex-rollout-export");
    let path = dir.join("rollout.jsonl");
    fs::write(
        &path,
        [
            r#"{"timestamp":"2026-06-03T10:00:00Z","type":"session_meta","payload":{"id":"session-one","cwd":"test-data/one"}}"#,
            r#"{"timestamp":"2026-06-03T10:00:01Z","type":"response_item","payload":{"type":"message","role":"user","content":[{"type":"input_text","text":"first session text"}]}}"#,
            r#"{"timestamp":"2026-06-03T10:00:02Z","sessionId":"session-two","type":"session_meta","payload":{"id":"session-two","cwd":"test-data/two"}}"#,
            r#"{"timestamp":"2026-06-03T10:00:03Z","type":"response_item","payload":{"type":"message","role":"user","content":[{"type":"input_text","text":"second session text"}]}}"#,
        ]
        .join("\n"),
    )
    .unwrap();

    let raw = export_jsonl_source(&path, "session-two").unwrap();

    assert_eq!(
        raw.export_kind,
        "codex-rollout-jsonl-native-session-records"
    );
    assert!(raw.content.contains("second session text"));
    assert!(!raw.content.contains("session_meta"));
    assert!(!raw.content.contains("session-one"));
    assert!(!raw.content.contains("first session text"));
    assert!(raw.diagnostics.is_empty());
}

#[test]
fn preserved_index_records_skip_excluded_dependency_sources() {
    let previous = vec![
        json!({
            "archive_key": "dependency",
            "source_path": "test-data/project/node_modules/pkg/README.md",
            "archive_status": "unchanged"
        }),
        json!({
            "archive_key": "history",
            "source_path": "test-data/history/session.jsonl",
            "archive_status": "unchanged"
        }),
    ];
    let current = BTreeSet::<String>::new();
    let mut index_records = Vec::<Value>::new();

    append_preserved_index_records(&previous, &current, &mut index_records);

    assert_eq!(index_records.len(), 1);
    assert_eq!(index_records[0]["archive_key"], "history");
    assert_eq!(index_records[0]["archive_status"], "preserved");
}

#[test]
fn prune_removes_excluded_unindexed_snapshot_directories() {
    let collection_dir = temp_dir("prune-excluded-snapshots");
    let conversation_dir = collection_dir.join("conversations/hash");
    fs::create_dir_all(&conversation_dir).unwrap();
    atomic_write_json(
        &conversation_dir.join(SNAPSHOT_JSON),
        &json!({
            "snapshotId": "excluded",
            "sourcePath": "test-data/project/node_modules/pkg/README.md"
        }),
    )
    .unwrap();

    prune_excluded_unindexed_snapshots(&collection_dir, &[]).unwrap();

    assert!(!conversation_dir.exists());
}

#[test]
fn collect_exports_sqlite_rows_without_key_identity() {
    let state = temp_dir("sqlite-snapshot-state");
    let home = temp_dir("sqlite-snapshot-home");
    let opencode = home.join(".config/opencode");
    fs::create_dir_all(&opencode).unwrap();
    let db_path = opencode.join("history.db");
    {
        let connection = Connection::open(&db_path).unwrap();
        connection
            .execute("CREATE TABLE conversation_history (body TEXT)", [])
            .unwrap();
        connection
            .execute(
                "INSERT INTO conversation_history (body) VALUES (?1)",
                ["message: SQLite archive topic without stable row key"],
            )
            .unwrap();
    }

    let result = collect(&json!({
        "stateRoot": display_path(&state),
        "homeDir": display_path(&home),
        "agent": "opencode",
        "topic": "sqlite archive topic"
    }))
    .unwrap();

    assert_eq!(result["status"], "materialized");
    let raw_path = PathBuf::from(result["written"][0]["rawContentPath"].as_str().unwrap());
    let raw = read_json_or_default(&raw_path, || json!({})).unwrap();
    assert_eq!(raw["rows"].as_array().unwrap().len(), 1);
    assert_eq!(raw["rows"][0]["table"], "conversation_history");
    let snapshot_path = PathBuf::from(result["written"][0]["snapshotPath"].as_str().unwrap());
    let snapshot = read_json_or_default(&snapshot_path, || json!({})).unwrap();
    assert_eq!(snapshot["rawExportKind"], "sqlite-native-session-records");
}

#[test]
fn collect_refresh_preserves_previous_unseen_snapshots() {
    let state = temp_dir("preserve-state");
    let home = temp_dir("preserve-home");
    let codex = home.join(".codex");
    fs::create_dir_all(&codex).unwrap();
    fs::write(
        codex.join("history.jsonl"),
        r#"{"sessionId":"session-1","role":"user","content":"Archive topic first"}"#,
    )
    .unwrap();
    let first = collect(&json!({
        "stateRoot": display_path(&state),
        "homeDir": display_path(&home),
        "agent": "codex",
        "topic": "archive topic"
    }))
    .unwrap();
    fs::write(
        codex.join("history.jsonl"),
        r#"{"sessionId":"session-2","role":"user","content":"Archive topic second"}"#,
    )
    .unwrap();
    let second = collect(&json!({
        "stateRoot": display_path(&state),
        "homeDir": display_path(&home),
        "agent": "codex",
        "topic": "archive topic"
    }))
    .unwrap();
    assert_eq!(first["selectedCount"], 1);
    assert_eq!(second["selectedCount"], 1);
    let collection_path = PathBuf::from(second["collectionPath"].as_str().unwrap());
    let collection = read_json_or_default(&collection_path, || json!({})).unwrap();
    assert_eq!(collection["conversations"].as_array().unwrap().len(), 2);
}

#[test]
fn large_rollout_streams_completely_in_browse_and_archive_modes() {
    let state = temp_dir("archive-large-state");
    let home = temp_dir("archive-large-home");
    let archive_root = temp_dir("archive-large-root");
    let codex = home.join(".codex");
    let sessions_dir = codex.join("sessions");
    fs::create_dir_all(&sessions_dir).unwrap();
    let large_text = "x".repeat((32 * 1024 * 1024) + 2048);
    fs::write(
        sessions_dir.join("rollout-2026-08-01T00-00-00-019f0000-0000-7000-8000-00000000e001.jsonl"),
        format!(
            "{{\"timestamp\":\"2026-08-01T00:00:00Z\",\"type\":\"session_meta\",\"payload\":{{\"id\":\"large-1\",\"cwd\":\"/workspace/large\"}}}}\n{{\"timestamp\":\"2026-08-01T00:00:01Z\",\"type\":\"response_item\",\"payload\":{{\"type\":\"message\",\"role\":\"user\",\"content\":[{{\"type\":\"input_text\",\"text\":\"LicoMesh {}\"}}]}}}}\n",
            large_text
        ),
    )
    .unwrap();

    // The retired total file-size rejection must stay retired: a rollout above
    // the former 32 MiB cutoff streams completely in browse mode, keyed by the
    // record identity in its session_meta header rather than the file stem.
    let browse = conversations::conversation_list(
        &json!({"agent": "codex", "homeDir": display_path(&home)}),
    )
    .unwrap();
    let sessions = browse["sessions"].as_array().unwrap();
    assert_eq!(sessions.len(), 1);
    let session = &sessions[0];
    assert_eq!(session["nativeSessionId"], "large-1");
    assert_eq!(session["workingDirectory"], "/workspace/large");
    assert_eq!(session["sourceMessageCount"], 1);
    assert_eq!(session["messagePage"]["total"], 1);
    assert_eq!(session["messagePage"]["hasEarlier"], false);
    let messages = session["messages"].as_array().unwrap();
    assert_eq!(messages.len(), 1);
    assert_eq!(messages[0]["role"], "user");
    assert!(
        messages[0]["text"]
            .as_str()
            .unwrap()
            .starts_with("LicoMesh ")
    );
    assert_eq!(
        messages[0]["text"].as_str().unwrap().len(),
        "LicoMesh ".len() + (32 * 1024 * 1024) + 2048
    );
    assert!(
        browse["sources"]["skipped"]
            .as_array()
            .unwrap()
            .iter()
            .all(|skip| skip["reason"].as_str() != Some("file_too_large"))
    );

    profile_import(&json!({
        "stateRoot": display_path(&state),
        "profileId": "licomesh-large",
        "displayName": "LicoMesh Large",
        "archiveRoot": display_path(&archive_root),
        "canonicalNames": "LicoMesh",
        "expectedAgents": "codex"
    }))
    .unwrap();
    let archived = archive_run(&json!({
        "stateRoot": display_path(&state),
        "homeDir": display_path(&home),
        "profile": "licomesh-large"
    }))
    .unwrap();
    assert_eq!(archived["selectedCount"], 1);
    assert_eq!(archived["validation"]["healthStatus"], "ok");
}
