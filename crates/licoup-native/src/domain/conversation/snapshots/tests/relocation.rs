use super::*;

#[test]
fn relocating_copied_data_home_rebases_only_owned_snapshot_references() {
    exercise_relocation(false);
}

#[test]
fn staged_relocation_verifies_physical_content_but_records_only_final_logical_paths() {
    exercise_relocation(true);
}

fn exercise_relocation(staged: bool) {
    let fixture = temp_dir(if staged {
        "data-home-preparation"
    } else {
        "data-home-relocation"
    });
    let previous_root = fixture.join("previous");
    let new_root = fixture.join("new");
    let client_state = previous_root.join("client-state");
    let snapshot_root = previous_root.join("snapshots");
    let internal_archive_root = previous_root.join("archives/internal");
    let external_archive_root = previous_root.join("archives/external");
    let internal_baseline = previous_root.join("baseline/internal.jsonl");
    let outside = temp_dir(if staged {
        "data-home-preparation-outside"
    } else {
        "data-home-relocation-outside"
    });
    let home = outside.join("agent-home");
    let external_baseline = outside.join("baseline.jsonl");
    let history = home.join(".codex/history.jsonl");

    fs::create_dir_all(&previous_root).unwrap();
    fs::create_dir_all(internal_baseline.parent().unwrap()).unwrap();
    fs::create_dir_all(&home.join(".codex")).unwrap();
    fs::write(&internal_baseline, "{\"raw_content_bytes\":100}\n").unwrap();
    fs::write(&external_baseline, "{\"raw_content_bytes\":100}\n").unwrap();
    fs::write(
        &history,
        r#"{"sessionId":"relocation-session","role":"user","content":"LicoMesh relocation fixture"}"#,
    )
    .unwrap();

    root_set(&json!({
        "stateRoot": display_path(&client_state),
        "path": display_path(&snapshot_root)
    }))
    .unwrap();
    for (profile_id, archive_root, baseline_path) in [
        (
            "internal-baseline",
            &internal_archive_root,
            &internal_baseline,
        ),
        (
            "external-baseline",
            &external_archive_root,
            &external_baseline,
        ),
    ] {
        profile_import(&json!({
            "stateRoot": display_path(&client_state),
            "profileJson": serde_json::to_string(&json!({
                "profileId": profile_id,
                "displayName": "LicoMesh",
                "archiveRoot": display_path(archive_root),
                "baselineIndexPath": display_path(baseline_path),
                "canonicalNames": ["LicoMesh"],
                "projectPaths": [display_path(&outside.join("project"))],
                "expectedAgents": ["codex"]
            }))
            .unwrap()
        }))
        .unwrap();
        let result = archive_run(&json!({
            "stateRoot": display_path(&client_state),
            "homeDir": display_path(&home),
            "profile": profile_id
        }))
        .unwrap();
        assert_eq!(result["selectedCount"], 1);
        assert_eq!(result["validation"]["healthStatus"], "ok");
    }
    let future_archive_root = previous_root.join("archives/future");
    let future_baseline_path = previous_root.join("baseline/future.jsonl");
    profile_import(&json!({
        "stateRoot": display_path(&client_state),
        "profileJson": serde_json::to_string(&json!({
            "profileId": "future-root",
            "displayName": "LicoMesh future archive",
            "archiveRoot": display_path(&future_archive_root),
            "baselineIndexPath": display_path(&future_baseline_path),
            "canonicalNames": ["LicoMesh"],
            "projectPaths": [display_path(&outside.join("project"))],
            "expectedAgents": ["codex"]
        }))
        .unwrap()
    }))
    .unwrap();
    assert!(!future_archive_root.exists());
    assert!(!future_baseline_path.exists());

    let internal_collection_dir = internal_archive_root.join("collections/internal-baseline");
    let external_collection_dir = external_archive_root.join("collections/external-baseline");
    let internal_before = snapshot_fixture(&internal_collection_dir);
    let external_before = snapshot_fixture(&external_collection_dir);
    let external_validation_before =
        read_json_or_default(&external_collection_dir.join(VALIDATION_JSON), || json!({})).unwrap();
    assert_eq!(
        external_validation_before["baseline"]["baselineIndexPath"],
        display_path(&external_baseline)
    );

    let physical_root = if staged {
        fixture.join("private-staging")
    } else {
        new_root.clone()
    };
    copy_dir_all(&previous_root, &physical_root).unwrap();
    // The helper must carry forward the previously computed external baseline
    // result without reading the external file during relocation.
    fs::remove_file(&external_baseline).unwrap();
    relocate_copied_data_home_references(&physical_root, &previous_root, &new_root).unwrap();
    if staged {
        assert!(!new_root.exists(), "owner preparation did not publish");
        fs::rename(&physical_root, &new_root).unwrap();
    }
    fs::remove_dir_all(&previous_root).unwrap();

    let moved_state = new_root.join("client-state");
    let moved_settings = ClientStateStore::new(moved_state.clone())
        .unwrap()
        .read_collection(SETTINGS_COLLECTION)
        .unwrap();
    assert_eq!(
        moved_settings["conversationSnapshotRoot"],
        display_path(&new_root.join("snapshots"))
    );
    let moved_profiles = ClientStateStore::new(moved_state.clone())
        .unwrap()
        .read_collection(PROFILES_COLLECTION)
        .unwrap();
    let internal_profile = moved_profiles["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|profile| profile["profileId"] == "internal-baseline")
        .unwrap();
    let external_profile = moved_profiles["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|profile| profile["profileId"] == "external-baseline")
        .unwrap();
    assert_eq!(
        internal_profile["archiveRoot"],
        display_path(&new_root.join("archives/internal"))
    );
    assert_eq!(
        internal_profile["baselineIndexPath"],
        display_path(&new_root.join("baseline/internal.jsonl"))
    );
    assert_eq!(
        external_profile["baselineIndexPath"],
        display_path(&external_baseline)
    );
    assert_eq!(
        external_profile["projectPaths"][0],
        display_path(&outside.join("project"))
    );
    let future_profile = moved_profiles["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|profile| profile["profileId"] == "future-root")
        .unwrap();
    assert_eq!(
        future_profile["archiveRoot"],
        display_path(&new_root.join("archives/future"))
    );
    assert_eq!(
        future_profile["baselineIndexPath"],
        display_path(&new_root.join("baseline/future.jsonl"))
    );

    for (profile_id, archive_root, before) in [
        (
            "internal-baseline",
            new_root.join("archives/internal"),
            internal_before,
        ),
        (
            "external-baseline",
            new_root.join("archives/external"),
            external_before,
        ),
    ] {
        let collection_dir = archive_root.join("collections").join(profile_id);
        let after = snapshot_fixture(&collection_dir);
        assert_eq!(after.snapshot_id, before.snapshot_id);
        assert_eq!(after.semantic_hash, before.semantic_hash);
        assert_eq!(after.raw_hash, before.raw_hash);
        assert_eq!(after.raw_bytes, before.raw_bytes);
        assert_eq!(after.semantic_bytes, before.semantic_bytes);
        assert_eq!(after.source_path, display_path(&history));
        assert!(Path::new(&after.snapshot_path).starts_with(&new_root));
        assert_eq!(after.snapshot_semantic_path, after.semantic_path);
        assert_eq!(after.snapshot_markdown_path, after.semantic_markdown_path);
        assert_eq!(after.snapshot_raw_path, after.raw_path);

        let collection =
            read_json_or_default(&collection_dir.join(COLLECTION_JSON), || json!({})).unwrap();
        assert_eq!(collection["snapshotRoot"], display_path(&archive_root));
        assert_eq!(
            collection["archiveProfile"]["archiveRoot"],
            display_path(&archive_root)
        );
        let expected_baseline = if profile_id == "internal-baseline" {
            new_root.join("baseline/internal.jsonl")
        } else {
            external_baseline.clone()
        };
        assert_eq!(
            collection["archiveProfile"]["baselineIndexPath"],
            display_path(&expected_baseline)
        );
        assert_eq!(
            collection["conversations"][0]["semanticDocumentPath"],
            after.semantic_path
        );
        let index = read_index_records(&collection_dir.join(CONVERSATION_INDEX_JSONL)).unwrap();
        assert_eq!(index[0]["source_path"], display_path(&history));
        assert_eq!(index[0]["snapshot_path"], after.snapshot_path);

        let report = archive_report(&json!({
            "stateRoot": display_path(&moved_state),
            "profile": profile_id
        }))
        .unwrap();
        assert_eq!(report["indexCount"], 1);
        assert_eq!(report["validation"]["healthStatus"], "ok");
        if profile_id == "internal-baseline" {
            let verified = archive_verify(&json!({
                "stateRoot": display_path(&moved_state),
                "profile": profile_id
            }))
            .unwrap();
            assert_eq!(verified["validation"]["healthStatus"], "ok");
        }
        let index_markdown =
            fs::read_to_string(collection_dir.join(CONVERSATION_INDEX_MD)).unwrap();
        assert!(index_markdown.contains(&after.semantic_markdown_path));
        let summary = fs::read_to_string(collection_dir.join(SUMMARY_MD)).unwrap();
        assert!(summary.contains(&display_path(&archive_root)));
        if staged {
            assert!(!summary.contains(&display_path(&physical_root)));
        }
    }

    let external_validation_after = read_json_or_default(
        &new_root
            .join("archives/external/collections/external-baseline")
            .join(VALIDATION_JSON),
        || json!({}),
    )
    .unwrap();
    assert_eq!(
        external_validation_after["baseline"],
        external_validation_before["baseline"]
    );

    let activity = fs::read_to_string(moved_state.join("activity/activity.jsonl")).unwrap();
    assert!(activity.contains(&display_path(&snapshot_root)));
    assert!(history.exists());
}

fn snapshot_fixture(collection_dir: &Path) -> SnapshotFixture {
    let index = read_index_records(&collection_dir.join(CONVERSATION_INDEX_JSONL)).unwrap();
    let record = &index[0];
    let snapshot_path = record["snapshot_path"].as_str().unwrap();
    let snapshot: Value =
        serde_json::from_str(&fs::read_to_string(snapshot_path).unwrap()).unwrap();
    let raw_path = record["raw_content_path"].as_str().unwrap();
    let semantic_path = record["semantic_document_path"].as_str().unwrap();
    let semantic_markdown_path = record["semantic_markdown_path"].as_str().unwrap();
    SnapshotFixture {
        snapshot_id: record["snapshot_id"].as_str().unwrap().to_string(),
        semantic_hash: record["semantic_content_hash"]
            .as_str()
            .unwrap()
            .to_string(),
        raw_hash: record["content_fingerprint"].as_str().unwrap().to_string(),
        raw_bytes: fs::read(raw_path).unwrap(),
        semantic_bytes: fs::read(semantic_path).unwrap(),
        source_path: snapshot["sourcePath"].as_str().unwrap().to_string(),
        snapshot_semantic_path: snapshot["semanticDocumentPath"]
            .as_str()
            .unwrap()
            .to_string(),
        snapshot_markdown_path: snapshot["semanticMarkdownPath"]
            .as_str()
            .unwrap()
            .to_string(),
        snapshot_raw_path: snapshot["rawContentPath"].as_str().unwrap().to_string(),
        snapshot_path: snapshot_path.to_string(),
        semantic_path: semantic_path.to_string(),
        semantic_markdown_path: semantic_markdown_path.to_string(),
        raw_path: raw_path.to_string(),
    }
}

struct SnapshotFixture {
    snapshot_id: String,
    semantic_hash: String,
    raw_hash: String,
    raw_bytes: Vec<u8>,
    semantic_bytes: Vec<u8>,
    source_path: String,
    snapshot_semantic_path: String,
    snapshot_markdown_path: String,
    snapshot_raw_path: String,
    snapshot_path: String,
    semantic_path: String,
    semantic_markdown_path: String,
    raw_path: String,
}
