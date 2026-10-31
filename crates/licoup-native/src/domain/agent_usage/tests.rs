use super::{report, scan};
use rusqlite::Connection;
use serde_json::{Value, json};
use std::fs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};
use time::{OffsetDateTime, format_description::well_known::Rfc3339};

fn install_host() {
    let _ = super::host::install();
}

fn temp_root(label: &str) -> PathBuf {
    // The clock can repeat between rapid calls; the process-local sequence
    // keeps parallel tests on disjoint roots.
    static SEQUENCE: AtomicU64 = AtomicU64::new(0);
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    std::env::temp_dir().join(format!(
        "lico-agent-usage-{label}-{nonce}-{}",
        SEQUENCE.fetch_add(1, Ordering::Relaxed)
    ))
}

fn epoch(value: &str) -> f64 {
    OffsetDateTime::parse(value, &Rfc3339)
        .unwrap()
        .unix_timestamp() as f64
}

fn write_usage_history(root: &PathBuf) {
    fs::create_dir_all(root).unwrap();
    fs::write(
        root.join("session.json"),
        json!({
            "id": "local-session",
            "model": "local-model-a",
            "messages": [
                {
                    "role": "user",
                    "text": "private-local-prompt-canary",
                    "createdAt": "2026-07-01T10:00:00Z"
                },
                {
                    "role": "assistant",
                    "text": "private-local-response-canary",
                    "createdAt": "2026-07-01T10:00:01Z",
                    "model": "local-model-a",
                    "usage": {
                        "prompt_tokens": 11,
                        "cached_input_tokens": 4,
                        "completion_tokens": 7,
                        "total_tokens": 18
                    }
                }
            ]
        })
        .to_string(),
    )
    .unwrap();
}

#[test]
fn command_scan_keeps_schema_modes_dimensions_and_privacy_boundary() {
    install_host();
    let history_root = temp_root("command-history");
    let state_root = temp_root("command-state");
    write_usage_history(&history_root);
    let result = scan(&json!({
        "agent": "opencode",
        "root": history_root.to_string_lossy(),
        "stateRoot": state_root.to_string_lossy(),
        "now": "2026-07-15T12:00:00Z"
    }))
    .unwrap();

    assert_eq!(result["schemaVersion"], 7);
    assert_eq!(result["mode"], "local-token-usage");
    assert_eq!(
        result["tokenSourceMode"],
        "native-metadata-first-incremental"
    );
    assert_eq!(result["window"]["days"], 30);
    assert_eq!(result["agents"][0]["agentId"], "opencode");
    assert_eq!(result["agents"][0]["history"]["totalTokens"], 18);
    let model_total = result["agents"][0]["history"]["dailyUsage"][0]["modelUsage"]
        .as_object()
        .map(|usage| usage.values().filter_map(Value::as_u64).sum::<u64>())
        .unwrap_or(0);
    assert_eq!(model_total, 18);
    let serialized = serde_json::to_string(&result).unwrap();
    assert!(!serialized.contains("private-local-prompt-canary"));
    assert!(!serialized.contains("private-local-response-canary"));
    assert!(!serialized.contains(&history_root.to_string_lossy().to_string()));

    fs::remove_dir_all(history_root).unwrap();
    fs::remove_dir_all(state_root).unwrap();
}

#[test]
fn command_custom_window_and_retained_report_close_independently() {
    install_host();
    let history_root = temp_root("window-history");
    let state_root = temp_root("window-state");
    write_usage_history(&history_root);
    let params = json!({
        "agent": "opencode",
        "root": history_root.to_string_lossy(),
        "stateRoot": state_root.to_string_lossy(),
        "historyDays": 7,
        "now": "2026-07-15T12:00:00Z"
    });
    let result = scan(&params).unwrap();
    assert_eq!(result["summary"]["windowDays"], 7);
    assert_eq!(result["summary"]["windowStart"], "2026-07-09");
    assert_eq!(result["summary"]["totalTokens"], 0);

    let listed = report(&json!({
        "agent": "opencode",
        "stateRoot": state_root.to_string_lossy(),
        "limit": 1
    }))
    .unwrap();
    assert_eq!(listed["schemaVersion"], 7);
    assert_eq!(listed["mode"], "local-token-usage");
    assert_eq!(
        listed["tokenSourceMode"],
        "native-metadata-first-incremental"
    );
    let reports = listed["reports"].as_array().map(Vec::len).unwrap_or(0);
    assert_eq!(reports, 1);

    fs::remove_dir_all(history_root).unwrap();
    fs::remove_dir_all(state_root).unwrap();
}

#[test]
fn command_kimi_code_keeps_exact_turn_and_session_usage_with_model_dimension() {
    install_host();
    let history_root = temp_root("kimi-code-history");
    let wire = history_root.join("work/session/agents/main/wire.jsonl");
    fs::create_dir_all(wire.parent().unwrap()).unwrap();
    fs::write(
        wire,
        [
            r#"{"type":"context.append_message","time":"2026-07-10T10:00:00Z","message":{"role":"user","content":"local prompt covered by explicit turn usage"}}"#,
            r#"{"type":"usage.record","time":"2026-07-10T10:00:01Z","model":"kimi-code-k3-256k","usageScope":"turn","effort":"high","usage":{"inputOther":100,"inputCacheRead":20,"inputCacheCreation":5,"output":30}}"#,
            r#"{"type":"usage.record","time":"2026-07-10T10:00:02Z","model":"kimi-code-k3-256k","usageScope":"session","effort":"max","usage":{"inputOther":40,"inputCacheRead":10,"inputCacheCreation":3,"output":12}}"#,
        ]
        .join("\n"),
    )
    .unwrap();
    let state_root = temp_root("kimi-code-state");
    let registry=crate::domain::model_registry::RegistrySnapshot::from_catalog(json!({"models":{"moonshotai/kimi-k3":{"name":"Kimi K3"}},"providers":{"kimi-for-coding":{"name":"Kimi Code","models":{"k3-256k":{"name":"Kimi Code K3 256K","base_model":"moonshotai/kimi-k3"}}}}})).unwrap();
    let result = crate::domain::model_registry::with_test_snapshot(registry, || {
        scan(&json!({
            "agent": "kimi-code",
            "root": history_root.to_string_lossy(),
            "stateRoot": state_root.to_string_lossy(),
            "now": "2026-07-10T12:00:00Z"
        }))
    })
    .unwrap();
    let history = &result["agents"][0]["history"];
    assert_eq!(history["promptTokens"], 178);
    assert_eq!(history["cachedInputTokens"], 30);
    assert_eq!(history["completionTokens"], 42);
    assert_eq!(history["totalTokens"], 220);
    assert_eq!(history["tokenSourceBreakdown"]["explicitRecords"], 2);
    assert_eq!(
        history["dailyUsage"][0]["modelUsage"]["moonshotai/kimi-k3"],
        220
    );
    assert_eq!(
        history["dailyUsage"][0]["modelTokenUsage"]["moonshotai/kimi-k3"]["cachedInputTokens"],
        30
    );
    assert_eq!(
        history["dailyUsage"][0]["modelTokenUsage"]["moonshotai/kimi-k3"]["variants"]["High"]["totalTokens"],
        155
    );
    assert_eq!(
        history["dailyUsage"][0]["modelTokenUsage"]["moonshotai/kimi-k3"]["variants"]["Max"]["totalTokens"],
        65
    );
    fs::remove_dir_all(history_root).unwrap();
    fs::remove_dir_all(state_root).unwrap();
}

#[test]
fn command_hermes_uses_reconciled_gateway_counters_without_text_estimates() {
    install_host();
    let history_root = temp_root("hermes-gateway-history");
    let state_root = temp_root("hermes-gateway-state");
    fs::create_dir_all(&history_root).unwrap();
    let database = history_root.join("state.db");
    let connection = Connection::open(&database).unwrap();
    connection
        .execute_batch(
            "CREATE TABLE sessions (
                id TEXT PRIMARY KEY,
                source TEXT NOT NULL,
                model TEXT,
                started_at REAL NOT NULL,
                ended_at REAL,
                input_tokens INTEGER NOT NULL DEFAULT 0,
                output_tokens INTEGER NOT NULL DEFAULT 0,
                cache_read_tokens INTEGER NOT NULL DEFAULT 0,
                cache_write_tokens INTEGER NOT NULL DEFAULT 0
            );
            CREATE TABLE messages (
                session_id TEXT NOT NULL,
                timestamp REAL NOT NULL
            );
            CREATE INDEX messages_session_timestamp
                ON messages(session_id, timestamp);
            CREATE TABLE session_model_usage (
                session_id TEXT NOT NULL,
                model TEXT NOT NULL,
                billing_provider TEXT NOT NULL DEFAULT '',
                billing_base_url TEXT NOT NULL DEFAULT '',
                billing_mode TEXT NOT NULL DEFAULT '',
                task TEXT NOT NULL DEFAULT '',
                input_tokens INTEGER NOT NULL DEFAULT 0,
                output_tokens INTEGER NOT NULL DEFAULT 0,
                cache_read_tokens INTEGER NOT NULL DEFAULT 0,
                cache_write_tokens INTEGER NOT NULL DEFAULT 0,
                reasoning_tokens INTEGER NOT NULL DEFAULT 0,
                first_seen REAL,
                last_seen REAL,
                PRIMARY KEY (
                    session_id, model, billing_provider,
                    billing_base_url, billing_mode, task
                )
            );",
        )
        .unwrap();
    connection
        .execute(
            "INSERT INTO sessions VALUES (?1, 'gateway', ?2, ?3, NULL, 25, 12, 4, 1)",
            rusqlite::params![
                "synthetic-session",
                "hermes-test",
                epoch("2026-07-15T09:59:00Z")
            ],
        )
        .unwrap();
    connection
        .execute(
            "INSERT INTO messages VALUES (?1, ?2)",
            rusqlite::params!["synthetic-session", epoch("2026-07-15T10:00:04Z")],
        )
        .unwrap();
    connection
        .execute(
            "INSERT INTO session_model_usage VALUES
                (?1, ?2, '', '', '', '', 10, 5, 2, 0, 1, ?3, ?4),
                (?1, ?2, '', '', '', 'compression', 7, 3, 1, 0, 2, ?5, ?6)",
            rusqlite::params![
                "synthetic-session",
                "hermes-test",
                epoch("2026-07-15T10:00:00Z"),
                epoch("2026-07-15T10:00:01Z"),
                epoch("2026-07-15T10:00:02Z"),
                epoch("2026-07-15T10:00:03Z")
            ],
        )
        .unwrap();
    drop(connection);

    let result = scan(&json!({
        "agent": "hermes",
        "root": history_root.to_string_lossy(),
        "stateRoot": state_root.to_string_lossy(),
        "now": "2026-07-15T12:00:00Z"
    }))
    .unwrap();
    let history = &result["agents"][0]["history"];
    assert_eq!(history["promptTokens"], 30);
    assert_eq!(history["cachedInputTokens"], 4);
    assert_eq!(history["completionTokens"], 12);
    assert_eq!(history["totalTokens"], 42);
    assert_eq!(history["sessionCount"], 1);
    assert_eq!(history["tokenSourceBreakdown"]["explicitRecords"], 2);
    assert_eq!(history["tokenSourceBreakdown"]["estimatedRecords"], 0);
    assert_eq!(history["source"], "hermes-gateway-usage-database");
    assert_eq!(history["dailyUsage"][0]["modelUsage"]["hermes-test"], 42);
    assert_eq!(result["agents"][0]["confidence"], "high");

    fs::remove_dir_all(history_root).unwrap();
    fs::remove_dir_all(state_root).unwrap();
}

#[test]
fn supported_usage_sources_use_product_identity() {
    install_host();
    let labels = crate::domain::agent_usage::supported_agents()
        .iter()
        .map(|agent| (agent.id, agent.label))
        .collect::<std::collections::BTreeMap<_, _>>();

    assert_eq!(labels.get("codex"), Some(&"Codex"));
    assert_eq!(labels.get("kimi-code"), Some(&"Kimi Code"));
    let authoritative =
        crate::domain::agent_catalog::supported_membership(std::iter::empty::<&str>())
            .into_iter()
            .map(|agent| agent.id)
            .collect::<std::collections::BTreeSet<_>>();
    assert!(
        authoritative.is_subset(
            &labels
                .keys()
                .map(|id| id.to_string())
                .collect::<std::collections::BTreeSet<_>>()
        )
    );
    assert!(!labels.contains_key("code"));
    assert!(labels.contains_key("lico-agent"));
    assert!(labels.contains_key("deepseek-harness"));
}

#[test]
fn catalog_agents_without_historical_usage_sources_report_explicit_unavailability() {
    install_host();
    use crate::domain::conversation::source_catalog::usage_adapter_for_agent;
    let unsupported = crate::domain::agent_usage::supported_agents()
        .into_iter()
        .filter(|agent| usage_adapter_for_agent(agent.id).is_none())
        .map(|agent| agent.id)
        .collect::<Vec<_>>();
    assert_eq!(
        unsupported
            .iter()
            .copied()
            .collect::<std::collections::BTreeSet<_>>(),
        ["grok", "command-code"].into_iter().collect()
    );
    let state_root = temp_root("unavailable-source-state");
    for agent in unsupported {
        let result=scan(&json!({"agent":agent,"stateRoot":state_root.to_string_lossy(),"now":"2026-07-15T12:00:00Z"})).unwrap();
        assert_eq!(result["agents"][0]["agentId"], agent);
        assert_eq!(
            result["agents"][0]["history"]["source"],
            "native-usage-source-unavailable"
        );
        assert_eq!(result["agents"][0]["confidence"], "unavailable");
        assert_eq!(
            result["agents"][0]["sources"]["skipped"][0]["code"],
            "native_usage_source_unavailable"
        );
    }
    fs::remove_dir_all(state_root).unwrap();
}

#[test]
fn lico_agent_native_transcript_preserves_exact_model_and_request_effort_without_duplicate_ledger()
{
    install_host();
    let history_root = temp_root("lico-history");
    let state_root = temp_root("lico-state");
    fs::create_dir_all(&history_root).unwrap();
    fs::write(history_root.join("fixture.jsonl"),[
        json!({"type":"session","id":"synthetic-session"}),
        json!({"type":"message","role":"assistant","text":"synthetic response","timestamp":"2026-07-15T10:00:00Z","model":"fixture-model","reasoning_effort":"high","usage":{"input_tokens":12,"output_tokens":3}})
    ].iter().map(Value::to_string).collect::<Vec<_>>().join("\n")+"\n").unwrap();
    let result=scan(&json!({"agent":"lico-agent","root":history_root.to_string_lossy(),"stateRoot":state_root.to_string_lossy(),"now":"2026-07-15T12:00:00Z"})).unwrap();
    assert_eq!(result["agents"][0]["history"]["totalTokens"], 15);
    assert_eq!(
        result["agents"][0]["history"]["dailyUsage"][0]["modelTokenUsage"]["fixture-model"]["variants"]
            ["High"]["totalTokens"],
        15
    );
    fs::remove_dir_all(history_root).unwrap();
    fs::remove_dir_all(state_root).unwrap();
}

/// A store this client cannot open stays a reported refusal.
///
/// The metering port is composed before a scan settles anything, so the
/// composition itself must not turn an unopenable store into a panic, a
/// half-installed host, or a silent success: the install refuses with the
/// ledger owner's recoverable code, the registry keeps no host for that home,
/// the projection has nothing to read (which is what the report renders as
/// `available: false`), and a later install over a healthy home still works.
/// The verb's own answer for that home is the ledger refusal it already
/// reported, not a panic and not a fabricated empty ledger.
#[test]
fn an_unopenable_metering_store_is_a_reported_refusal() {
    install_host();
    let broken = temp_root("unopenable-store");
    fs::create_dir_all(&broken).unwrap();
    // `agent-usage` as a regular file is exactly "the ledger store cannot be
    // created here".
    fs::write(broken.join("agent-usage"), b"not a ledger directory").unwrap();
    let broken_params = json!({"stateRoot": broken.to_string_lossy()});

    let refusal = super::install(&broken_params).expect_err("an unopenable store is refused");
    assert_eq!(refusal.code, "usage_ledger_store_unavailable");
    assert!(
        refusal.retryable,
        "a store that is unavailable now is retried once it recovers"
    );
    assert!(
        super::source_host(&broken_params).is_err(),
        "a refused install leaves no host behind"
    );
    assert!(
        super::held_metering(&broken_params).is_err(),
        "the projection reports the store it could not read"
    );

    // The refusal is scoped to the home that failed: a healthy home still
    // composes, and the verb reports the ledger refusal rather than panicking.
    let healthy = temp_root("recovered-store");
    let healthy_params = json!({
        "stateRoot": healthy.to_string_lossy(),
        "agent": "codex"
    });
    super::install(&healthy_params).expect("a healthy home still composes");
    assert!(super::source_host(&healthy_params).is_ok());
    let error = scan(&json!({
        "agent": "codex",
        "stateRoot": broken.to_string_lossy(),
        "now": "2026-07-15T12:00:00Z"
    }))
    .expect_err("the verb reports the store it cannot open");
    assert!(
        error.to_string().contains("usage_ledger_store_unavailable"),
        "the refusal surfaces through the verb's error channel: {error}"
    );

    fs::remove_dir_all(broken).unwrap();
    fs::remove_dir_all(healthy).unwrap();
}
