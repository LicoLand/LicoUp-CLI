use std::fs;
use std::time::{SystemTime, UNIX_EPOCH};

use rusqlite::Connection;
use serde_json::json;

use super::super::super::{HistoryAdapter, HistoryPageConfig, HistoryScanConfig};
use super::super::fallback::parse_generic_sqlite_sessions;

#[test]
fn generic_fallback_projects_only_adapter_accepted_rows() {
    let unique = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock")
        .as_nanos();
    let path = std::env::temp_dir().join(format!("lico-sqlite-fallback-{unique}.db"));
    let connection = Connection::open(&path).expect("database");
    connection
        .execute("CREATE TABLE ItemTable (key TEXT, value TEXT)", [])
        .expect("table");
    connection
        .execute(
            "INSERT INTO ItemTable (key, value) VALUES (?1, ?2)",
            ["chat.first", "user message: visible prompt"],
        )
        .expect("accepted row");
    connection
        .execute(
            "INSERT INTO ItemTable (key, value) VALUES (?1, ?2)",
            ["settings.theme", "dark"],
        )
        .expect("rejected row");
    let metadata = fs::metadata(&path).expect("metadata");
    let sessions = parse_generic_sqlite_sessions(
        HistoryAdapter::Cursor,
        &path,
        "sqlite",
        &metadata,
        &HistoryScanConfig {
            archive_mode: false,
            session_ids: Vec::new(),
            match_terms: Vec::new(),
            match_project_paths: Vec::new(),
            page: HistoryPageConfig {
                offset: 0,
                limit: None,
            },
        },
        &connection,
    );
    assert_eq!(sessions.len(), 1);
    assert_eq!(sessions[0]["nativeSessionId"], "chat.first");
    drop(connection);
    fs::remove_file(path).expect("remove fixture");
}

#[test]
fn generic_sqlite_reads_all_pages_and_large_values() {
    let unique = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock")
        .as_nanos();
    let path = std::env::temp_dir().join(format!("lico-sqlite-complete-{unique}.db"));
    let mut connection = Connection::open(&path).expect("database");
    connection
        .execute("CREATE TABLE ItemTable (key TEXT, value TEXT)", [])
        .expect("table");
    let transaction = connection.transaction().expect("transaction");
    {
        let mut insert = transaction
            .prepare("INSERT INTO ItemTable (key, value) VALUES (?1, ?2)")
            .expect("insert");
        for index in 0..2_001 {
            let content = if index == 1_000 {
                "x".repeat(4 * 1024 * 1024 + 1)
            } else {
                format!("message-{index}")
            };
            insert
                .execute([
                    format!("chat.{index:04}"),
                    json!({"role": "user", "content": content}).to_string(),
                ])
                .expect("row");
        }
    }
    transaction.commit().expect("commit");
    let metadata = fs::metadata(&path).expect("metadata");
    let sessions = parse_generic_sqlite_sessions(
        HistoryAdapter::Cursor,
        &path,
        "sqlite",
        &metadata,
        &HistoryScanConfig {
            archive_mode: false,
            session_ids: Vec::new(),
            match_terms: Vec::new(),
            match_project_paths: Vec::new(),
            page: HistoryPageConfig {
                offset: 0,
                limit: None,
            },
        },
        &connection,
    );
    assert_eq!(sessions.len(), 2_001);
    assert!(
        sessions
            .iter()
            .any(|session| session["nativeSessionId"] == "chat.2000")
    );
    let large = sessions
        .iter()
        .find(|session| session["nativeSessionId"] == "chat.1000")
        .expect("large session");
    assert_eq!(
        large["messages"][0]["text"].as_str().unwrap().len(),
        4 * 1024 * 1024 + 1
    );
    drop(connection);
    fs::remove_file(path).expect("remove fixture");
}
