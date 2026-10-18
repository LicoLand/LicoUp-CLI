use rusqlite::Connection;
use serde_json::json;

use super::super::cursor::{cursor_composer_rows, cursor_disk_kv_json};

#[test]
fn cursor_sql_parser_reads_composer_and_bubble_records_from_bounded_codec() {
    let connection = Connection::open_in_memory().expect("database");
    connection
        .execute("CREATE TABLE cursorDiskKV (key TEXT, value BLOB)", [])
        .expect("table");
    connection
        .execute(
            "INSERT INTO cursorDiskKV (key, value) VALUES (?1, ?2)",
            rusqlite::params![
                "composerData:session-1",
                serde_json::to_vec(&json!({
                    "composerId": "session-1",
                    "name": "Cursor session",
                    "modelConfig": {"selectedModels": [{"modelId": "model-a"}]},
                    "fullConversationHeadersOnly": [{"bubbleId": "bubble-1"}]
                }))
                .unwrap()
            ],
        )
        .expect("composer");
    connection
        .execute(
            "INSERT INTO cursorDiskKV (key, value) VALUES (?1, ?2)",
            rusqlite::params![
                "bubbleId:session-1:bubble-1",
                serde_json::to_vec(&json!({"type": 1, "text": "Prompt"})).unwrap()
            ],
        )
        .expect("bubble");

    assert_eq!(cursor_composer_rows(&connection).len(), 1);
    assert_eq!(
        cursor_disk_kv_json(&connection, "bubbleId:session-1:bubble-1").unwrap()["text"],
        "Prompt"
    );
}
