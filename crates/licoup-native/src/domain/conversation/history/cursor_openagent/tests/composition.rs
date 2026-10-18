use std::fs;
use std::time::{SystemTime, UNIX_EPOCH};

use rusqlite::Connection;
use serde_json::json;

use super::super::super::{HistoryAdapter, HistoryPageConfig, HistoryScanConfig};
use super::super::composition::{
    CopilotChatSessionsReadError, copilot_chat_sessions_document, parse_sqlite_sessions,
};

#[test]
fn cursor_exact_miss_does_not_parse_unrelated_database_records() {
    let unique = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let path = std::env::temp_dir().join(format!("lico-cursor-exact-miss-{unique}.db"));
    let mut connection = Connection::open(&path).unwrap();
    connection
        .execute_batch(
            "CREATE TABLE cursorDiskKV (key TEXT PRIMARY KEY, value TEXT);
        CREATE TABLE ItemTable (key TEXT PRIMARY KEY, value TEXT);",
        )
        .unwrap();
    let transaction = connection.transaction().unwrap();
    transaction
        .execute(
            "INSERT INTO cursorDiskKV VALUES (?1, ?2)",
            [
                "composerData:other",
                &json!({"composerId":"other", "fullConversationHeadersOnly":[{"bubbleId":"one"}]})
                    .to_string(),
            ],
        )
        .unwrap();
    transaction
        .execute(
            "INSERT INTO cursorDiskKV VALUES (?1, ?2)",
            [
                "bubbleId:other:one",
                &json!({"type":1,"text":"Synthetic other conversation"}).to_string(),
            ],
        )
        .unwrap();
    for index in 0..256 {
        transaction
            .execute(
                "INSERT INTO ItemTable VALUES (?1, ?2)",
                [
                    format!("chat.unrelated-{index}"),
                    json!({"role":"user","content":"x".repeat(4096)}).to_string(),
                ],
            )
            .unwrap();
    }
    transaction.commit().unwrap();
    drop(connection);
    let metadata = fs::metadata(&path).unwrap();
    let mut counts = Vec::new();
    for attempt in 0..2 {
        let started = std::time::Instant::now();
        let sessions = parse_sqlite_sessions(
            HistoryAdapter::Cursor,
            &path,
            "sqlite",
            &metadata,
            HistoryScanConfig {
                archive_mode: false,
                session_ids: vec!["missing".into()],
                match_terms: vec![],
                match_project_paths: vec![],
                page: HistoryPageConfig {
                    offset: 0,
                    limit: Some(1),
                },
            },
        );
        eprintln!(
            "synthetic Cursor exact miss attempt={attempt} parsed_rows={} elapsed_us={}",
            sessions.len(),
            started.elapsed().as_micros()
        );
        counts.push(sessions.len());
    }
    // The same recognized store must still serve browse and exact hits.
    for requested in [None, Some("other")] {
        let sessions = parse_sqlite_sessions(
            HistoryAdapter::Cursor,
            &path,
            "sqlite",
            &metadata,
            HistoryScanConfig {
                archive_mode: false,
                session_ids: requested.into_iter().map(str::to_string).collect(),
                match_terms: vec![],
                match_project_paths: vec![],
                page: HistoryPageConfig {
                    offset: 0,
                    limit: Some(1),
                },
            },
        );
        assert_eq!(sessions.len(), 1);
        assert_eq!(sessions[0]["nativeSessionId"], "other");
        assert_eq!(sessions[0]["messages"].as_array().unwrap().len(), 1);
    }
    fs::remove_file(path).unwrap();
    assert_eq!(
        counts,
        vec![0, 0],
        "A recognized store's empty result is final; unrelated records are not history for this identity."
    );
}

#[test]
fn cursor_recognized_empty_stores_do_not_fall_back_to_bookkeeping() {
    for (file, schema) in [
        (
            "state.vscdb",
            "CREATE TABLE cursorDiskKV (key TEXT, value TEXT)",
        ),
        ("store.db", "CREATE TABLE blobs (id TEXT, data BLOB)"),
    ] {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root = std::env::temp_dir().join(format!("lico-cursor-empty-{unique}"));
        fs::create_dir(&root).unwrap();
        let path = root.join(file);
        let connection = Connection::open(&path).unwrap();
        connection.execute_batch(schema).unwrap();
        connection
            .execute_batch(
                "CREATE TABLE ItemTable (key TEXT, value TEXT);
            INSERT INTO ItemTable VALUES ('chat.bookkeeping', 'user message: not a conversation');",
            )
            .unwrap();
        drop(connection);
        let sessions = parse_sqlite_sessions(
            HistoryAdapter::Cursor,
            &path,
            "sqlite",
            &fs::metadata(&path).unwrap(),
            HistoryScanConfig {
                archive_mode: false,
                session_ids: vec![],
                match_terms: vec![],
                match_project_paths: vec![],
                page: HistoryPageConfig {
                    offset: 0,
                    limit: Some(1),
                },
            },
        );
        fs::remove_dir_all(root).unwrap();
        assert!(
            sessions.is_empty(),
            "An empty recognized store cannot invent a bookkeeping conversation."
        );
    }
}

#[test]
fn public_composition_opens_locally_and_routes_to_generic_fallback() {
    let unique = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock")
        .as_nanos();
    let path = std::env::temp_dir().join(format!("lico-sqlite-composition-{unique}.db"));
    let connection = Connection::open(&path).expect("database");
    connection
        .execute("CREATE TABLE ItemTable (key TEXT, value TEXT)", [])
        .expect("table");
    connection
        .execute(
            "INSERT INTO ItemTable (key, value) VALUES (?1, ?2)",
            ["chat.first", "user message: routed prompt"],
        )
        .expect("row");
    drop(connection);
    let metadata = fs::metadata(&path).expect("metadata");
    let sessions = parse_sqlite_sessions(
        HistoryAdapter::Cursor,
        &path,
        "sqlite",
        &metadata,
        HistoryScanConfig {
            archive_mode: false,
            session_ids: Vec::new(),
            match_terms: Vec::new(),
            match_project_paths: Vec::new(),
            page: HistoryPageConfig {
                offset: 0,
                limit: None,
            },
        },
    );
    assert_eq!(sessions.len(), 1);
    fs::remove_file(path).expect("remove fixture");
}

#[test]
fn copilot_document_reader_accepts_payloads_beyond_the_removed_cap_and_case_drift() {
    let connection = Connection::open_in_memory().expect("database");
    connection
        .execute("CREATE TABLE itemtable (key TEXT, value TEXT)", [])
        .expect("table");
    let payload = json!({
        "chatSessions": [{
            "id": "large-session",
            "messages": [{"role": "user", "content": "x".repeat(33 * 1024 * 1024)}]
        }]
    })
    .to_string();
    connection
        .execute(
            "INSERT INTO itemtable (key, value) VALUES (?1, ?2)",
            ["GITHUB.COPILOT-CHAT.CHATSESSIONS", payload.as_str()],
        )
        .expect("row");

    let document = copilot_chat_sessions_document(&connection)
        .expect("read")
        .expect("document");
    assert_eq!(document["chatSessions"][0]["id"], "large-session");
}

#[test]
fn copilot_document_reader_distinguishes_invalid_payloads_from_missing_rows() {
    let connection = Connection::open_in_memory().expect("database");
    connection
        .execute("CREATE TABLE ItemTable (key TEXT, value TEXT)", [])
        .expect("table");
    assert_eq!(copilot_chat_sessions_document(&connection), Ok(None));
    connection
        .execute(
            "INSERT INTO ItemTable (key, value) VALUES (?1, ?2)",
            ["github.copilot-chat.chatSessions", "not-json"],
        )
        .expect("row");
    assert_eq!(
        copilot_chat_sessions_document(&connection),
        Err(CopilotChatSessionsReadError::InvalidPayload)
    );
}
