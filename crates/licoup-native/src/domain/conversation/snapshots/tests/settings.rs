use super::*;

#[test]
fn root_set_initializes_empty_user_root_and_persists_settings() {
    let state = temp_dir("root-state");
    let root = temp_dir("snapshot-root");
    fs::remove_dir_all(&root).unwrap();

    let result = root_set(&json!({
        "stateRoot": display_path(&state),
        "path": display_path(&root)
    }))
    .unwrap();

    assert_eq!(result["ok"], true);
    assert!(root.join(MARKER_FILE).exists());
    let get = root_get(&json!({"stateRoot": display_path(&state)})).unwrap();
    assert_eq!(get["snapshotRoot"], display_path(&root));
    assert_eq!(get["mode"], "user-controlled");
}

#[test]
fn archive_profile_import_list_and_get_round_trip() {
    let state = temp_dir("archive-profile-state");
    let archive_root = temp_dir("archive-profile-root");

    let imported = profile_import(&json!({
        "stateRoot": display_path(&state),
        "profileJson": serde_json::to_string(&json!({
            "profileId": "licomesh",
            "displayName": "LicoMesh",
            "archiveRoot": display_path(&archive_root),
            "canonicalNames": ["LicoMesh"],
            "aliasNames": ["LicoMesh-Archive-Alias"],
            "projectPaths": ["/repo/licomesh"],
            "expectedAgents": ["codex"],
            "expectedSources": ["codex"]
        })).unwrap()
    }))
    .unwrap();
    assert_eq!(imported["status"], "imported");
    assert_eq!(imported["profile"]["profileId"], "licomesh");

    let list = profiles_list(&json!({"stateRoot": display_path(&state)})).unwrap();
    assert_eq!(list["profiles"].as_array().unwrap().len(), 1);
    let get = profile_get(&json!({
        "stateRoot": display_path(&state),
        "profile": "licomesh"
    }))
    .unwrap();
    assert_eq!(get["profile"]["displayName"], "LicoMesh");
    assert_eq!(get["profile"]["expectedAgents"][0], "codex");
}
