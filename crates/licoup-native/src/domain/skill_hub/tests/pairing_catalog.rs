use super::super::*;
use super::support::*;

#[test]
fn pairing_lifecycle_controls_local_catalog_access() {
    let store = test_store("pairing-lifecycle");
    let root = local_catalog("pairing-local", "review");

    let unpaired = skill_list_in(
        &store,
        &json!({"agent": "custom-agent", "skillRoot": display_path(root.clone())}),
    )
    .unwrap();
    assert_eq!(unpaired["error"], "pairing_required");

    let requested = pair_request_in(
        &store,
        &json!({
            "agent": "custom-agent",
            "target": "custom-agent",
            "defaultVisibilityPolicy": "allow-all"
        }),
    )
    .unwrap();
    assert_eq!(requested["status"], STATUS_APPROVED);

    let listed = skill_list_in(
        &store,
        &json!({"agent": "custom-agent", "skillRoot": display_path(root.clone())}),
    )
    .unwrap();
    assert_eq!(listed["skills"].as_array().unwrap().len(), 1);
    assert_eq!(listed["skills"][0]["skillId"], "review");

    pair_revoke_in(&store, &json!({"agent": "custom-agent"})).unwrap();
    let revoked = skill_list_in(
        &store,
        &json!({"agent": "custom-agent", "skillRoot": display_path(root)}),
    )
    .unwrap();
    assert_eq!(revoked["error"], "pairing_required");
}

#[test]
fn local_catalog_get_never_uses_managed_install_state() {
    let store = test_store("local-get");
    let root = local_catalog("local-get-root", "review");
    pair_request_in(
        &store,
        &json!({
            "agent": "custom-agent",
            "defaultVisibilityPolicy": "allow-all"
        }),
    )
    .unwrap();

    let result = skill_get_in(
        &store,
        &json!({
            "agent": "custom-agent",
            "skill": "review",
            "skillRoot": display_path(root)
        }),
    )
    .unwrap();
    assert_eq!(result["ok"], true);
    assert_eq!(result["source"], "local-agent-skill-roots");
}

#[test]
fn local_visibility_hides_an_existing_skill() {
    let store = test_store("visibility");
    let root = local_catalog("visibility-root", "review");
    pair_request_in(
        &store,
        &json!({
            "agent": "custom-agent",
            "defaultVisibilityPolicy": "allow-all"
        }),
    )
    .unwrap();
    skill_visibility_in(
        &store,
        &json!({"agent": "custom-agent", "skill": "review", "hidden": true}),
    )
    .unwrap();

    let listed = skill_list_in(
        &store,
        &json!({"agent": "custom-agent", "skillRoot": display_path(root)}),
    )
    .unwrap();
    assert!(listed["skills"].as_array().unwrap().is_empty());
}

#[test]
fn parsing_helpers_keep_local_catalog_inputs_bounded() {
    assert_eq!(
        string_param(&json!({"agentId": "codex"}), &["agent", "agentId"], 0).unwrap(),
        "codex"
    );
    assert_eq!(
        bool_param(&json!({"hidden": "hidden"}), "hidden"),
        Some(true)
    );
    assert_eq!(bool_param(&json!({"hidden": "no"}), "hidden"), Some(false));
}

fn local_catalog(name: &str, skill_id: &str) -> PathBuf {
    let root = temp_test_dir(name);
    let skill = root.join(skill_id);
    fs::create_dir_all(&skill).unwrap();
    fs::write(
        skill.join("SKILL.md"),
        format!("---\nname: {skill_id}\ntitle: Local Skill\nversion: local\n---\n"),
    )
    .unwrap();
    root
}
