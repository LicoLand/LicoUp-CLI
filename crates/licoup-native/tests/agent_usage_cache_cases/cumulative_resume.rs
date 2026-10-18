use super::support::*;

fn params(history_root: &PathBuf, state_root: &PathBuf, now: &str) -> Value {
    agent_params("hermes", history_root, state_root, now)
}

fn agent_params(agent: &str, history_root: &PathBuf, state_root: &PathBuf, now: &str) -> Value {
    json!({
        "agent": agent,
        "root": history_root.to_string_lossy(),
        "stateRoot": state_root.to_string_lossy(),
        "forceRefresh": true,
        "historyDays": 30,
        "now": now
    })
}

fn epoch(value: &str) -> f64 {
    time::OffsetDateTime::parse(value, &time::format_description::well_known::Rfc3339)
        .unwrap()
        .unix_timestamp() as f64
}

fn update_usage(database: &PathBuf, input: i64, output: i64, last_seen: &str) {
    let connection = SqliteConnection::open(database).unwrap();
    connection
        .execute(
            "UPDATE session_model_usage
             SET input_tokens=?1,output_tokens=?2,last_seen=?3
             WHERE session_id='resumed-session'",
            rusqlite::params![input, output, epoch(last_seen)],
        )
        .unwrap();
    connection
        .execute(
            "UPDATE sessions SET input_tokens=?1,output_tokens=?2
             WHERE id='resumed-session'",
            rusqlite::params![input, output],
        )
        .unwrap();
}

#[test]
fn cumulative_metadata_counts_new_usage_when_an_old_session_resumes() {
    let history_root = temp_dir("native-cumulative-resume-history");
    let state_root = temp_dir("native-cumulative-resume-state");
    let database = history_root.join("state.db");
    let connection = SqliteConnection::open(&database).unwrap();
    connection
        .execute_batch(
            "CREATE TABLE sessions(
               id TEXT PRIMARY KEY,model TEXT,started_at REAL NOT NULL,ended_at REAL,
               input_tokens INTEGER,output_tokens INTEGER,
               cache_read_tokens INTEGER,cache_write_tokens INTEGER
             );
             CREATE TABLE messages(session_id TEXT NOT NULL,timestamp REAL NOT NULL);
             CREATE INDEX messages_session_timestamp ON messages(session_id,timestamp);
             CREATE TABLE session_model_usage(
               session_id TEXT NOT NULL,model TEXT NOT NULL,
               billing_provider TEXT NOT NULL DEFAULT '',
               billing_base_url TEXT NOT NULL DEFAULT '',
               billing_mode TEXT NOT NULL DEFAULT '',task TEXT NOT NULL DEFAULT '',
               input_tokens INTEGER,output_tokens INTEGER,
               cache_read_tokens INTEGER,cache_write_tokens INTEGER,
               reasoning_tokens INTEGER,first_seen REAL,last_seen REAL,
               PRIMARY KEY(
                 session_id,model,billing_provider,billing_base_url,billing_mode,task
               )
             );",
        )
        .unwrap();
    connection
        .execute(
            "INSERT INTO sessions VALUES(
               'resumed-session','model-a',?1,NULL,100,10,0,0
             )",
            [epoch("2026-07-01T10:00:00Z")],
        )
        .unwrap();
    connection
        .execute(
            "INSERT INTO session_model_usage VALUES(
               'resumed-session','model-a','','','','',100,10,0,0,0,?1,?2
             )",
            rusqlite::params![epoch("2026-07-01T10:00:00Z"), epoch("2026-07-10T09:00:00Z")],
        )
        .unwrap();
    drop(connection);
    let today = params(&history_root, &state_root, "2026-07-10T12:00:00Z");

    let baseline = agent_usage::scan(&today).unwrap();
    assert_eq!(baseline["summary"]["totalTokens"], 110);

    update_usage(&database, 106, 14, "2026-07-10T13:00:00Z");
    let resumed = agent_usage::scan(&today).unwrap();
    assert_eq!(resumed["summary"]["totalTokens"], 120);
    assert_eq!(
        agent_usage::scan(&today).unwrap()["summary"]["totalTokens"],
        120
    );

    update_usage(&database, 2, 1, "2026-07-10T14:00:00Z");
    assert_eq!(
        agent_usage::scan(&today).unwrap()["summary"]["totalTokens"],
        120
    );
    update_usage(&database, 5, 2, "2026-07-10T15:00:00Z");
    assert_eq!(
        agent_usage::scan(&today).unwrap()["summary"]["totalTokens"],
        124
    );

    update_usage(&database, 9, 4, "2026-07-11T10:00:00Z");
    let tomorrow = params(&history_root, &state_root, "2026-07-11T12:00:00Z");
    let rolled = agent_usage::scan(&tomorrow).unwrap();
    assert_eq!(rolled["summary"]["totalTokens"], 130);
    assert_eq!(
        rolled["agents"][0]["history"]["scanCache"]["compactedDays"],
        1
    );
    let cache = SqliteConnection::open(state_root.join("agent-usage-rollups-v2.sqlite3")).unwrap();
    let (session_key, usage_key): (String, String) = cache
        .query_row(
            "SELECT session_key,usage_key FROM native_usage_watermarks LIMIT 1",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap();
    assert_eq!((session_key.len(), usage_key.len()), (64, 64));
    assert!(!format!("{session_key}{usage_key}").contains("resumed-session"));
    drop(cache);

    fs::remove_dir_all(history_root).unwrap();
    fs::remove_dir_all(state_root).unwrap();
}

fn cumulative_event(day: &str, input: u64, output: u64) -> String {
    json!({
        "type": "assistant",
        "timestamp": format!("{day}T10:00:00Z"),
        "usageScope": "session",
        "sessionId": "resumed-session",
        "model": "cumulative-test",
        "usage": {"input_tokens": input, "output_tokens": output}
    })
    .to_string()
}

#[test]
fn cumulative_append_rewrite_preserves_today_and_only_adds_new_delta() {
    let history_root = temp_dir("native-cumulative-rewrite-history");
    let state_root = temp_dir("native-cumulative-rewrite-state");
    let wire = history_root.join("resumed.jsonl");
    let old = cumulative_event("2026-07-09", 100, 10);
    let today = cumulative_event("2026-07-10", 106, 14);
    fs::write(&wire, format!("{old}\n{today}\n")).unwrap();
    let scan_params = json!({
        "agent": "claude-code", "root": history_root.to_string_lossy(),
        "stateRoot": state_root.to_string_lossy(), "forceRefresh": true,
        "historyDays": 30, "now": "2026-07-10T12:00:00Z"
    });

    assert_eq!(
        agent_usage::scan(&scan_params).unwrap()["summary"]["totalTokens"],
        120
    );
    fs::write(&wire, format!("{old}\n{today}\n")).unwrap();
    assert_eq!(
        agent_usage::scan(&scan_params).unwrap()["summary"]["totalTokens"],
        120
    );
    fs::write(
        &wire,
        format!("{old}\n{}\n", cumulative_event("2026-07-10", 110, 16)),
    )
    .unwrap();
    assert_eq!(
        agent_usage::scan(&scan_params).unwrap()["summary"]["totalTokens"],
        126
    );

    fs::remove_dir_all(history_root).unwrap();
    fs::remove_dir_all(state_root).unwrap();
}

#[test]
fn openagent_today_query_detects_a_resumed_cross_day_session() {
    let history_root = temp_dir("native-openagent-resume-history");
    let state_root = temp_dir("native-openagent-resume-state");
    let database = history_root.join("opencode.db");
    let connection = SqliteConnection::open(&database).unwrap();
    connection
        .execute_batch(
            "CREATE TABLE session(
               id TEXT,model TEXT,time_created INTEGER,time_updated INTEGER,
               tokens_input INTEGER,tokens_output INTEGER,tokens_reasoning INTEGER,
               tokens_cache_read INTEGER,tokens_cache_write INTEGER
             );
             INSERT INTO session VALUES(
               'resumed-session','model-a',1782871200000,1783648800000,
               100,10,0,0,0
             );",
        )
        .unwrap();
    drop(connection);
    let scan_params = agent_params(
        "opencode",
        &history_root,
        &state_root,
        "2026-07-10T12:00:00Z",
    );
    assert_eq!(
        agent_usage::scan(&scan_params).unwrap()["summary"]["totalTokens"],
        0
    );
    let connection = SqliteConnection::open(&database).unwrap();
    connection
        .execute(
            "UPDATE session SET tokens_input=106,tokens_output=14,
             time_updated=1783652400000 WHERE id='resumed-session'",
            [],
        )
        .unwrap();
    drop(connection);
    assert_eq!(
        agent_usage::scan(&scan_params).unwrap()["summary"]["totalTokens"],
        10
    );

    fs::remove_dir_all(history_root).unwrap();
    fs::remove_dir_all(state_root).unwrap();
}
