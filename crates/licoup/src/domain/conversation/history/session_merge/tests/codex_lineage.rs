use std::collections::BTreeMap;

use serde_json::{Value, json};

use super::super::codex_lineage::{
    codex_rollout_lineage_parents, codex_rollout_lineage_root, merge_codex_rollout_lineage_sessions,
};

fn codex_session(id: &str, parent: Option<&str>, updated_at: &str, messages: Vec<Value>) -> Value {
    let message_count = messages.len();
    let mut session = json!({
        "adapterId": "codex",
        "nativeSessionId": id,
        "createdAt": "2026-01-01T00:00:00Z",
        "updatedAt": updated_at,
        "messages": messages,
        "messageCount": message_count
    });
    if let Some(parent) = parent {
        session["parentSessionId"] = json!(parent);
    }
    session
}

#[test]
fn lineage_parent_and_cycle_roots_are_deterministic() {
    let sessions = vec![
        codex_session("root", None, "1", vec![]),
        codex_session("child", Some("root"), "2", vec![]),
    ];
    let parents = codex_rollout_lineage_parents(&sessions);
    assert_eq!(parents.get("child").map(String::as_str), Some("root"));
    assert_eq!(codex_rollout_lineage_root("child", &parents), "root");

    let cycle = BTreeMap::from([
        ("alpha".to_string(), "beta".to_string()),
        ("beta".to_string(), "alpha".to_string()),
    ]);
    assert_eq!(codex_rollout_lineage_root("beta", &cycle), "alpha");
}

#[test]
fn lineage_merge_prefers_tip_and_deduplicates_replayed_thread_messages() {
    let shared = json!({
        "id": "shared", "role": "user", "text": "Same prompt", "createdAt": 1
    });
    let sessions = vec![
        codex_session("root", None, "1", vec![shared.clone()]),
        codex_session(
            "tip",
            Some("root"),
            "2",
            vec![
                shared,
                json!({"id": "reply", "role": "assistant", "text": "Reply", "createdAt": 2}),
            ],
        ),
    ];
    let merged = merge_codex_rollout_lineage_sessions(sessions);
    assert_eq!(merged.len(), 1);
    assert_eq!(merged[0]["nativeSessionId"], "tip");
    assert_eq!(merged[0]["lineageRootId"], "root");
    assert_eq!(merged[0]["messages"].as_array().unwrap().len(), 2);
    assert!(merged[0].get("parentSessionId").is_none());
}

#[test]
fn active_lineage_member_marks_the_collapsed_conversation_running() {
    let root = codex_session(
        "root",
        None,
        "1",
        vec![json!({"role": "user", "text": "Start"})],
    );
    let mut tip = codex_session(
        "tip",
        Some("root"),
        "2",
        vec![json!({"role": "assistant", "text": "Working"})],
    );
    tip["running"] = json!(true);

    let merged = merge_codex_rollout_lineage_sessions(vec![root, tip]);
    assert_eq!(merged.len(), 1);
    assert_eq!(merged[0]["running"], true);
}
