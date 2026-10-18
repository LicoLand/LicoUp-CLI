use super::*;
use crate::domain::skill_hub::{pair_approve_in, pair_request_in, pair_revoke_in};
use serde_json::json;
use std::collections::BTreeMap;
use std::sync::{Arc, Barrier};
use std::{env, fs};
use time::format_description::well_known::Rfc3339;
use uuid::Uuid;

#[test]
fn real_conversation_events_record_sanitized_local_skill_invocations() {
    let store = test_store("runtime-events");
    seed_local_skill(&store, "codex", "review-helper");
    let occurred_at = OffsetDateTime::parse("2026-07-01T01:00:00Z", &Rfc3339).unwrap();
    let receipt = observe_at(
        &store,
        "codex",
        &json!({
            "ok": true,
            "events": [
                {"event": "skill.invoked", "skillId": "review-helper"},
                {"event": "skill.invoked", "skillId": "review-helper"},
                {"event": "skill.invoked", "skillId": "not-installed"},
                {"sessionUpdate": "tool_call", "skillId": "review-helper"}
            ],
            "output": "must never be retained"
        }),
        occurred_at,
    )
    .unwrap();
    assert_eq!(receipt["recordedCount"], 3);

    let ledger = store.read_collection(ledger::COLLECTION).unwrap();
    assert_eq!(
        ledger["items"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|item| item["count"].as_u64())
            .sum::<u64>(),
        3
    );
    assert!(!ledger.to_string().contains("must never be retained"));
}

#[test]
fn report_supports_custom_windows_and_dimensions() {
    let store = test_store("report");
    for (agent, skill) in [("codex", "review"), ("claude-code", "lint")] {
        seed_local_skill(&store, agent, skill);
    }
    for (agent, skill, at) in [
        ("codex", "review", "2026-07-01T00:00:00Z"),
        ("claude-code", "lint", "2026-07-02T00:00:00Z"),
    ] {
        observe_at(
            &store,
            agent,
            &json!({"ok": true, "events": [{"event": "skill.invoked", "skillId": skill}]}),
            OffsetDateTime::parse(at, &Rfc3339).unwrap(),
        )
        .unwrap();
    }
    let result = report(&store, &json!({"from": "2026-07-01", "to": "2026-07-02"})).unwrap();
    assert_eq!(result["totalInvocations"], 2);
    assert_eq!(result["byAgent"].as_array().unwrap().len(), 2);
    let filtered = report(
        &store,
        &json!({
            "from": "2026-07-01",
            "to": "2026-07-02",
            "agent": "codex",
            "skill": "review"
        }),
    )
    .unwrap();
    assert_eq!(filtered["totalInvocations"], 1);
}

#[test]
fn concurrent_runtime_events_do_not_lose_daily_counts() {
    let store = test_store("concurrent");
    seed_local_skill(&store, "codex", "review");
    let barrier = Arc::new(Barrier::new(8));
    let workers = (0..8)
        .map(|_| {
            let store = store.clone();
            let barrier = Arc::clone(&barrier);
            std::thread::spawn(move || {
                barrier.wait();
                observe_at(
                    &store,
                    "codex",
                    &json!({"ok": true, "events": [{"event": "skill.invoked", "skillId": "review"}]}),
                    OffsetDateTime::parse("2026-07-01T00:00:00Z", &Rfc3339).unwrap(),
                )
                .unwrap();
            })
        })
        .collect::<Vec<_>>();
    for worker in workers {
        worker.join().unwrap();
    }
    let result = report(&store, &json!({"from": "2026-07-01", "to": "2026-07-01"})).unwrap();
    assert_eq!(result["totalInvocations"], 8);
}

#[test]
fn day_count_is_selectable_and_bounded() {
    let store = test_store("days");
    let result = report(&store, &json!({"days": 7, "to": "2026-07-16"})).unwrap();
    assert_eq!(result["window"]["from"], "2026-07-10");
    assert_eq!(result["window"]["selectedDays"], 7);
    assert!(report(&store, &json!({"days": 366})).is_err());
}

fn seed_local_skill(store: &ClientStateStore, agent_id: &str, skill_id: &str) {
    pair_request_in(store, &json!({"agent": agent_id})).unwrap();
    pair_approve_in(store, &json!({"agent": agent_id})).unwrap();
    assert!(ledger::is_sanitized_skill_id(skill_id));
}

fn test_store(name: &str) -> ClientStateStore {
    let root = env::temp_dir().join(format!("lico-skill-usage-{name}-{}", Uuid::new_v4()));
    fs::create_dir_all(&root).unwrap();
    ClientStateStore::new(root).unwrap()
}

fn fixture_root(name: &str) -> std::path::PathBuf {
    env::temp_dir().join(format!(
        "lico-skill-usage-fixture-{name}-{}",
        Uuid::new_v4()
    ))
}

fn claude_transcript_lines() -> String {
    concat!(
        "{\"type\":\"user\",\"timestamp\":\"2026-07-14T00:00:01Z\",\"message\":{\"role\":\"user\",\"content\":[{\"type\":\"text\",\"text\":\"synthetic prompt\"}]}}\n",
        "{\"type\":\"assistant\",\"timestamp\":\"2026-07-14T00:00:02Z\",\"message\":{\"content\":[{\"type\":\"tool_use\",\"id\":\"toolu-1\",\"name\":\"Skill\",\"input\":{\"skill\":\"lint-fix\",\"args\":\"sensitive-args\"}}]}}\n",
        "{\"type\":\"assistant\",\"timestamp\":\"2026-07-14T00:00:03Z\",\"message\":{\"content\":[{\"type\":\"tool_use\",\"id\":\"toolu-2\",\"name\":\"Read\",\"input\":{\"file_path\":\"/sensitive/path\"}}]}}\n",
        "{\"type\":\"assistant\",\"timestamp\":\"2026-07-14T00:00:04Z\",\"message\":{\"content\":[{\"type\":\"tool_use\",\"id\":\"toolu-3\",\"name\":\"Skill\",\"input\":{\"skill\":\"lint-fix\"}}]}}\n"
    )
    .to_owned()
}

#[test]
fn backfill_projects_synthetic_kimi_claude_and_codex_transcripts() {
    let store = test_store("backfill-formats");
    let root = fixture_root("formats");

    // Kimi Code wire transcript: `context.append_loop_event` envelopes.
    let kimi_dir = root.join("kimi/agents/session-1");
    fs::create_dir_all(&kimi_dir).unwrap();
    fs::write(
        kimi_dir.join("wire.jsonl"),
        concat!(
            "{\"type\":\"context.append_loop_event\",\"turnId\":\"turn-1\",\"time\":\"2026-07-10T00:00:01Z\",\"event\":{\"type\":\"user.message\",\"message\":{\"role\":\"user\",\"text\":\"synthetic prompt\"}}}\n",
            "{\"type\":\"context.append_loop_event\",\"turnId\":\"turn-1\",\"time\":\"2026-07-10T00:00:02Z\",\"event\":{\"type\":\"tool.call\",\"name\":\"Skill\",\"arguments\":{\"skill\":\"review-helper\",\"prompt\":\"sensitive-never-stored\"}}}\n",
            "{\"type\":\"context.append_loop_event\",\"turnId\":\"turn-1\",\"time\":\"2026-07-10T00:00:03Z\",\"event\":{\"type\":\"tool.call\",\"name\":\"exec\",\"arguments\":{\"command\":\"sensitive-command\"}}}\n",
            "{\"type\":\"context.append_loop_event\",\"turnId\":\"turn-1\",\"time\":\"2026-07-10T00:00:04Z\",\"event\":{\"type\":\"tool.call\",\"name\":\"Skill\",\"arguments\":{\"skill\":\"release-check\"}}}\n"
        ),
    )
    .unwrap();
    let kimi = scan(
        &store,
        &json!({"agent": "kimi-code", "historyRoot": root.join("kimi").to_string_lossy()}),
    )
    .unwrap();
    assert_eq!(kimi["invocationsAdded"], 2);

    // Claude Code transcript: `message.content[]` tool_use blocks.
    let claude_dir = root.join("claude");
    fs::create_dir_all(&claude_dir).unwrap();
    fs::write(claude_dir.join("session.jsonl"), claude_transcript_lines()).unwrap();
    let claude = scan(
        &store,
        &json!({"agent": "claude-code", "historyRoot": claude_dir.to_string_lossy()}),
    )
    .unwrap();
    assert_eq!(claude["invocationsAdded"], 2);

    // Codex rollout transcript: `response_item` function_call payloads with
    // string-encoded arguments.
    let codex_dir = root.join("codex");
    fs::create_dir_all(&codex_dir).unwrap();
    fs::write(
        codex_dir.join("rollout.jsonl"),
        concat!(
            "{\"timestamp\":\"2026-06-03T10:53:43.745Z\",\"type\":\"response_item\",\"payload\":{\"type\":\"message\",\"role\":\"user\",\"content\":[{\"type\":\"input_text\",\"text\":\"synthetic prompt\"}]}}\n",
            "{\"timestamp\":\"2026-06-03T10:53:55.000Z\",\"type\":\"response_item\",\"payload\":{\"type\":\"function_call\",\"name\":\"Skill\",\"call_id\":\"call-1\",\"arguments\":\"{\\\"skill\\\":\\\"repo-audit\\\",\\\"cmd\\\":\\\"sensitive-command\\\"}\"}}\n",
            "{\"timestamp\":\"2026-06-03T10:53:56.000Z\",\"type\":\"response_item\",\"payload\":{\"type\":\"function_call_output\",\"call_id\":\"call-1\",\"output\":\"sensitive-output\"}}\n",
            "{\"timestamp\":\"2026-06-04T10:53:57.000Z\",\"type\":\"response_item\",\"payload\":{\"type\":\"function_call\",\"name\":\"skill:deploy-check\",\"call_id\":\"call-2\",\"arguments\":\"{}\"}}\n"
        ),
    )
    .unwrap();
    let codex = scan(
        &store,
        &json!({"agent": "codex", "historyRoot": codex_dir.to_string_lossy()}),
    )
    .unwrap();
    assert_eq!(codex["invocationsAdded"], 2);

    // No pairing state was seeded: the backfill gate accepts
    // any locally discovered agent and any sanitized skill id.
    let kimi_report = report(
        &store,
        &json!({"agent": "kimi-code", "from": "2026-07-10", "to": "2026-07-10"}),
    )
    .unwrap();
    assert_eq!(kimi_report["totalInvocations"], 2);
    let codex_report = report(
        &store,
        &json!({"agent": "codex", "from": "2026-06-03", "to": "2026-06-04"}),
    )
    .unwrap();
    assert_eq!(codex_report["totalInvocations"], 2);
    assert_eq!(codex_report["byDay"].as_array().unwrap().len(), 2);

    // Only aggregate counts and sanitized ids persist; no transcript content,
    // vendor call ids, or paths leak into the ledger.
    let ledger_text = store
        .read_collection(ledger::COLLECTION)
        .unwrap()
        .to_string();
    for sensitive in [
        "synthetic prompt",
        "sensitive",
        "call-1",
        "turn-1",
        "/sensitive/path",
    ] {
        assert!(
            !ledger_text.contains(sensitive),
            "ledger retained {sensitive}"
        );
    }
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn backfill_rescan_is_idempotent_and_append_resumes_from_watermark() {
    use std::io::Write;

    let store = test_store("backfill-idempotent");
    let root = fixture_root("idempotent");
    fs::create_dir_all(&root).unwrap();
    let file = root.join("session.jsonl");
    fs::write(&file, claude_transcript_lines()).unwrap();
    let params = json!({"agent": "claude-code", "historyRoot": root.to_string_lossy()});

    let first = scan(&store, &params).unwrap();
    assert_eq!(first["invocationsAdded"], 2);
    assert_eq!(first["filesScanned"], 1);

    let second = scan(&store, &params).unwrap();
    assert_eq!(second["invocationsAdded"], 0);
    assert_eq!(second["filesScanned"], 0);
    assert_eq!(second["filesUnchanged"], 1);

    fs::OpenOptions::new()
        .append(true)
        .open(&file)
        .unwrap()
        .write_all(
            b"{\"type\":\"assistant\",\"timestamp\":\"2026-07-14T00:00:05Z\",\"message\":{\"content\":[{\"type\":\"tool_use\",\"id\":\"toolu-4\",\"name\":\"Skill\",\"input\":{\"skill\":\"lint-fix\"}}]}}\n",
        )
        .unwrap();
    let third = scan(&store, &params).unwrap();
    assert_eq!(third["invocationsAdded"], 1);
    assert_eq!(third["agents"][0]["filesAppended"], 1);

    let windowed = report(
        &store,
        &json!({"agent": "claude-code", "from": "2026-07-14", "to": "2026-07-14"}),
    )
    .unwrap();
    assert_eq!(windowed["totalInvocations"], 3);
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn backfill_deduplicates_across_overlapping_full_rescans() {
    let store = test_store("backfill-dedup");
    let root = fixture_root("dedup");
    fs::create_dir_all(&root).unwrap();
    fs::write(root.join("session.jsonl"), claude_transcript_lines()).unwrap();
    let params = json!({"agent": "claude-code", "historyRoot": root.to_string_lossy()});

    let first = scan(&store, &params).unwrap();
    assert_eq!(first["invocationsAdded"], 2);

    // A forced full re-scan re-finds both invocations but persisted digests
    // suppress every duplicate.
    let mut refreshed_params = params.clone();
    refreshed_params["forceRefresh"] = json!(true);
    let refreshed = scan(&store, &refreshed_params).unwrap();
    assert_eq!(refreshed["invocationsFound"], 2);
    assert_eq!(refreshed["invocationsAdded"], 0);
    assert_eq!(refreshed["invocationsDuplicate"], 2);
    assert_eq!(refreshed["agents"][0]["filesReplaced"], 1);

    // A rewrite with identical content (guard mismatch on mtime, same vendor
    // call ids) is also fully deduplicated.
    fs::write(root.join("session.jsonl"), claude_transcript_lines()).unwrap();
    let rewritten = scan(&store, &params).unwrap();
    assert_eq!(rewritten["invocationsAdded"], 0);
    assert_eq!(rewritten["invocationsDuplicate"], 2);

    let windowed = report(
        &store,
        &json!({"agent": "claude-code", "from": "2026-07-14", "to": "2026-07-14"}),
    )
    .unwrap();
    assert_eq!(windowed["totalInvocations"], 2);
    assert_eq!(windowed["allTimeInvocations"], 2);
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn runtime_gate_stays_enforced_while_backfill_gate_is_relaxed() {
    let store = test_store("gate-split");
    pair_request_in(&store, &json!({"agent": "codex"})).unwrap();
    pair_approve_in(&store, &json!({"agent": "codex"})).unwrap();
    pair_revoke_in(&store, &json!({"agent": "codex"})).unwrap();

    // A revoked pairing still blocks runtime live events.
    let blocked = observe_at(
        &store,
        "codex",
        &json!({"ok": true, "events": [{"event": "skill.invoked", "skillId": "review"}]}),
        OffsetDateTime::parse("2026-07-01T00:00:00Z", &Rfc3339).unwrap(),
    );
    assert!(blocked.is_err());

    // The backfill record path needs no pairing and no installer record; it
    // only requires well-formed sanitized skill ids.
    let counts = BTreeMap::from([
        ("review".to_string(), 2_u64),
        ("invalid.skill.id".to_string(), 5_u64),
    ]);
    let receipt = ledger::record_counts(
        &store,
        "codex",
        counts,
        OffsetDateTime::parse("2026-07-01T00:00:00Z", &Rfc3339).unwrap(),
        ledger::RecordSource::Backfill,
    )
    .unwrap();
    assert_eq!(receipt["recordedCount"], 2);
    assert_eq!(receipt["source"], "history-backfill-scan");
    let windowed = report(
        &store,
        &json!({"agent": "codex", "from": "2026-07-01", "to": "2026-07-01"}),
    )
    .unwrap();
    assert_eq!(windowed["totalInvocations"], 2);
}

#[test]
fn runtime_recording_preserves_backfill_watermark_items() {
    let store = test_store("watermark-preserved");
    let root = fixture_root("watermark-preserved");
    fs::create_dir_all(&root).unwrap();
    fs::write(root.join("session.jsonl"), claude_transcript_lines()).unwrap();
    let params = json!({"agent": "claude-code", "historyRoot": root.to_string_lossy()});
    assert_eq!(scan(&store, &params).unwrap()["invocationsAdded"], 2);

    seed_local_skill(&store, "claude-code", "lint-fix");
    observe_at(
        &store,
        "claude-code",
        &json!({"ok": true, "events": [{"event": "skill.invoked", "skillId": "lint-fix"}]}),
        OffsetDateTime::parse("2026-07-14T01:00:00Z", &Rfc3339).unwrap(),
    )
    .unwrap();

    let document = store.read_collection(ledger::COLLECTION).unwrap();
    let items = document["items"].as_array().unwrap();
    assert!(
        items
            .iter()
            .any(|item| item["kind"] == "skill-usage-scan-source")
    );
    assert!(items.iter().any(|item| item["kind"] == "skill-usage-day"));

    // The surviving watermark still makes the next scan free.
    let rescan = scan(&store, &params).unwrap();
    assert_eq!(rescan["filesUnchanged"], 1);
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn report_adds_all_time_totals_beside_the_bounded_window() {
    let store = test_store("report-totals");
    let day = |text: &str| OffsetDateTime::parse(text, &Rfc3339).unwrap();
    ledger::record_counts(
        &store,
        "codex",
        BTreeMap::from([("skill-a".to_string(), 2_u64)]),
        day("2020-01-01T00:00:00Z"),
        ledger::RecordSource::Backfill,
    )
    .unwrap();
    ledger::record_counts(
        &store,
        "codex",
        BTreeMap::from([
            ("skill-a".to_string(), 1_u64),
            ("skill-b".to_string(), 3_u64),
        ]),
        day("2026-07-15T00:00:00Z"),
        ledger::RecordSource::Backfill,
    )
    .unwrap();
    ledger::record_counts(
        &store,
        "claude-code",
        BTreeMap::from([("skill-a".to_string(), 4_u64)]),
        day("2026-07-15T00:00:00Z"),
        ledger::RecordSource::Backfill,
    )
    .unwrap();

    let windowed = report(&store, &json!({"from": "2026-07-15", "to": "2026-07-15"})).unwrap();
    assert_eq!(windowed["totalInvocations"], 8);
    assert_eq!(windowed["allTimeInvocations"], 10);
    let totals = windowed["totalsBySkill"].as_array().unwrap();
    assert_eq!(totals.len(), 2);
    assert_eq!(totals[0], json!({"skillId": "skill-a", "count": 7}));
    assert_eq!(totals[1], json!({"skillId": "skill-b", "count": 3}));

    // All-time totals honor the same agent/skill filters as the window.
    let filtered = report(
        &store,
        &json!({"from": "2026-07-15", "to": "2026-07-15", "agent": "codex", "skill": "skill-a"}),
    )
    .unwrap();
    assert_eq!(filtered["totalInvocations"], 1);
    assert_eq!(filtered["allTimeInvocations"], 3);

    // A window that excludes every record still reports all-time totals.
    let empty_window = report(&store, &json!({"days": 1, "to": "2026-07-16"})).unwrap();
    assert_eq!(empty_window["totalInvocations"], 0);
    assert_eq!(empty_window["allTimeInvocations"], 10);
}
