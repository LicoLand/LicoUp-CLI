use super::support::*;

#[test]
fn codex_usage_warm_scan_reuses_files_and_append_scan_reads_only_suffix() {
    let history_root = temp_dir("codex-usage-cache-history");
    let state_root = temp_dir("codex-usage-cache-state");
    let rollout = history_root.join("rollout.jsonl");
    fs::write(
        &rollout,
        [
            r#"{"timestamp":"2026-07-10T10:00:00Z","type":"session_meta","payload":{"id":"session-cache"}}"#.to_string(),
            token_event("2026-07-10T10:00:01Z", (6, 2, 4), (6, 2, 4)),
        ]
        .join("\n")
            + "\n",
    )
    .unwrap();
    let params = scan_params(&history_root, &state_root);

    let cold = agent_usage::scan(&params).unwrap();
    assert_eq!(
        cold["agents"][0]["history"]["scanCache"]["rescannedFiles"],
        1
    );

    let warm = agent_usage::scan(&params).unwrap();
    assert_eq!(warm["agents"][0]["history"]["scanCache"]["reusedFiles"], 1);
    assert_eq!(warm["agents"][0]["history"]["scanCache"]["parsedBytes"], 0);

    let mut file = fs::OpenOptions::new().append(true).open(&rollout).unwrap();
    writeln!(
        file,
        "{}",
        token_event("2026-07-10T10:00:02Z", (8, 2, 5), (2, 0, 1))
    )
    .unwrap();

    let appended = agent_usage::scan(&params).unwrap();
    assert_eq!(appended["agents"][0]["history"]["totalTokens"], 13);
    assert_eq!(
        appended["agents"][0]["history"]["scanCache"]["appendedFiles"],
        1
    );
}

#[test]
fn codex_usage_keeps_finalized_day_immutable_after_source_rewrite() {
    let history_root = temp_dir("codex-usage-rewrite-history");
    let state_root = temp_dir("codex-usage-rewrite-state");
    let rollout = history_root.join("rollout.jsonl");
    fs::write(
        &rollout,
        [
            r#"{"timestamp":"2026-07-08T10:00:00Z","type":"session_meta","payload":{"id":"old-session"}}"#.to_string(),
            token_event("2026-07-08T10:00:01Z", (6, 2, 4), (6, 2, 4)),
        ]
        .join("\n"),
    )
    .unwrap();
    let params = scan_params(&history_root, &state_root);
    let first = agent_usage::scan(&params).unwrap();
    assert_eq!(first["summary"]["totalTokens"], 10);
    let old_size = fs::metadata(&rollout).unwrap().len();

    let padding = "rewritten-content-".repeat(128);
    let replacement = [
        r#"{"timestamp":"2026-07-08T11:00:00Z","type":"session_meta","payload":{"id":"replacement-session"}}"#.to_string(),
        format!(
            r#"{{"timestamp":"2026-07-08T11:00:01Z","type":"response_item","payload":{{"type":"message","role":"user","content":[{{"type":"input_text","text":"{padding}"}}]}}}}"#
        ),
        token_event("2026-07-08T11:00:02Z", (15, 3, 5), (15, 3, 5)),
    ]
    .join("\n");
    fs::write(&rollout, replacement).unwrap();
    assert!(fs::metadata(&rollout).unwrap().len() > old_size);

    let rewritten = agent_usage::scan(&params).unwrap();
    let history = &rewritten["agents"][0]["history"];
    assert_eq!(history["totalTokens"], 10);
    assert_eq!(history["scanCache"]["appendedFiles"], 0);
    assert_eq!(history["scanCache"]["rescannedFiles"], 1);
}

#[test]
fn codex_usage_detects_middle_rewrite_before_append_in_large_file() {
    let history_root = temp_dir("codex-usage-large-middle-rewrite-history");
    let state_root = temp_dir("codex-usage-large-middle-rewrite-state");
    let rollout = history_root.join("rollout.jsonl");
    let metadata = r#"{"timestamp":"2026-07-10T10:00:00Z","type":"session_meta","payload":{"id":"large-rewrite-session"}}"#;
    let padding = json!({
        "timestamp": "2026-07-10T10:00:00Z",
        "type": "turn_context",
        "payload": {
            "model": "gpt-test-codex",
            "padding": "x".repeat(70 * 1024)
        }
    })
    .to_string();
    let original = [
        metadata.to_string(),
        padding.clone(),
        token_event("2026-07-10T10:00:01Z", (10, 4, 2), (10, 4, 2)),
        padding.clone(),
    ]
    .join("\n");
    fs::write(&rollout, original).unwrap();
    let params = scan_params(&history_root, &state_root);
    assert_eq!(
        agent_usage::scan(&params).unwrap()["summary"]["totalTokens"],
        12
    );

    let rewritten = [
        metadata.to_string(),
        padding.clone(),
        token_event("2026-07-10T10:00:01Z", (20, 4, 2), (20, 4, 2)),
        padding,
        r#"{"timestamp":"2026-07-10T10:00:02Z","type":"turn_context","payload":{"model":"gpt-test-codex"}}"#.to_string(),
    ]
    .join("\n");
    fs::write(&rollout, rewritten).unwrap();

    let report = agent_usage::scan(&params).unwrap();
    let history = &report["agents"][0]["history"];
    assert_eq!(history["totalTokens"], 22);
    assert_eq!(history["scanCache"]["appendedFiles"], 0);
    assert_eq!(history["scanCache"]["rescannedFiles"], 1);
}

#[test]
fn codex_usage_force_refresh_detects_equal_metadata_rewrite() {
    let history_root = temp_dir("codex-usage-equal-metadata-rewrite-history");
    let state_root = temp_dir("codex-usage-equal-metadata-rewrite-state");
    let rollout = history_root.join("rollout.jsonl");
    let metadata = r#"{"timestamp":"2026-07-10T10:00:00Z","type":"session_meta","payload":{"id":"equal-metadata-session"}}"#;
    let original = [
        metadata.to_string(),
        token_event("2026-07-10T10:00:01Z", (10, 4, 2), (10, 4, 2)),
    ]
    .join("\n");
    fs::write(&rollout, &original).unwrap();
    let params = scan_params(&history_root, &state_root);
    assert_eq!(
        agent_usage::scan(&params).unwrap()["summary"]["totalTokens"],
        12
    );
    let original_metadata = fs::metadata(&rollout).unwrap();
    let original_modified = original_metadata.modified().unwrap();

    let replacement = [
        metadata.to_string(),
        token_event("2026-07-10T10:00:01Z", (20, 4, 2), (20, 4, 2)),
    ]
    .join("\n");
    assert_eq!(replacement.len(), original.len());
    fs::write(&rollout, replacement).unwrap();
    fs::File::options()
        .write(true)
        .open(&rollout)
        .unwrap()
        .set_times(fs::FileTimes::new().set_modified(original_modified))
        .unwrap();
    assert_eq!(
        fs::metadata(&rollout).unwrap().len(),
        original_metadata.len()
    );

    let report = agent_usage::scan(&params).unwrap();
    let history = &report["agents"][0]["history"];
    assert_eq!(history["totalTokens"], 22);
    assert_eq!(history["scanCache"]["reusedFiles"], 0);
    assert_eq!(history["scanCache"]["rescannedFiles"], 1);
}
