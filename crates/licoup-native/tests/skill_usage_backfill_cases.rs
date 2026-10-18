//! End-to-end skill-usage backfill acceptance over synthetic native history
//! fixtures, driven through the public native facade.

use licoup_foundation::platform::paths::set_portable_data_dir_override;
use licoup_native::domain::skill_hub;
use serde_json::json;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

struct PortableDirGuard(Option<PathBuf>);

impl PortableDirGuard {
    fn set(path: &Path) -> Self {
        Self(set_portable_data_dir_override(Some(path.to_path_buf())))
    }
}

impl Drop for PortableDirGuard {
    fn drop(&mut self) {
        set_portable_data_dir_override(self.0.take());
    }
}

fn temp_dir(label: &str) -> PathBuf {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let dir = std::env::temp_dir().join(format!(
        "lico-skill-usage-backfill-cases-{label}-{}-{nonce}",
        std::process::id()
    ));
    fs::create_dir_all(&dir).unwrap();
    dir
}

fn claude_fixture(root: &Path) {
    fs::write(
        root.join("session.jsonl"),
        concat!(
            "{\"type\":\"assistant\",\"timestamp\":\"2026-07-14T00:00:02Z\",\"message\":{\"content\":[{\"type\":\"tool_use\",\"id\":\"toolu-1\",\"name\":\"Skill\",\"input\":{\"skill\":\"lint-fix\"}}]}}\n",
            "{\"type\":\"assistant\",\"timestamp\":\"2026-07-14T00:00:04Z\",\"message\":{\"content\":[{\"type\":\"tool_use\",\"id\":\"toolu-2\",\"name\":\"Skill\",\"input\":{\"skill\":\"lint-fix\"}}]}}\n"
        ),
    )
    .unwrap();
}

#[test]
fn facade_scan_is_idempotent_and_report_exposes_all_time_totals() {
    let dir = temp_dir("round-trip");
    let history = dir.join("claude-history");
    fs::create_dir_all(&history).unwrap();
    claude_fixture(&history);
    let _portable = PortableDirGuard::set(&dir);

    let scan_params = || json!({"agent": "claude-code", "historyRoot": history.to_string_lossy()});
    let first = skill_hub::skill_usage_scan(&scan_params()).unwrap();
    assert_eq!(first["ok"], true);
    assert_eq!(first["mode"], "local-skill-usage");
    assert_eq!(first["invocationsAdded"], 2);
    assert_eq!(first["filesScanned"], 1);
    assert_eq!(
        first["watermark"],
        json!({
            "collection": "skill-usage",
            "sourceKind": "skill-usage-scan-source",
            "sourcesTracked": 1
        })
    );
    // Scan output carries only aggregate counts and sanitized identifiers.
    let scan_text = first.to_string();
    assert!(!scan_text.contains(&history.to_string_lossy().to_string()));

    let second = skill_hub::skill_usage_scan(&scan_params()).unwrap();
    assert_eq!(second["invocationsAdded"], 0);
    assert_eq!(second["filesUnchanged"], 1);

    // Append-only growth resumes from the persisted watermark.
    fs::OpenOptions::new()
        .append(true)
        .open(history.join("session.jsonl"))
        .unwrap()
        .write_all(
            b"{\"type\":\"assistant\",\"timestamp\":\"2026-07-15T00:00:05Z\",\"message\":{\"content\":[{\"type\":\"tool_use\",\"id\":\"toolu-3\",\"name\":\"Skill\",\"input\":{\"skill\":\"release-check\"}}]}}\n",
        )
        .unwrap();
    let third = skill_hub::skill_usage_scan(&scan_params()).unwrap();
    assert_eq!(third["invocationsAdded"], 1);
    assert_eq!(third["agents"][0]["filesAppended"], 1);

    // A forced full re-scan dedups against persisted invocation digests.
    let refreshed = skill_hub::skill_usage_scan(&json!({
        "agent": "claude-code",
        "historyRoot": history.to_string_lossy(),
        "forceRefresh": true
    }))
    .unwrap();
    assert_eq!(refreshed["invocationsFound"], 3);
    assert_eq!(refreshed["invocationsAdded"], 0);
    assert_eq!(refreshed["invocationsDuplicate"], 3);

    let report = skill_hub::skill_usage_report(&json!({
        "agent": "claude-code",
        "from": "2026-07-14",
        "to": "2026-07-15"
    }))
    .unwrap();
    assert_eq!(report["totalInvocations"], 3);
    assert_eq!(report["allTimeInvocations"], 3);
    assert_eq!(
        report["totalsBySkill"].as_array().unwrap(),
        &vec![
            json!({"skillId": "lint-fix", "count": 2}),
            json!({"skillId": "release-check", "count": 1})
        ]
    );
    // The day before the fixture contributes nothing to the window but the
    // all-time totals stay stable.
    let older_window = skill_hub::skill_usage_report(&json!({
        "agent": "claude-code",
        "from": "2026-07-13",
        "to": "2026-07-13"
    }))
    .unwrap();
    assert_eq!(older_window["totalInvocations"], 0);
    assert_eq!(older_window["allTimeInvocations"], 3);

    fs::remove_dir_all(dir).unwrap();
}

#[test]
fn facade_runtime_gate_is_unchanged_by_backfill_relaxation() {
    let dir = temp_dir("gate");
    let history = dir.join("codex-history");
    fs::create_dir_all(&history).unwrap();
    fs::write(
        history.join("rollout.jsonl"),
        "{\"timestamp\":\"2026-06-03T10:53:55.000Z\",\"type\":\"response_item\",\"payload\":{\"type\":\"function_call\",\"name\":\"Skill\",\"call_id\":\"call-1\",\"arguments\":\"{\\\"skill\\\":\\\"repo-audit\\\"}\"}}\n",
    )
    .unwrap();
    let _portable = PortableDirGuard::set(&dir);

    // Backfill records counts for an agent with no pairing.
    let scan = skill_hub::skill_usage_scan(&json!({
        "agent": "codex",
        "historyRoot": history.to_string_lossy()
    }))
    .unwrap();
    assert_eq!(scan["invocationsAdded"], 1);

    // Backfill does not relax the runtime pairing gate. Establish then revoke
    // the pairing explicitly so this assertion does not depend on implicit
    // first-observation approval.
    let paired = skill_hub::pair_request(&json!({"agent": "codex"})).unwrap();
    assert_eq!(paired["status"], "approved");
    let revoked = skill_hub::pair_revoke(&json!({"agent": "codex"})).unwrap();
    assert_eq!(revoked["status"], "revoked");
    let blocked = skill_hub::observe_agent_skill_invocations(
        "codex",
        &json!({"ok": true, "events": [{"event": "skill.invoked", "skillId": "repo-audit"}]}),
    )
    .unwrap_err();
    assert_eq!(
        blocked.to_string(),
        "runtime skill usage requires an approved agent pairing"
    );

    let report = skill_hub::skill_usage_report(&json!({
        "agent": "codex",
        "from": "2026-06-03",
        "to": "2026-06-03"
    }))
    .unwrap();
    assert_eq!(report["totalInvocations"], 1);
    assert_eq!(report["allTimeInvocations"], 1);

    fs::remove_dir_all(dir).unwrap();
}
