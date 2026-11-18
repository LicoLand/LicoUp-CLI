use serde_json::json;

use super::super::super::HistoryPageConfig;
use super::super::dedupe_paging::{
    dedupe_history_sessions, history_session_dedupe_key, paged_history_sessions,
};

#[test]
fn dedupe_uses_native_identity_for_every_adapter() {
    let codex_active = json!({
        "adapterId": "codex", "nativeSessionId": "same", "sourcePath": "active"
    });
    let codex_archive = json!({
        "adapterId": "codex", "nativeSessionId": "same", "sourcePath": "archive"
    });
    assert_eq!(
        history_session_dedupe_key(&codex_active),
        history_session_dedupe_key(&codex_archive)
    );
    assert_eq!(
        dedupe_history_sessions(vec![codex_active, codex_archive]).len(),
        1
    );

    let copilot_store = json!({
        "adapterId": "copilot", "nativeSessionId": "same", "sourcePath": "state.vscdb"
    });
    let copilot_transcript = json!({
        "adapterId": "copilot", "nativeSessionId": "same", "sourcePath": "session.jsonl"
    });
    assert_eq!(
        history_session_dedupe_key(&copilot_store),
        history_session_dedupe_key(&copilot_transcript)
    );
    assert_eq!(
        dedupe_history_sessions(vec![copilot_store, copilot_transcript]).len(),
        1
    );

    let other_a = json!({"adapterId": "pi", "nativeSessionId": "same", "sourcePath": "a"});
    let other_b = json!({"adapterId": "pi", "nativeSessionId": "same", "sourcePath": "b"});
    assert_eq!(
        history_session_dedupe_key(&other_a),
        history_session_dedupe_key(&other_b)
    );
    assert_eq!(dedupe_history_sessions(vec![other_a, other_b]).len(), 1);
}

#[test]
fn paging_applies_offset_and_bounded_end_without_overread() {
    let sessions = (0..5).map(|id| json!({"id": id})).collect();
    let page = HistoryPageConfig {
        offset: 2,
        limit: Some(2),
    };
    let page = paged_history_sessions(sessions, &page);
    assert_eq!(page, vec![json!({"id": 2}), json!({"id": 3})]);

    let empty = paged_history_sessions(
        vec![json!({"id": 1})],
        &HistoryPageConfig {
            offset: 2,
            limit: Some(1),
        },
    );
    assert!(empty.is_empty());
}

#[test]
fn richest_copy_wins_and_complementary_runtime_archive_metadata_survives() {
    let richest = json!({
        "adapterId": "codex",
        "nativeSessionId": "shared",
        "sourcePath": "active",
        "messages": [
            {"id": "m1", "role": "user", "text": "one"},
            {"id": "m2", "role": "agent", "text": "two"},
            {"id": "m3", "role": "agent", "text": "three"}
        ],
        "messageCount": 3
    });
    let metadata = json!({
        "adapterId": "codex",
        "nativeSessionId": "shared",
        "sourcePath": "archive",
        "messages": [{"id": "m1", "role": "user", "text": "one"}],
        "messageCount": 1,
        "workingDirectory": "/synthetic/workspace",
        "model": "synthetic-model",
        "runtime": {"name": "native"},
        "archived": true,
        "archivePath": "/synthetic/archive"
    });
    let merged = dedupe_history_sessions(vec![metadata, richest]);
    assert_eq!(merged.len(), 1);
    assert_eq!(merged[0]["messages"].as_array().unwrap().len(), 3);
    assert_eq!(merged[0]["workingDirectory"], "/synthetic/workspace");
    assert_eq!(merged[0]["model"], "synthetic-model");
    assert_eq!(merged[0]["runtime"]["name"], "native");
    assert_eq!(merged[0]["archived"], true);
    assert_eq!(merged[0]["archivePath"], "/synthetic/archive");
}
