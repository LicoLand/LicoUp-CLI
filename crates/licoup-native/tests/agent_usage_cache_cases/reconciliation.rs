use super::support::*;

#[test]
fn codex_usage_reconciles_subsets_duplicates_and_divergent_totals() {
    let history_root = temp_dir("codex-usage-reconcile-history");
    let state_root = temp_dir("codex-usage-reconcile-state");
    fs::write(
        history_root.join("rollout.jsonl"),
        [
            r#"{"timestamp":"2026-07-08T10:00:00Z","type":"session_meta","payload":{"id":"session-reconcile"}}"#.to_string(),
            r#"{"timestamp":"2026-07-08T10:00:01Z","type":"turn_context","payload":{"model":"gpt-test-codex"}}"#.to_string(),
            token_event("2026-07-08T10:00:02Z", (100, 20, 10), (100, 20, 10)),
            token_event("2026-07-08T10:00:03Z", (100, 20, 10), (100, 20, 10)),
            token_event("2026-07-08T10:00:04Z", (160, 40, 16), (60, 20, 6)),
            token_event("2026-07-08T10:00:05Z", (1000, 900, 100), (40, 30, 5)),
            token_event("2026-07-08T10:00:06Z", (1050, 930, 110), (50, 30, 10)),
            token_event("2026-07-08T10:00:07Z", (1050, 930, 110), (50, 30, 10)),
        ]
        .join("\n"),
    )
    .unwrap();

    let report = agent_usage::scan(&scan_params(&history_root, &state_root)).unwrap();
    let history = &report["agents"][0]["history"];
    assert_eq!(history["promptTokens"], 250);
    assert_eq!(history["cachedInputTokens"], 100);
    assert_eq!(history["completionTokens"], 31);
    assert_eq!(history["totalTokens"], 281);
}
