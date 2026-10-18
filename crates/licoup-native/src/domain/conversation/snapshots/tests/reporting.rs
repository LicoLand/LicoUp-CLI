use super::*;

#[test]
fn archive_run_materializes_profile_index_summary_and_report() {
    let state = temp_dir("archive-run-state");
    let home = temp_dir("archive-run-home");
    let archive_root = temp_dir("archive-run-root");
    let codex = home.join(".codex");
    fs::create_dir_all(&codex).unwrap();
    fs::write(
        codex.join("history.jsonl"),
        r#"{"sessionId":"licomesh-1","role":"user","content":"Work on LicoMesh at /repo/licomesh"}"#,
    )
    .unwrap();
    profile_import(&json!({
        "stateRoot": display_path(&state),
        "profileId": "licomesh",
        "displayName": "LicoMesh",
        "archiveRoot": display_path(&archive_root),
        "canonicalNames": "LicoMesh",
        "projectPaths": "/repo/licomesh",
        "expectedAgents": "codex"
    }))
    .unwrap();

    let result = archive_run(&json!({
        "stateRoot": display_path(&state),
        "homeDir": display_path(&home),
        "profile": "licomesh",
        "trigger": "agent"
    }))
    .unwrap();

    assert_eq!(result["status"], "materialized");
    assert_eq!(result["mode"], "conversation-archive");
    assert_eq!(result["selectedCount"], 1);
    assert_eq!(result["validation"]["healthStatus"], "ok");
    let collection_path = PathBuf::from(result["collectionPath"].as_str().unwrap());
    let collection = read_json_or_default(&collection_path, || json!({})).unwrap();
    assert_eq!(collection["kind"], "native-conversation-archive");
    let index_path = archive_root
        .join("collections")
        .join("licomesh")
        .join(CONVERSATION_INDEX_JSONL);
    let index = read_index_records(&index_path).unwrap();
    assert_eq!(index.len(), 1);
    assert_eq!(index[0]["confidence"], "high");
    assert_eq!(index[0]["archive_status"], "new");
    let semantic_json_path = PathBuf::from(
        index[0]["semantic_document_path"]
            .as_str()
            .expect("semantic JSON path"),
    );
    let semantic_markdown_path = PathBuf::from(
        index[0]["semantic_markdown_path"]
            .as_str()
            .expect("semantic Markdown path"),
    );
    assert!(semantic_json_path.exists());
    assert!(semantic_markdown_path.exists());
    assert!(
        !index[0]["semantic_content_hash"]
            .as_str()
            .unwrap_or_default()
            .is_empty()
    );
    let semantic: Value =
        serde_json::from_str(&fs::read_to_string(&semantic_json_path).unwrap()).unwrap();
    crate::domain::conversation_semantic::validate_semantic_conversation(&semantic).unwrap();
    for layer in ["thread", "execution", "artifacts", "audit", "raw"] {
        assert!(
            semantic.get(layer).is_some(),
            "missing semantic {layer} layer"
        );
    }
    assert!(
        archive_root
            .join("collections/licomesh/summary.md")
            .exists()
    );
    let index_markdown_path = archive_root
        .join("collections")
        .join("licomesh")
        .join(CONVERSATION_INDEX_MD);
    assert!(index_markdown_path.exists());
    assert!(
        fs::read_to_string(index_markdown_path)
            .unwrap()
            .contains(SEMANTIC_MD)
    );
    assert!(
        archive_root
            .join("collections/licomesh/sources.json")
            .exists()
    );
    assert!(
        archive_root
            .join("collections/licomesh/matches.jsonl")
            .exists()
    );
    assert!(
        archive_root
            .join("collections/licomesh/validation.json")
            .exists()
    );

    let report = archive_report(&json!({
        "stateRoot": display_path(&state),
        "profile": "licomesh"
    }))
    .unwrap();
    assert_eq!(report["indexCount"], 1);
    assert_eq!(report["validation"]["healthStatus"], "ok");
}
