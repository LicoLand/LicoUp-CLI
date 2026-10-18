use serde_json::{Value, json};
use std::fs;

use super::support::TestRoot;
use crate::activity::retention_probe;
use crate::policy::{ACTIVITY_DIR, ACTIVITY_FILE, MAX_ACTIVITY_EVENT_BYTES};

fn synthetic_record(index: usize) -> String {
    serde_json::to_string(&json!({
        "schemaVersion": crate::policy::STATE_SCHEMA_VERSION,
        "eventId": format!("activity-fixture-{index}"),
        "type": "task.finished",
        "target": "codex",
        "createdAt": format!("fixture-{index}"),
        "payload": {"index": index, "note": format!("fixture record {index}")}
    }))
    .unwrap()
}

fn activity_path(store: &crate::ClientStateStore) -> std::path::PathBuf {
    store.root().join(ACTIVITY_DIR).join(ACTIVITY_FILE)
}

fn write_hardened_activity(store: &crate::ClientStateStore, content: &str) -> std::path::PathBuf {
    let path = activity_path(store);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(&path, content).unwrap();
    licoup_foundation::platform::file_security::harden_private_path(&path).unwrap();
    path
}

#[test]
fn activity_jsonl_is_filterable_bounded_and_path_redacted() {
    let root = TestRoot::new("activity");
    let store = crate::ClientStateStore::new(root.path().join("state")).unwrap();
    let log = store.activity_log();
    log.append(
        "target.config.applied",
        json!({
            "target": "opencode",
            "configPath": root.path().join("private-config"),
            "accessToken": "activity-token-canary"
        }),
    )
    .unwrap();
    log.append("skill.hidden", json!({"target": "codex"}))
        .unwrap();

    let listed = log
        .list(&json!({"target": "opencode", "limit": 1}))
        .unwrap();
    let events = listed["events"].as_array().unwrap();
    assert_eq!(events.len(), 1);
    assert_eq!(events[0]["type"], "target.config.applied");
    assert_eq!(
        events[0]["payload"]["configPath"],
        crate::policy::REDACTED_LOCAL_PATH
    );
    assert_eq!(
        events[0]["payload"]["accessToken"],
        crate::policy::REDACTED_SECRET
    );
    assert_eq!(listed["path"], "activity/activity.jsonl");
    assert!(!listed.to_string().contains(root.path().to_str().unwrap()));

    let raw = fs::read_to_string(
        store
            .root()
            .join(crate::policy::ACTIVITY_DIR)
            .join(crate::policy::ACTIVITY_FILE),
    )
    .unwrap();
    assert!(
        raw.lines()
            .all(|line| serde_json::from_str::<Value>(line).is_ok())
    );
    assert!(!raw.contains("activity-token-canary"));
}

#[test]
fn activity_limit_keeps_only_the_latest_matching_events() {
    let root = TestRoot::new("activity-limit");
    let store = crate::ClientStateStore::new(root.path().join("state")).unwrap();
    let log = store.activity_log();
    for index in 0..4 {
        log.append("task.finished", json!({"target": "codex", "index": index}))
            .unwrap();
    }

    let listed = log.list(&json!({"limit": 2})).unwrap();
    assert_eq!(listed["events"].as_array().unwrap().len(), 2);
    assert_eq!(listed["events"][0]["payload"]["index"], 2);
    assert_eq!(listed["events"][1]["payload"]["index"], 3);
}

#[test]
fn activity_rejects_unbounded_or_path_shaped_event_types() {
    let root = TestRoot::new("activity-type");
    let store = crate::ClientStateStore::new(root.path().join("state")).unwrap();
    let log = store.activity_log();

    assert!(log.append("../../private", json!({})).is_err());
    assert!(
        log.append(
            &"x".repeat(crate::policy::MAX_ACTIVITY_TYPE_BYTES + 1),
            json!({})
        )
        .is_err()
    );
}

#[test]
fn activity_streams_existing_empty_log_without_records() {
    let root = TestRoot::new("activity-empty");
    let store = crate::ClientStateStore::new(root.path().join("state")).unwrap();
    let log = store.activity_log();
    write_hardened_activity(&store, "");

    retention_probe::reset();
    let listed = log.list(&json!({"limit": 8})).unwrap();
    assert_eq!(listed["events"].as_array().unwrap().len(), 0);
    let probe = retention_probe::snapshot();
    assert_eq!(probe.validated_lines, 0);
    assert_eq!(probe.peak_retained, 0);
    assert_eq!(probe.peak_line_buffer_bytes, 0);
}

#[test]
fn activity_large_log_validates_every_line_and_retains_only_latest_window() {
    const TOTAL: usize = 25_000;
    const LIMIT: usize = 16;
    let root = TestRoot::new("activity-large");
    let store = crate::ClientStateStore::new(root.path().join("state")).unwrap();
    let log = store.activity_log();
    let records = (0..TOTAL).map(synthetic_record).collect::<Vec<_>>();
    let file_path = write_hardened_activity(&store, &format!("{}\n", records.join("\n")));
    let file_bytes = fs::metadata(&file_path).unwrap().len() as usize;

    retention_probe::reset();
    let listed = log.list(&json!({"limit": LIMIT})).unwrap();
    let events = listed["events"].as_array().unwrap();
    assert_eq!(events.len(), LIMIT);
    for (offset, event) in events.iter().enumerate() {
        assert_eq!(event["type"], "task.finished");
        assert_eq!(event["payload"]["index"], TOTAL - LIMIT + offset);
        assert_eq!(
            event["payload"]["note"],
            format!("fixture record {}", TOTAL - LIMIT + offset)
        );
    }
    let probe = retention_probe::snapshot();
    assert_eq!(probe.validated_lines, TOTAL);
    assert!(probe.peak_retained <= LIMIT);
    assert!(probe.peak_line_buffer_bytes <= MAX_ACTIVITY_EVENT_BYTES);
    assert!(probe.peak_line_buffer_bytes < file_bytes);

    retention_probe::reset();
    let zero = log.list(&json!({"limit": 0})).unwrap();
    assert_eq!(zero["events"].as_array().unwrap().len(), 0);
    let zero_probe = retention_probe::snapshot();
    assert_eq!(zero_probe.validated_lines, TOTAL);
    assert_eq!(zero_probe.peak_retained, 0);
}

#[test]
fn activity_accepts_carriage_return_line_endings() {
    let root = TestRoot::new("activity-crlf");
    let store = crate::ClientStateStore::new(root.path().join("state")).unwrap();
    let log = store.activity_log();
    let records = (0..3).map(synthetic_record).collect::<Vec<_>>();
    write_hardened_activity(&store, &format!("{}\r\n", records.join("\r\n")));

    let listed = log.list(&json!({"limit": 8})).unwrap();
    let events = listed["events"].as_array().unwrap();
    assert_eq!(events.len(), 3);
    for (index, event) in events.iter().enumerate() {
        assert_eq!(event["payload"]["index"], index);
        assert_eq!(event["payload"]["note"], format!("fixture record {index}"));
    }
}

#[test]
fn activity_accepts_trailing_line_without_newline() {
    let root = TestRoot::new("activity-trailing");
    let store = crate::ClientStateStore::new(root.path().join("state")).unwrap();
    let log = store.activity_log();
    let records = (0..3).map(synthetic_record).collect::<Vec<_>>();
    write_hardened_activity(&store, &records.join("\n"));

    let listed = log.list(&json!({"limit": 8})).unwrap();
    let events = listed["events"].as_array().unwrap();
    assert_eq!(events.len(), 3);
    assert_eq!(events[2]["payload"]["index"], 2);
}

#[test]
fn activity_preserves_multibyte_payloads() {
    let root = TestRoot::new("activity-multibyte");
    let store = crate::ClientStateStore::new(root.path().join("state")).unwrap();
    let log = store.activity_log();
    let note = "中文活动记录 · テスト · 🚀";
    log.append("task.finished", json!({"target": "codex", "note": note}))
        .unwrap();

    let listed = log.list(&json!({"limit": 8})).unwrap();
    let events = listed["events"].as_array().unwrap();
    assert_eq!(events.len(), 1);
    assert_eq!(events[0]["payload"]["note"], note);
    let raw = fs::read_to_string(activity_path(&store)).unwrap();
    assert!(raw.contains(note));
}

#[test]
fn activity_fails_on_malformed_old_record_before_valid_records() {
    let root = TestRoot::new("activity-malformed");
    let store = crate::ClientStateStore::new(root.path().join("state")).unwrap();
    let log = store.activity_log();
    let content = format!("not-json\n{}\n{}", synthetic_record(1), synthetic_record(2));
    write_hardened_activity(&store, &content);

    assert!(log.list(&json!({"limit": 8})).is_err());
}

#[test]
fn activity_rejects_oversized_line_with_bounded_error() {
    let root = TestRoot::new("activity-oversized");
    let store = crate::ClientStateStore::new(root.path().join("state")).unwrap();
    let log = store.activity_log();
    let oversized = format!(
        r#"{{"type":"task.finished","target":"codex","payload":{{"blob":"{}"}}}}"#,
        "x".repeat(MAX_ACTIVITY_EVENT_BYTES)
    );
    write_hardened_activity(&store, &oversized);

    let error = log.list(&json!({"limit": 8})).unwrap_err();
    assert!(
        error
            .to_string()
            .contains("activity event exceeds its bounded size")
    );
}
