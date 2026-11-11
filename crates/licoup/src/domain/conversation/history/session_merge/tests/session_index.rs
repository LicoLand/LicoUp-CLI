use std::fs;
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::json;

use super::super::session_index::{
    codex_session_index_candidates, parse_codex_session_index_titles,
    read_codex_session_index_titles_file,
};

#[test]
fn session_index_parser_keeps_latest_meaningful_title_per_session() {
    let raw = [
        r#"{"id":"session-1","thread_name":"First title","updated_at":"2026-01-01T00:00:00Z"}"#,
        "not-json",
        r#"{"session_id":"session-1","title":"Latest title","updatedAt":"2026-01-02T00:00:00Z"}"#,
        r#"{"id":"session-2","title":"Second title","updated_at":"2026-01-01T00:00:00Z"}"#,
    ]
    .join("\n");
    let titles = parse_codex_session_index_titles(&raw);
    assert_eq!(
        titles.get("session-1").map(String::as_str),
        Some("Latest title")
    );
    assert_eq!(
        titles.get("session-2").map(String::as_str),
        Some("Second title")
    );
}

#[test]
fn session_index_candidates_and_file_io_stay_local_and_explicit() {
    let unique = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock")
        .as_nanos();
    let root = std::env::temp_dir().join(format!("lico-session-index-{unique}"));
    fs::create_dir_all(&root).expect("create fixture root");
    let index = root.join("session_index.jsonl");
    fs::write(&index, r#"{"id":"session-1","title":"Local title"}"#).expect("write fixture");

    let candidates = codex_session_index_candidates(&json!({
        "root": root.to_string_lossy(),
        "historyRootKind": "override-root"
    }));
    assert_eq!(candidates.first(), Some(&index));
    assert_eq!(
        read_codex_session_index_titles_file(&index)
            .get("session-1")
            .map(String::as_str),
        Some("Local title")
    );
    fs::remove_dir_all(root).expect("remove fixture root");
}
