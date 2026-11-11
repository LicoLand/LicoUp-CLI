use super::*;

#[test]
fn archive_validation_detects_semantic_missing_stale_duplicate_and_metadata_only_records() {
    let state = temp_dir("archive-semantic-validation-state");
    let home = temp_dir("archive-semantic-validation-home");
    let archive_root = temp_dir("archive-semantic-validation-root");
    let codex = home.join(".codex");
    fs::create_dir_all(&codex).unwrap();
    fs::write(
        codex.join("history.jsonl"),
        r#"{"sessionId":"semantic-validation-1","role":"user","content":"LicoMesh semantic validation"}"#,
    )
    .unwrap();
    profile_import(&json!({
        "stateRoot": display_path(&state),
        "profileId": "licomesh-semantic-validation",
        "displayName": "LicoMesh Semantic Validation",
        "archiveRoot": display_path(&archive_root),
        "canonicalNames": "LicoMesh",
        "expectedAgents": "codex"
    }))
    .unwrap();
    archive_run(&json!({
        "stateRoot": display_path(&state),
        "homeDir": display_path(&home),
        "profile": "licomesh-semantic-validation"
    }))
    .unwrap();

    let collection_dir = archive_root
        .join("collections")
        .join("licomesh-semantic-validation");
    let index_path = collection_dir.join(CONVERSATION_INDEX_JSONL);
    let mut records = read_index_records(&index_path).unwrap();
    assert_eq!(records.len(), 1);
    let original = records[0].clone();
    let profile = parse_archive_profile(&json!({
        "profileId": "licomesh-semantic-validation",
        "displayName": "LicoMesh Semantic Validation",
        "archiveRoot": display_path(&archive_root),
        "canonicalNames": "LicoMesh",
        "expectedAgents": "codex"
    }))
    .unwrap();
    let semantic_path = PathBuf::from(
        original["semantic_document_path"]
            .as_str()
            .expect("semantic document path"),
    );
    let semantic_markdown_path = PathBuf::from(
        original["semantic_markdown_path"]
            .as_str()
            .expect("semantic markdown path"),
    );
    let semantic_original = fs::read_to_string(&semantic_path).unwrap();
    let semantic_markdown_original = fs::read_to_string(&semantic_markdown_path).unwrap();

    fs::remove_file(&semantic_markdown_path).unwrap();
    let missing = validate_archive_collection(&collection_dir, &records, &profile).unwrap();
    assert!(missing["issues"].as_array().unwrap().iter().any(|issue| {
        issue["type"] == "missing_semantic_document" && issue["field"] == "semantic_markdown_path"
    }));
    fs::write(&semantic_markdown_path, semantic_markdown_original).unwrap();

    fs::write(&semantic_path, "{}\n").unwrap();
    let stale = validate_archive_collection(&collection_dir, &records, &profile).unwrap();
    assert!(
        stale["issues"]
            .as_array()
            .unwrap()
            .iter()
            .any(|issue| issue["type"] == "stale_semantic_hash")
    );
    fs::write(&semantic_path, &semantic_original).unwrap();

    let duplicate = validate_archive_collection(
        &collection_dir,
        &[original.clone(), original.clone()],
        &profile,
    )
    .unwrap();
    assert!(
        duplicate["issues"]
            .as_array()
            .unwrap()
            .iter()
            .any(|issue| issue["type"] == "duplicate_archive_key")
    );

    let mut metadata_semantic: Value = serde_json::from_str(&semantic_original).unwrap();
    metadata_semantic["thread"] = json!([]);
    metadata_semantic["execution"] = json!([]);
    let metadata_json = serde_json::to_string_pretty(&metadata_semantic).unwrap();
    fs::write(&semantic_path, format!("{metadata_json}\n")).unwrap();
    records[0]["semantic_content_hash"] = json!(hash_text(&metadata_json));
    records[0]["match_reason"] = json!("metadata-only candidate");
    let metadata_only = validate_archive_collection(&collection_dir, &records, &profile).unwrap();
    assert!(
        metadata_only["issues"]
            .as_array()
            .unwrap()
            .iter()
            .any(|issue| {
                issue["type"] == "metadata_only_false_positive" && issue["severity"] == "warning"
            })
    );

    fs::write(&semantic_path, semantic_original).unwrap();
    let raw_path = PathBuf::from(
        original["raw_content_path"]
            .as_str()
            .expect("raw content path"),
    );
    fs::write(raw_path, b"tampered\n").unwrap();
    let raw_stale = validate_archive_collection(&collection_dir, &[original], &profile).unwrap();
    assert!(
        raw_stale["issues"]
            .as_array()
            .unwrap()
            .iter()
            .any(|issue| { issue["type"] == "raw_content_fingerprint_mismatch" })
    );
}

#[test]
fn archive_run_marks_incremental_statuses_and_verify_missing_files() {
    let state = temp_dir("archive-incremental-state");
    let home = temp_dir("archive-incremental-home");
    let archive_root = temp_dir("archive-incremental-root");
    let codex = home.join(".codex");
    fs::create_dir_all(&codex).unwrap();
    let history = codex.join("history.jsonl");
    fs::write(
        &history,
        r#"{"sessionId":"inc-1","role":"user","content":"LicoMesh first"}"#,
    )
    .unwrap();
    profile_import(&json!({
        "stateRoot": display_path(&state),
        "profileId": "licomesh-inc",
        "displayName": "LicoMesh Incremental",
        "archiveRoot": display_path(&archive_root),
        "canonicalNames": "LicoMesh",
        "expectedAgents": "codex"
    }))
    .unwrap();

    archive_run(&json!({
        "stateRoot": display_path(&state),
        "homeDir": display_path(&home),
        "profile": "licomesh-inc"
    }))
    .unwrap();
    archive_run(&json!({
        "stateRoot": display_path(&state),
        "homeDir": display_path(&home),
        "profile": "licomesh-inc"
    }))
    .unwrap();
    let index_path = archive_root
        .join("collections")
        .join("licomesh-inc")
        .join(CONVERSATION_INDEX_JSONL);
    let index = read_index_records(&index_path).unwrap();
    assert_eq!(index[0]["archive_status"], "unchanged");

    fs::write(
        &history,
        r#"{"sessionId":"inc-1","role":"user","content":"LicoMesh changed"}"#,
    )
    .unwrap();
    archive_run(&json!({
        "stateRoot": display_path(&state),
        "homeDir": display_path(&home),
        "profile": "licomesh-inc"
    }))
    .unwrap();
    let index = read_index_records(&index_path).unwrap();
    assert_eq!(index[0]["archive_status"], "updated");
    let raw_path = PathBuf::from(index[0]["raw_content_path"].as_str().unwrap());
    fs::remove_file(raw_path).unwrap();
    let verify = archive_verify(&json!({
        "stateRoot": display_path(&state),
        "profile": "licomesh-inc"
    }))
    .unwrap();
    assert_eq!(verify["validation"]["healthStatus"], "failed");
    assert_eq!(verify["validation"]["errorCount"], 1);
}

#[test]
fn archive_verify_recomputes_raw_content_fingerprint() {
    let state = temp_dir("archive-fingerprint-state");
    let home = temp_dir("archive-fingerprint-home");
    let archive_root = temp_dir("archive-fingerprint-root");
    let codex = home.join(".codex");
    fs::create_dir_all(&codex).unwrap();
    fs::write(
        codex.join("history.jsonl"),
        r#"{"sessionId":"fingerprint-1","role":"user","content":"LicoMesh fingerprint"}"#,
    )
    .unwrap();
    profile_import(&json!({
        "stateRoot": display_path(&state),
        "profileId": "licomesh-fingerprint",
        "displayName": "LicoMesh Fingerprint",
        "archiveRoot": display_path(&archive_root),
        "canonicalNames": "LicoMesh",
        "expectedAgents": "codex"
    }))
    .unwrap();

    archive_run(&json!({
        "stateRoot": display_path(&state),
        "homeDir": display_path(&home),
        "profile": "licomesh-fingerprint"
    }))
    .unwrap();
    let index_path = archive_root
        .join("collections")
        .join("licomesh-fingerprint")
        .join(CONVERSATION_INDEX_JSONL);
    let index = read_index_records(&index_path).unwrap();
    let raw_path = PathBuf::from(index[0]["raw_content_path"].as_str().unwrap());
    fs::write(&raw_path, b"{\"tampered\":true}\n").unwrap();

    let verify = archive_verify(&json!({
        "stateRoot": display_path(&state),
        "profile": "licomesh-fingerprint"
    }))
    .unwrap();
    assert_eq!(verify["validation"]["healthStatus"], "failed");
    assert!(
        verify["validation"]["issues"]
            .as_array()
            .unwrap()
            .iter()
            .any(|issue| issue["type"] == "raw_content_fingerprint_mismatch")
    );
}

#[test]
fn archive_verify_collection_path_recomputes_hashes_for_keyword_archives() {
    let state = temp_dir("keyword-verify-state");
    let home = temp_dir("keyword-verify-home");
    let destination = temp_dir("keyword-verify-destination");
    let codex = home.join(".codex");
    fs::create_dir_all(&codex).unwrap();
    fs::write(
        codex.join("history.jsonl"),
        r#"{"sessionId":"keyword-verify-1","role":"user","content":"LicoMesh keyword verify"}"#,
    )
    .unwrap();

    let result = archive_collect(&json!({
        "stateRoot": display_path(&state),
        "homeDir": display_path(&home),
        "agent": "codex",
        "keywords": "LicoMesh",
        "path": display_path(&destination)
    }))
    .unwrap();
    let collection_path = PathBuf::from(result["collectionPath"].as_str().unwrap());
    let index = read_index_records(
        &collection_path
            .parent()
            .unwrap()
            .join(CONVERSATION_INDEX_JSONL),
    )
    .unwrap();
    let raw_path = PathBuf::from(index[0]["raw_content_path"].as_str().unwrap());
    fs::write(&raw_path, b"{\"copiedButCorrupt\":true}\n").unwrap();

    let verify = archive_verify(&json!({
        "collectionPath": display_path(&collection_path)
    }))
    .unwrap();
    assert_eq!(verify["validation"]["healthStatus"], "failed");
    assert!(
        verify["validation"]["issues"]
            .as_array()
            .unwrap()
            .iter()
            .any(|issue| issue["type"] == "raw_content_fingerprint_mismatch")
    );
}

#[test]
fn archive_validation_reports_baseline_coverage() {
    let state = temp_dir("archive-baseline-state");
    let home = temp_dir("archive-baseline-home");
    let archive_root = temp_dir("archive-baseline-root");
    let baseline = temp_dir("archive-baseline-index").join("conversation-index.jsonl");
    write_jsonl(
        &baseline,
        &[
            json!({"archive_key": "a", "raw_content_bytes": 10}),
            json!({"archive_key": "b", "raw_content_bytes": 10}),
        ],
    )
    .unwrap();
    let codex = home.join(".codex");
    fs::create_dir_all(&codex).unwrap();
    fs::write(
        codex.join("history.jsonl"),
        r#"{"sessionId":"baseline-1","role":"user","content":"LicoMesh baseline"}"#,
    )
    .unwrap();
    profile_import(&json!({
        "stateRoot": display_path(&state),
        "profileId": "licomesh-baseline",
        "displayName": "LicoMesh Baseline",
        "archiveRoot": display_path(&archive_root),
        "canonicalNames": "LicoMesh",
        "expectedAgents": "codex",
        "baselineIndexPath": display_path(&baseline)
    }))
    .unwrap();

    let result = archive_run(&json!({
        "stateRoot": display_path(&state),
        "homeDir": display_path(&home),
        "profile": "licomesh-baseline"
    }))
    .unwrap();

    assert_eq!(result["validation"]["baseline"]["status"], "compared");
    assert_eq!(result["validation"]["baseline"]["baselineCount"], 2);
    assert_eq!(result["validation"]["baseline"]["currentCount"], 1);
}
