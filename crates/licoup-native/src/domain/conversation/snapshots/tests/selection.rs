use super::*;

#[test]
fn topic_key_normalizes_case_space_width_and_separators() {
    assert_eq!(
        topic_key(" Codex＿Spark weekly limit ").unwrap(),
        "codex-spark-weekly-limit"
    );
    assert_eq!(topic_key("Ｃｏｄｅｘ　Spark").unwrap(), "codex-spark");
}

#[test]
fn archive_keywords_dedupes_after_normalization() {
    let keywords = archive_keywords(&json!({
        "keywords": "OSysIt,osysit, OSYSIT "
    }))
    .unwrap();

    assert_eq!(keywords, vec!["OSysIt"]);
    let profile =
        derived_archive_profile(&keywords, Path::new("test-data/archive"), &["codex".into()])
            .unwrap();
    assert_eq!(profile.profile_id, "osysit");
    assert_eq!(profile.collection_path_segments, vec!["osysit"]);
    assert_eq!(profile.canonical_names, vec!["OSysIt"]);
}

#[test]
fn archive_keywords_create_one_profile_per_keyword() {
    let keywords = archive_keywords(&json!({
        "keywords": "LicoMesh, Agent Studio, osysit"
    }))
    .unwrap();
    let profiles = derived_keyword_archive_profiles(
        &keywords,
        Path::new("test-data/archive"),
        &["codex".into()],
    )
    .unwrap();

    assert_eq!(keywords, vec!["LicoMesh", "Agent Studio", "osysit"]);
    assert_eq!(profiles.len(), 3);
    assert_eq!(profiles[0].collection_path_segments, vec!["licomesh"]);
    assert_eq!(profiles[0].canonical_names, vec!["LicoMesh"]);
    assert_eq!(profiles[1].collection_path_segments, vec!["agent-studio"]);
    assert_eq!(profiles[1].canonical_names, vec!["Agent Studio"]);
    assert!(profiles[1].alias_names.contains(&"agentstudio".to_string()));
    assert_eq!(profiles[2].collection_path_segments, vec!["osysit"]);
    assert_eq!(profiles[2].canonical_names, vec!["osysit"]);
}

#[test]
fn archive_profile_completes_phrase_keyword_aliases() {
    let keywords = archive_keywords(&json!({
        "keywords": "Design Studio"
    }))
    .unwrap();
    let profile =
        derived_archive_profile(&keywords, Path::new("test-data/archive"), &["codex".into()])
            .unwrap();

    assert_eq!(profile.profile_id, "design-studio");
    assert_eq!(profile.collection_path_segments, vec!["design-studio"]);
    assert_eq!(profile.canonical_names, vec!["Design Studio"]);
    assert_eq!(profile.alias_names, vec!["designstudio"]);

    let compact_candidate = json!({
        "title": "designstudio migration thread",
        "messages": []
    });
    let compact_match = profile_match(&compact_candidate, &profile).unwrap();
    assert_eq!(compact_match.matched_terms, vec!["designstudio"]);
    assert_eq!(compact_match.confidence, "medium");

    let duplicate_form_candidate = json!({
        "title": "Design Studio designstudio migration thread",
        "messages": []
    });
    let duplicate_form_match = profile_match(&duplicate_form_candidate, &profile).unwrap();
    assert_eq!(duplicate_form_match.confidence, "medium");

    let camel_keywords = archive_keywords(&json!({
        "keywords": "DesignStudio"
    }))
    .unwrap();
    let camel_profile = derived_archive_profile(
        &camel_keywords,
        Path::new("test-data/archive"),
        &["codex".into()],
    )
    .unwrap();
    assert_eq!(camel_profile.profile_id, "designstudio");
    assert_eq!(camel_profile.alias_names, vec!["Design Studio"]);
    let spaced_candidate = json!({
        "title": "Design Studio migration thread",
        "messages": []
    });
    assert!(profile_match(&spaced_candidate, &camel_profile).is_some());
}

#[test]
fn pactium_keyword_uses_strict_current_project_archive() {
    let state = temp_dir("pactium-strict-state");
    let home = temp_dir("pactium-strict-home");
    let destination = temp_dir("pactium-strict-destination");
    let manual_history = temp_dir("pactium-strict-history");
    fs::write(
        manual_history.join("manual-codex-history.jsonl"),
        [
            r#"{"sessionId":"pactium-session","role":"user","content":"Pactium archive keyword"}"#,
            r#"{"sessionId":"pact-session","role":"user","content":"Pact archive keyword"}"#,
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
        "keywords": "Pactium",
        "path": display_path(&destination)
    }))
    .unwrap();

    assert_eq!(result["status"], "archived");
    assert_eq!(result["selectedCount"], 1);
    assert_eq!(
        PathBuf::from(result["collectionPath"].as_str().unwrap()),
        destination.join("pactium").join(COLLECTION_JSON)
    );
    let records =
        read_index_records(&destination.join("pactium").join(CONVERSATION_INDEX_JSONL)).unwrap();
    assert_eq!(records.len(), 1);
    assert_eq!(records[0]["native_session_id"], "pactium-session");
}
