use super::support::*;

#[test]
fn codex_usage_does_not_advance_cache_past_an_incomplete_jsonl_line() {
    let history_root = temp_dir("codex-usage-partial-history");
    let state_root = temp_dir("codex-usage-partial-state");
    let rollout = history_root.join("rollout.jsonl");
    fs::write(
        &rollout,
        concat!(
            r#"{"timestamp":"2026-07-10T10:00:00Z","type":"session_meta","payload":{"id":"session-partial"}}"#,
            "\n",
            r#"{"timestamp":"2026-07-10T10:00:01Z","type":"event_msg","payload":{"type":"token_count","info":{"total_token_usage":{"input_tokens":6,"cached_input_tokens":2,"output_tokens":4},"last_token_usage":{"input_tokens":6"#
        ),
    )
    .unwrap();
    let params = scan_params(&history_root, &state_root);
    let first = agent_usage::scan(&params).unwrap();
    assert_eq!(first["summary"]["totalTokens"], 0);

    let mut file = fs::OpenOptions::new().append(true).open(&rollout).unwrap();
    writeln!(
        file,
        "{}",
        r#", "cached_input_tokens":2,"output_tokens":4}}}}"#
    )
    .unwrap();

    let completed = agent_usage::scan(&params).unwrap();
    assert_eq!(completed["agents"][0]["history"]["totalTokens"], 10);
    assert_eq!(
        completed["agents"][0]["history"]["scanCache"]["appendedFiles"],
        1
    );
}

#[test]
fn codex_usage_cache_keeps_independent_roots_warm() {
    let first_root = temp_dir("codex-usage-multi-root-first");
    let second_root = temp_dir("codex-usage-multi-root-second");
    let state_root = temp_dir("codex-usage-multi-root-state");
    fs::write(
        first_root.join("rollout.jsonl"),
        token_event("2026-07-08T10:00:01Z", (6, 2, 4), (6, 2, 4)),
    )
    .unwrap();
    fs::write(
        second_root.join("rollout.jsonl"),
        token_event("2026-07-08T10:00:01Z", (8, 2, 5), (8, 2, 5)),
    )
    .unwrap();
    agent_usage::scan(&scan_params(&first_root, &state_root)).unwrap();
    agent_usage::scan(&scan_params(&second_root, &state_root)).unwrap();

    let mut first_params = scan_params(&first_root, &state_root);
    first_params["forceRefresh"] = json!(false);
    let first_again = agent_usage::scan(&first_params).unwrap();
    assert_eq!(first_again["summary"]["totalTokens"], 10);
    assert_eq!(
        first_again["agents"][0]["history"]["scanCache"]["fresh"],
        true
    );
    let root_cache_count = fs::read_dir(&state_root)
        .unwrap()
        .filter_map(Result::ok)
        .filter(|entry| {
            entry
                .file_name()
                .to_string_lossy()
                .starts_with("agent-usage-cache-v2-")
                && entry.path().extension().and_then(|value| value.to_str()) == Some("sqlite3")
        })
        .count();
    assert_eq!(root_cache_count, 2);
}

#[test]
fn codex_usage_returns_cached_snapshot_when_same_root_refresh_is_busy() {
    let history_root = temp_dir("codex-usage-busy-history");
    let state_root = temp_dir("codex-usage-busy-state");
    fs::write(
        history_root.join("rollout.jsonl"),
        [
            r#"{"timestamp":"2026-07-08T10:00:00Z","type":"session_meta","payload":{"id":"busy-session"}}"#.to_string(),
            token_event("2026-07-08T10:00:01Z", (6, 2, 4), (6, 2, 4)),
        ]
        .join("\n"),
    )
    .unwrap();
    let params = scan_params(&history_root, &state_root);
    assert_eq!(
        agent_usage::scan(&params).unwrap()["summary"]["totalTokens"],
        10
    );
    let database_path = fs::read_dir(&state_root)
        .unwrap()
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .find(|path| {
            path.file_name()
                .and_then(|value| value.to_str())
                .is_some_and(|value| value.starts_with("agent-usage-cache-v2-"))
                && path.extension().and_then(|value| value.to_str()) == Some("sqlite3")
        })
        .unwrap();
    let lock = SqliteConnection::open(database_path).unwrap();
    lock.execute_batch("BEGIN IMMEDIATE").unwrap();

    let report = agent_usage::scan(&params).unwrap();
    lock.execute_batch("ROLLBACK").unwrap();
    let history = &report["agents"][0]["history"];
    assert_eq!(history["totalTokens"], 10);
    assert_eq!(history["scanCache"]["fresh"], false);
    assert_eq!(history["scanCache"]["refreshDeferred"], true);
}
