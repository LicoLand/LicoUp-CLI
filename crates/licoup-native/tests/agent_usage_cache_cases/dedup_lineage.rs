use super::support::*;

#[test]
fn codex_usage_unions_active_and_archived_copies_by_event_identity() {
    let history_root = temp_dir("codex-usage-dedup-history");
    let state_root = temp_dir("codex-usage-dedup-state");
    let contents = [
        r#"{"timestamp":"2026-07-08T10:00:00Z","type":"session_meta","payload":{"id":"session-copy"}}"#.to_string(),
        token_event("2026-07-08T10:00:01Z", (10, 4, 2), (10, 4, 2)),
    ]
    .join("\n");
    fs::write(history_root.join("active.jsonl"), &contents).unwrap();
    fs::write(history_root.join("archived.jsonl"), &contents).unwrap();

    let report = agent_usage::scan(&scan_params(&history_root, &state_root)).unwrap();
    let history = &report["agents"][0]["history"];

    assert_eq!(history["sessionCount"], 1);
    assert_eq!(history["totalTokens"], 12);
    assert_eq!(history["tokenSourceBreakdown"]["explicitRecords"], 1);
}

#[test]
fn codex_usage_ignores_text_only_incomplete_copy() {
    let history_root = temp_dir("codex-usage-cross-copy-coverage-history");
    let state_root = temp_dir("codex-usage-cross-copy-coverage-state");
    let metadata = r#"{"timestamp":"2026-07-08T10:00:00Z","type":"session_meta","payload":{"id":"cross-copy-session"}}"#;
    let message = r#"{"timestamp":"2026-07-08T10:00:01Z","type":"response_item","payload":{"type":"message","role":"user","content":[{"type":"input_text","text":"abcd"}]}}"#;
    fs::write(
        history_root.join("active.jsonl"),
        [
            metadata.to_string(),
            message.to_string(),
            token_event("2026-07-08T10:00:02Z", (6, 2, 4), (6, 2, 4)),
        ]
        .join("\n"),
    )
    .unwrap();
    fs::write(
        history_root.join("incomplete-archive.jsonl"),
        [metadata, message].join("\n"),
    )
    .unwrap();

    let report = agent_usage::scan(&scan_params(&history_root, &state_root)).unwrap();
    let history = &report["agents"][0]["history"];
    assert_eq!(history["totalTokens"], 10);
    assert_eq!(history["tokenSourceBreakdown"]["explicitRecords"], 1);
}

#[test]
fn codex_usage_noop_events_do_not_split_copy_identity() {
    let history_root = temp_dir("codex-usage-noop-copy-history");
    let state_root = temp_dir("codex-usage-noop-copy-state");
    let metadata = r#"{"timestamp":"2026-07-08T10:00:00Z","type":"session_meta","payload":{"id":"noop-copy-session"}}"#;
    let first = token_event("2026-07-08T10:00:01Z", (10, 4, 2), (10, 4, 2));
    let second = token_event("2026-07-08T10:00:03Z", (20, 4, 4), (10, 0, 2));
    fs::write(
        history_root.join("canonical.jsonl"),
        [metadata.to_string(), first.clone(), second.clone()].join("\n"),
    )
    .unwrap();
    fs::write(
        history_root.join("copy-with-noops.jsonl"),
        [
            metadata.to_string(),
            first.clone(),
            first,
            r#"{"timestamp":"2026-07-08T10:00:02Z","type":"turn_context","payload":{"model":"gpt-test-codex"}}"#.to_string(),
            second,
        ]
        .join("\n"),
    )
    .unwrap();

    let report = agent_usage::scan(&scan_params(&history_root, &state_root)).unwrap();
    let history = &report["agents"][0]["history"];
    assert_eq!(history["totalTokens"], 24);
    assert_eq!(history["tokenSourceBreakdown"]["explicitRecords"], 2);
}

#[test]
fn codex_usage_counts_identical_events_from_independent_sessions() {
    let history_root = temp_dir("codex-usage-independent-sessions-history");
    let state_root = temp_dir("codex-usage-independent-sessions-state");
    let event = token_event("2026-07-08T10:00:01Z", (10, 4, 2), (10, 4, 2));
    for (file_name, session_id) in [
        ("first.jsonl", "independent-first"),
        ("second.jsonl", "independent-second"),
    ] {
        fs::write(
            history_root.join(file_name),
            [
                format!(
                    r#"{{"timestamp":"2026-07-08T10:00:00Z","type":"session_meta","payload":{{"id":"{session_id}"}}}}"#
                ),
                event.clone(),
            ]
            .join("\n"),
        )
        .unwrap();
    }

    let report = agent_usage::scan(&scan_params(&history_root, &state_root)).unwrap();
    let history = &report["agents"][0]["history"];
    assert_eq!(history["sessionCount"], 2);
    assert_eq!(history["totalTokens"], 24);
    assert_eq!(history["tokenSourceBreakdown"]["explicitRecords"], 2);
}

#[test]
fn codex_usage_deduplicates_forked_rollout_prefix_before_window_filtering() {
    let history_root = temp_dir("codex-usage-fork-history");
    let state_root = temp_dir("codex-usage-fork-state");
    let parent_meta = r#"{"timestamp":"2026-06-01T10:00:00Z","type":"session_meta","payload":{"id":"parent-session"}}"#;
    let parent_message = r#"{"timestamp":"2026-06-01T10:00:01Z","type":"response_item","payload":{"type":"message","role":"user","content":[{"type":"input_text","text":"shared prefix"}]}}"#;
    let parent_context = r#"{"timestamp":"2026-06-01T10:00:02Z","type":"turn_context","payload":{"model":"gpt-test-codex"}}"#;
    fs::write(
        history_root.join("parent.jsonl"),
        [
            parent_meta.to_string(),
            parent_message.to_string(),
            parent_context.to_string(),
            token_event("2026-06-01T10:00:03Z", (100, 20, 10), (100, 20, 10)),
        ]
        .join("\n"),
    )
    .unwrap();
    fs::write(
        history_root.join("child.jsonl"),
        [
            r#"{"timestamp":"2026-07-10T10:00:00Z","type":"session_meta","payload":{"id":"child-session","forked_from_id":"parent-session"}}"#.to_string(),
            parent_meta.replace("2026-06-01T10:00:00Z", "2026-07-10T10:00:01Z"),
            r#"{"timestamp":"2026-07-10T10:00:02Z","type":"response_item","payload":{"type":"message","id":"fork-added-id","role":"user","content":[{"type":"input_text","text":"shared prefix"}]}}"#.to_string(),
            parent_context.replace("2026-06-01T10:00:02Z", "2026-07-10T10:00:03Z"),
            token_event("2026-07-10T10:00:04Z", (100, 20, 10), (100, 20, 10)),
            r#"{"timestamp":"2026-07-10T10:00:05Z","type":"response_item","payload":{"type":"message","id":"child-message","role":"user","content":[{"type":"input_text","text":"child branch"}]}}"#.to_string(),
            token_event("2026-07-10T10:00:06Z", (106, 20, 14), (6, 0, 4)),
        ]
        .join("\n"),
    )
    .unwrap();

    let mut params = scan_params(&history_root, &state_root);
    params["historyDays"] = json!(1);
    let recent = agent_usage::scan(&params).unwrap();
    assert_eq!(recent["summary"]["totalTokens"], 10);

    params["historyDays"] = json!(365);
    params["forceRefresh"] = json!(false);
    let all = agent_usage::scan(&params).unwrap();
    assert_eq!(all["summary"]["totalTokens"], 120);
}
