use super::*;

#[test]
fn profile_matching_ignores_metadata_identity_terms() {
    let keywords = archive_keywords(&json!({
        "keywords": "Agent Studio"
    }))
    .unwrap();
    let profile =
        derived_archive_profile(&keywords, Path::new("test-data/archive"), &["codex".into()])
            .unwrap();
    let candidate = json!({
        "title": "Unrelated Pact work",
        "nativeSessionId": "pact-session",
        "messages": [
            {"role": "metadata", "text": "base instructions mention Agent Studio"},
            {"role": "user", "text": "Continue unrelated Pact work"}
        ]
    });

    assert!(candidate_has_real_conversation(&candidate));
    assert!(profile_match(&candidate, &profile).is_none());
}

#[test]
fn semantic_projection_uses_archive_relative_evidence_refs() {
    let source_path = ["/", "Users", "sample-user", "conversation.jsonl"].join("/");
    let session = json!({
        "adapterId": "codex",
        "nativeSessionId": "session-1",
        "sourceKind": "jsonl",
        "sourcePath": source_path.clone(),
        "semantic": {
            "artifacts": [],
            "raw": {},
            "audit": {}
        }
    });
    let raw = RawExport {
        file_name: "source.jsonl".to_string(),
        content: r#"{"role":"user","text":"hello"}
"#
        .to_string(),
        export_kind: "test".to_string(),
        diagnostics: Vec::new(),
    };
    let metadata = snapshot_source_metadata(&session);
    let projected = project_archive_semantic_document(
        &session,
        &raw,
        "snapshot-hash",
        "2026-01-01T00:00:00Z",
        &metadata,
    );

    assert_eq!(
        projected["raw"]["evidenceRefs"][0]["pathRef"],
        "source.jsonl"
    );
    assert_eq!(
        projected["audit"]["sourceEvidence"]["pathRef"],
        "source.jsonl"
    );
    assert_eq!(projected["audit"]["validationStatus"], "ok");
    assert!(!projected.to_string().contains(&source_path));
}
