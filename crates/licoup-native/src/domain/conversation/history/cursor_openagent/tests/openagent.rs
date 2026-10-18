use rusqlite::Connection;
use serde_json::json;

use super::super::openagent::{
    openagent_json_time, openagent_messages_for_session, openagent_session_rows,
    openagent_usage_from_columns,
};

#[test]
fn openagent_session_rows_and_usage_projection_remain_additive() {
    let connection = Connection::open_in_memory().expect("database");
    connection
        .execute_batch(
            "CREATE TABLE session (
                id TEXT, title TEXT, directory TEXT, path TEXT, agent TEXT, model TEXT,
                time_created INTEGER, time_updated INTEGER, tokens_input INTEGER,
                tokens_output INTEGER, tokens_reasoning INTEGER, tokens_cache_read INTEGER,
                tokens_cache_write INTEGER
             );
             INSERT INTO session VALUES (
                'session-1', 'Title', 'work', 'project', 'build', 'model-a',
                1773798000000, 1773798100000, 60, 5, 2, 30, 10
             );",
        )
        .expect("fixture");
    assert_eq!(openagent_session_rows(&connection).len(), 1);

    let usage = openagent_usage_from_columns(Some(60), Some(5), Some(2), Some(30), Some(10))
        .expect("usage");
    assert_eq!(usage["promptTokens"], 100);
    assert_eq!(usage["completionTokens"], 7);
    assert_eq!(usage["totalTokens"], 107);
}

#[test]
fn openagent_nested_time_projection_accepts_epoch_values() {
    assert!(
        openagent_json_time(
            &json!({"time": {"created": 1_773_798_000_000i64}}),
            "created"
        )
        .is_some()
    );
    assert!(openagent_json_time(&json!({}), "created").is_none());
}

#[test]
fn openagent_large_values_and_narrow_schema_are_complete() {
    let connection = Connection::open_in_memory().expect("database");
    connection
        .execute_batch(
            "CREATE TABLE session (id TEXT, title TEXT);
             CREATE TABLE message (id TEXT, session_id TEXT, data TEXT);
             CREATE TABLE part (message_id TEXT, session_id TEXT, data TEXT);
             INSERT INTO session VALUES ('narrow-session', 'Narrow');
             INSERT INTO message VALUES (
               'message-1', 'narrow-session', '{\"role\":\"assistant\"}'
             );",
        )
        .expect("fixture");
    let large = json!({
        "type": "text",
        "text": "x".repeat(4 * 1024 * 1024 + 1)
    })
    .to_string();
    connection
        .execute(
            "INSERT INTO part (message_id, session_id, data) VALUES (?1, ?2, ?3)",
            ("message-1", "narrow-session", large.as_str()),
        )
        .expect("large part");

    assert_eq!(openagent_session_rows(&connection).len(), 1);
    let messages = openagent_messages_for_session(
        super::super::super::HistoryAdapter::OpenCode,
        std::path::Path::new("synthetic.db"),
        &connection,
        "narrow-session",
    );
    assert_eq!(messages.len(), 1);
    assert_eq!(
        messages[0]["text"].as_str().expect("large text").len(),
        4 * 1024 * 1024 + 1
    );
}
