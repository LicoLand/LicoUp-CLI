use super::support::*;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::Duration;

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

fn write_native_events(path: &PathBuf, count: u64) {
    let mut content = String::new();
    for index in 0..count {
        content.push_str(&native_event("2026-07-10T10:00:00Z", 1 + index, 1));
        content.push('\n');
    }
    fs::write(path, content).unwrap();
}

#[test]
fn native_usage_keeps_connection_opens_bounded_across_roots() {
    let mut params_by_root = Vec::new();
    for index in 0..5 {
        let history_root = temp_dir(&format!("native-usage-pool-history-{index}"));
        let state_root = temp_dir(&format!("native-usage-pool-state-{index}"));
        write_native_events(&history_root.join("session.jsonl"), 1);
        params_by_root.push((
            native_params(&history_root, &state_root, "2026-07-10T12:00:00Z"),
            history_root,
            state_root,
        ));
    }

    for (params, _, _) in &params_by_root {
        let cold = agent_usage::scan(params).unwrap();
        let scan_cache = &cold["agents"][0]["history"]["scanCache"];
        assert_eq!(scan_cache["connectionOpens"], 2);
        assert_eq!(scan_cache["leases"], 3);
        assert_eq!(cold["summary"]["totalTokens"], 2);
    }

    let (first_params, first_root, first_state) = &params_by_root[0];
    let repeated = agent_usage::scan(first_params).unwrap();
    let scan_cache = &repeated["agents"][0]["history"]["scanCache"];
    assert!(scan_cache["connectionOpens"].as_u64().unwrap() <= 2);
    assert_eq!(scan_cache["leases"], 3);
    assert_eq!(repeated["summary"]["totalTokens"], 2);
    fs::remove_dir_all(first_root).unwrap();
    fs::remove_dir_all(first_state).unwrap();
}

#[test]
fn native_usage_unstable_source_applies_nothing_and_warns() {
    let history_root = temp_dir("native-usage-unstable-history");
    let state_root = temp_dir("native-usage-unstable-state");
    let transcript = history_root.join("session.jsonl");
    write_native_events(&transcript, 60_000);
    let params = native_params(&history_root, &state_root, "2026-07-10T12:00:00Z");

    let cold = agent_usage::scan(&params).unwrap();
    assert!(cold["summary"]["totalTokens"].as_u64().unwrap() > 0);

    let cache = state_root.join("agent-usage-rollups-v2.sqlite3");
    let totals_query = "SELECT COALESCE(SUM(prompt_tokens + completion_tokens), 0) FROM (
        SELECT prompt_tokens, completion_tokens FROM native_usage_daily_totals
        UNION ALL
        SELECT prompt_tokens, completion_tokens FROM native_usage_source_days
    )";
    let connection = SqliteConnection::open(&cache).unwrap();
    let snapshot_totals: i64 = connection
        .query_row(totals_query, [], |row| row.get(0))
        .unwrap();
    let snapshot_sources: i64 = connection
        .query_row("SELECT COUNT(*) FROM native_usage_sources", [], |row| {
            row.get(0)
        })
        .unwrap();
    let snapshot_parsed: i64 = connection
        .query_row(
            "SELECT COALESCE(SUM(parsed_bytes), 0) FROM native_usage_sources",
            [],
            |row| row.get(0),
        )
        .unwrap();
    drop(connection);
    assert!(snapshot_totals > 0);
    assert_eq!(snapshot_sources, 1);

    let scan_params = params.clone();
    let scan_handle = thread::spawn(move || agent_usage::scan(&scan_params));
    let stop = Arc::new(AtomicBool::new(false));
    let writer_stop = Arc::clone(&stop);
    let append_path = transcript.clone();
    let append_handle = thread::spawn(move || {
        let mut appended = 0_u64;
        while !writer_stop.load(Ordering::Relaxed) && appended < 600 {
            writeln!(
                fs::OpenOptions::new()
                    .append(true)
                    .open(&append_path)
                    .unwrap(),
                "{}",
                native_event("2026-07-10T10:00:00Z", 2, 1)
            )
            .unwrap();
            appended += 1;
            thread::sleep(Duration::from_millis(10));
        }
    });

    let report = scan_handle.join().unwrap().unwrap();
    stop.store(true, Ordering::Relaxed);
    append_handle.join().unwrap();

    let warnings = report["warnings"].as_array().unwrap();
    assert!(
        warnings
            .iter()
            .any(|warning| warning["code"] == "native_usage_cache_failed"),
        "expected unstable scan to warn, got {warnings:?}"
    );
    assert_eq!(report["agents"][0]["history"]["totalTokens"], 0);

    let connection = SqliteConnection::open(&cache).unwrap();
    let totals: i64 = connection
        .query_row(totals_query, [], |row| row.get(0))
        .unwrap();
    let sources: i64 = connection
        .query_row("SELECT COUNT(*) FROM native_usage_sources", [], |row| {
            row.get(0)
        })
        .unwrap();
    let parsed: i64 = connection
        .query_row(
            "SELECT COALESCE(SUM(parsed_bytes), 0) FROM native_usage_sources",
            [],
            |row| row.get(0),
        )
        .unwrap();
    drop(connection);
    assert_eq!(totals, snapshot_totals);
    assert_eq!(sources, snapshot_sources);
    assert_eq!(parsed, snapshot_parsed);

    let recovered = agent_usage::scan(&params).unwrap();
    assert!(
        recovered["summary"]["totalTokens"].as_u64().unwrap()
            > cold["summary"]["totalTokens"].as_u64().unwrap()
    );
    assert!(
        recovered["warnings"]
            .as_array()
            .unwrap()
            .iter()
            .all(|warning| warning["code"] != "native_usage_cache_failed")
    );

    fs::remove_dir_all(history_root).unwrap();
    fs::remove_dir_all(state_root).unwrap();
}
