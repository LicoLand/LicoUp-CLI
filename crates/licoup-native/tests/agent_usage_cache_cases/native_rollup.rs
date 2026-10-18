use super::support::*;
fn native_event(timestamp: &str, prompt: u64, completion: u64) -> String {
    json!({
        "type": "assistant",
        "timestamp": timestamp,
        "sessionId": "native-session",
        "model": "claude-test",
        "usage": {
            "input_tokens": prompt,
            "output_tokens": completion
        }
    })
    .to_string()
}

fn native_params(history_root: &PathBuf, state_root: &PathBuf, now: &str) -> Value {
    json!({
        "agent": "claude-code",
        "root": history_root.to_string_lossy(),
        "stateRoot": state_root.to_string_lossy(),
        "forceRefresh": true,
        "historyDays": 30,
        "now": now
    })
}

#[test]
fn native_usage_finalizes_past_days_and_only_parses_appended_bytes() {
    let history_root = temp_dir("native-usage-cache-history");
    let state_root = temp_dir("native-usage-cache-state");
    let transcript = history_root.join("session.jsonl");
    fs::write(
        &transcript,
        [
            native_event("2026-07-09T10:00:00Z", 10, 2),
            native_event("2026-07-10T10:00:00Z", 20, 3),
        ]
        .join("\n")
            + "\n",
    )
    .unwrap();
    let params = native_params(&history_root, &state_root, "2026-07-10T12:00:00Z");

    let cold = agent_usage::scan(&params).unwrap();
    let history = &cold["agents"][0]["history"];
    assert_eq!(history["totalTokens"], 35);
    assert_eq!(history["scanCache"]["compactedDays"], 1);
    assert_eq!(history["scanCache"]["replacedSources"], 1);
    assert_eq!(history["scanCache"]["connectionOpens"], 2);
    assert_eq!(history["scanCache"]["leases"], 3);
    assert!(history["scanCache"]["transactionMillis"].as_u64().is_some());
    assert!(history["scanCache"]["parsedBytes"].as_u64().unwrap() > 0);

    let cache = state_root.join("agent-usage-rollups-v2.sqlite3");
    let connection = SqliteConnection::open(&cache).unwrap();
    assert_eq!(
        connection
            .query_row(
                "SELECT COUNT(*) FROM native_usage_daily_totals",
                [],
                |row| { row.get::<_, i64>(0) }
            )
            .unwrap(),
        1
    );
    assert_eq!(
        connection
            .query_row("SELECT COUNT(*) FROM native_usage_source_days", [], |row| {
                row.get::<_, i64>(0)
            })
            .unwrap(),
        1
    );
    drop(connection);

    let warm = agent_usage::scan(&params).unwrap();
    let warm_cache = &warm["agents"][0]["history"]["scanCache"];
    assert_eq!(warm_cache["reusedSources"], 1);
    assert_eq!(warm_cache["parsedBytes"], 0);
    assert_eq!(warm_cache["leases"], 3);
    assert!(warm_cache["connectionOpens"].as_u64().unwrap() <= 2);
    assert!(warm_cache["statements"].as_u64().unwrap() < 30);
    let warm_statements = warm_cache["statements"].as_u64().unwrap();

    writeln!(
        fs::OpenOptions::new()
            .append(true)
            .open(&transcript)
            .unwrap(),
        "{}",
        native_event("2026-07-10T11:00:00Z", 5, 1)
    )
    .unwrap();
    let appended = agent_usage::scan(&params).unwrap();
    assert_eq!(appended["agents"][0]["history"]["totalTokens"], 41);
    assert_eq!(
        appended["agents"][0]["history"]["scanCache"]["appendedSources"],
        1
    );
    assert!(
        appended["agents"][0]["history"]["scanCache"]["statements"]
            .as_u64()
            .unwrap()
            > warm_statements
    );
    assert!(
        appended["agents"][0]["history"]["scanCache"]["parsedBytes"]
            .as_u64()
            .unwrap()
            < fs::metadata(&transcript).unwrap().len()
    );

    let next_day = agent_usage::scan(&native_params(
        &history_root,
        &state_root,
        "2026-07-11T12:00:00Z",
    ))
    .unwrap();
    assert_eq!(next_day["agents"][0]["history"]["totalTokens"], 41);
    assert_eq!(
        next_day["agents"][0]["history"]["scanCache"]["compactedDays"],
        1
    );
    let connection = SqliteConnection::open(&cache).unwrap();
    assert_eq!(
        connection
            .query_row(
                "SELECT COUNT(*) FROM native_usage_daily_totals",
                [],
                |row| { row.get::<_, i64>(0) }
            )
            .unwrap(),
        2
    );
    assert_eq!(
        connection
            .query_row("SELECT COUNT(*) FROM native_usage_source_days", [], |row| {
                row.get::<_, i64>(0)
            })
            .unwrap(),
        0
    );
    drop(connection);

    let mut suffix = fs::OpenOptions::new()
        .append(true)
        .open(&transcript)
        .unwrap();
    let late_suffix = [
        native_event("2026-07-10T23:59:00Z", 100, 1),
        native_event("2026-07-11T10:00:00Z", 3, 1),
    ]
    .join("\n");
    writeln!(suffix, "{late_suffix}").unwrap();
    let rollover = native_params(&history_root, &state_root, "2026-07-11T12:00:00Z");
    let refreshed = agent_usage::scan(&rollover).unwrap();
    assert_eq!(refreshed["agents"][0]["history"]["totalTokens"], 45);

    fs::remove_dir_all(history_root).unwrap();
    fs::remove_dir_all(state_root).unwrap();
}
