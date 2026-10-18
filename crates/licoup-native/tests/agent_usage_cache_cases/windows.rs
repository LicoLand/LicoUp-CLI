use super::support::*;

#[test]
fn codex_usage_applies_one_local_calendar_window_to_daily_and_total_values() {
    let history_root = temp_dir("codex-usage-window-history");
    let state_root = temp_dir("codex-usage-window-state");
    fs::write(
        history_root.join("rollout.jsonl"),
        [
            r#"{"timestamp":"2026-07-09T15:00:00Z","type":"session_meta","payload":{"id":"session-window"}}"#.to_string(),
            token_event("2026-07-09T15:00:01Z", (100, 20, 10), (100, 20, 10)),
            token_event("2026-07-10T16:30:00Z", (106, 20, 14), (6, 0, 4)),
        ]
        .join("\n"),
    )
    .unwrap();
    let mut params = scan_params(&history_root, &state_root);
    params["historyDays"] = json!(1);
    params["timezoneOffsetMinutes"] = json!(480);
    params["now"] = json!("2026-07-10T17:00:00Z");

    let report = agent_usage::scan(&params).unwrap();

    assert_eq!(report["summary"]["windowStart"], "2026-07-11");
    assert_eq!(report["summary"]["windowEnd"], "2026-07-11");
    assert_eq!(report["summary"]["totalTokens"], 10);
    assert_eq!(
        report["agents"][0]["history"]["dailyUsage"][0]["date"],
        "2026-07-11"
    );
}

#[test]
fn codex_usage_applies_historical_timezone_transitions_per_event() {
    let history_root = temp_dir("codex-usage-dst-history");
    let state_root = temp_dir("codex-usage-dst-state");
    fs::write(
        history_root.join("rollout.jsonl"),
        [
            r#"{"timestamp":"2026-03-08T04:29:00Z","type":"session_meta","payload":{"id":"dst-session"}}"#.to_string(),
            token_event("2026-03-08T04:30:00Z", (6, 2, 4), (6, 2, 4)),
            token_event("2026-03-08T07:30:00Z", (8, 2, 5), (2, 0, 1)),
        ]
        .join("\n"),
    )
    .unwrap();
    let mut params = scan_params(&history_root, &state_root);
    params["historyDays"] = json!(1);
    params["now"] = json!("2026-03-08T12:00:00Z");
    params["timezoneOffsetMinutes"] = json!(-240);
    params["timezoneTransitions"] = json!([
        {"atEpochSeconds": 1772323200_i64, "offsetMinutes": -300},
        {"atEpochSeconds": 1772953200_i64, "offsetMinutes": -240}
    ]);

    let report = agent_usage::scan(&params).unwrap();
    assert_eq!(report["summary"]["totalTokens"], 3);
    assert_eq!(report["window"]["timezoneTransitionCount"], 2);
    assert_eq!(
        report["agents"][0]["history"]["dailyUsage"][0]["date"],
        "2026-03-08"
    );
}
