mod hermes;
mod openagent;

use super::models::{CumulativeSnapshot, CumulativeTotals, ParseResult};
use crate::domain::conversation::adapter_dispatch::parse_history_file;
use crate::domain::conversation::history::HistoryScanConfig;
use crate::domain::conversation::source_catalog::HistoryAdapter;
use crate::domain::conversation::usage::extract_token_usage;
use anyhow::{Context, Result};
use hermes::parse_hermes_usage_database;
use licoup_statistics::attribution::{
    estimated_message_usage, message_usage, summarize_sessions, summarize_sessions_exact_only,
};
use licoup_statistics::contract::{HistoryUsageSummary, MessageUsage, text_field};
use licoup_statistics::variant::{
    UsageRequestContext, UsageVariant, compatible_recorded_models, model_label,
    model_with_provider_fallback, prefer_recorded_model,
};
use licoup_statistics::window::UsageWindow;
use openagent::parse_openagent_usage_database;
use rusqlite::{Connection, OpenFlags};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::collections::BTreeSet;
use std::fs;
use std::io::{BufRead, BufReader, Read, Seek, SeekFrom};
use std::path::Path;

pub(super) fn parse_append_source(
    adapter: HistoryAdapter,
    path: &Path,
    start_offset: u64,
    calendar: &UsageWindow,
    has_known_session: bool,
    request_context: UsageRequestContext,
) -> Result<ParseResult> {
    let mut reader =
        BufReader::new(fs::File::open(path).context("native usage append source open failed")?);
    reader.seek(SeekFrom::Start(start_offset))?;
    parse_append_reader(
        adapter,
        reader,
        start_offset,
        calendar,
        has_known_session,
        request_context,
    )
}

pub(super) fn parse_append_prefix(
    adapter: HistoryAdapter,
    path: &Path,
    end_offset: u64,
    calendar: &UsageWindow,
) -> Result<ParseResult> {
    let file = fs::File::open(path).context("native usage prefix source open failed")?;
    anyhow::ensure!(
        file.metadata()?.len() >= end_offset,
        "native usage prefix cursor exceeds source"
    );
    let parsed = parse_append_reader(
        adapter,
        BufReader::new(file.take(end_offset)),
        0,
        calendar,
        true,
        Default::default(),
    )?;
    anyhow::ensure!(
        parsed.parsed_bytes == end_offset,
        "native usage prefix cursor is not a complete record boundary"
    );
    Ok(parsed)
}

fn parse_append_reader(
    adapter: HistoryAdapter,
    mut reader: impl BufRead,
    start_offset: u64,
    calendar: &UsageWindow,
    has_known_session: bool,
    mut request_context: UsageRequestContext,
) -> Result<ParseResult> {
    let mut parsed_bytes = start_offset;
    let mut summary = HistoryUsageSummary::default();
    let mut cumulative_snapshots = Vec::new();
    let mut observed_scopes = BTreeSet::<String>::new();
    let mut saw_unscoped_usage = false;

    loop {
        let line_start = parsed_bytes;
        let mut bytes = Vec::new();
        let read = reader.read_until(b'\n', &mut bytes)?;
        if read == 0 {
            break;
        }
        if bytes.last() != Some(&b'\n') && serde_json::from_slice::<Value>(&bytes).is_err() {
            // A valid final JSON record does not require a trailing newline.
            // Only an actually incomplete tail is left for the next append.
            parsed_bytes = line_start;
            break;
        }
        parsed_bytes = parsed_bytes.saturating_add(read as u64);
        while matches!(bytes.last(), Some(b'\n' | b'\r')) {
            bytes.pop();
        }
        if bytes.is_empty() {
            continue;
        }
        let Ok(event) = serde_json::from_slice::<Value>(&bytes) else {
            continue;
        };
        update_request_context(adapter, &event, &mut request_context);
        if let Some(mut parsed) = explicit_usage_event(adapter, &event, calendar) {
            apply_request_context(&event, &mut parsed.usage, &request_context);
            if let Some(usage_key) = parsed.cumulative_key {
                let session_key = parsed
                    .session_key
                    .unwrap_or_else(|| opaque_scope("source-session"));
                cumulative_snapshots.push(CumulativeSnapshot {
                    usage_key,
                    session_key,
                    model: parsed.usage.model.clone(),
                    variant: parsed.usage.variant.clone(),
                    first_day: parsed.day.clone(),
                    observed_day: parsed.day,
                    totals: CumulativeTotals {
                        prompt: parsed.usage.prompt_tokens,
                        cached: parsed.usage.cached_input_tokens,
                        completion: parsed.usage.completion_tokens,
                    },
                    projects_usage: true,
                });
                continue;
            }
            record_append_usage(
                parsed,
                &mut summary,
                &mut observed_scopes,
                &mut saw_unscoped_usage,
            );
            continue;
        }
        if !matches!(adapter, HistoryAdapter::OpenClaw | HistoryAdapter::Hermes)
            && let Some(mut parsed) = estimated_usage_event(adapter, &event, calendar)
        {
            apply_request_context(&event, &mut parsed.usage, &request_context);
            record_append_usage(
                parsed,
                &mut summary,
                &mut observed_scopes,
                &mut saw_unscoped_usage,
            );
        }
    }

    let mut session_increment = 0;
    if !has_known_session {
        session_increment = observed_scopes.len() as u64;
        if session_increment == 0 && saw_unscoped_usage {
            session_increment = 1;
        }
    }
    summary.session_count = session_increment;
    summary.message_count = summary
        .explicit_records
        .saturating_add(summary.estimated_records);
    Ok(ParseResult {
        summary,
        parsed_bytes,
        cumulative_snapshots,
        session_increment,
        request_context,
    })
}

pub(super) fn parse_snapshot_source(
    adapter: HistoryAdapter,
    path: &Path,
    source_kind: &str,
    metadata: &fs::Metadata,
    calendar: &UsageWindow,
) -> Result<ParseResult> {
    if matches!(adapter, HistoryAdapter::OpenCode | HistoryAdapter::KiloCode)
        && matches!(extension(path).as_str(), "sqlite" | "sqlite3" | "db")
        && let Some(mut parsed) = parse_openagent_usage_database(path, calendar)
    {
        parsed.parsed_bytes = metadata.len();
        parsed.session_increment = parsed.summary.session_count;
        return Ok(parsed);
    }
    if adapter == HistoryAdapter::Hermes
        && matches!(extension(path).as_str(), "sqlite" | "sqlite3" | "db")
    {
        let mut parsed = parse_hermes_usage_database(path, calendar).unwrap_or_default();
        parsed.parsed_bytes = metadata.len();
        parsed.session_increment = parsed.summary.session_count;
        return Ok(parsed);
    }
    let config = HistoryScanConfig::from_params(&json!({"archiveMode": true}));
    let sessions = parse_history_file(adapter, path, source_kind, metadata, config);
    let summary = if matches!(adapter, HistoryAdapter::OpenClaw | HistoryAdapter::Hermes) {
        summarize_sessions_exact_only(&sessions, calendar)
    } else {
        summarize_sessions(&sessions, calendar)
    };
    let session_increment = summary.session_count;
    Ok(ParseResult {
        summary,
        parsed_bytes: metadata.len(),
        session_increment,
        ..ParseResult::default()
    })
}

struct ExplicitUsageEvent {
    usage: MessageUsage,
    day: String,
    session_key: Option<String>,
    cumulative_key: Option<String>,
}

fn record_append_usage(
    parsed: ExplicitUsageEvent,
    summary: &mut HistoryUsageSummary,
    observed_scopes: &mut BTreeSet<String>,
    saw_unscoped_usage: &mut bool,
) {
    if let Some(scope) = parsed.session_key {
        observed_scopes.insert(scope);
    } else {
        *saw_unscoped_usage = true;
    }
    if parsed.usage.total_tokens > 0 {
        summary.add(parsed.usage, Some(parsed.day));
    }
}

fn estimated_usage_event(
    adapter: HistoryAdapter,
    event: &Value,
    calendar: &UsageWindow,
) -> Option<ExplicitUsageEvent> {
    let event_type = event_type(event).to_ascii_lowercase();
    if event_type.contains("quota")
        || event_type.contains("credit")
        || event_type.contains("context_window")
        || event_type.contains("contextwindow")
        || event_type.contains("usage")
        || event_type.contains("metric")
    {
        return None;
    }
    let message = append_message_envelope(adapter, event, &event_type)?;
    let model = model_label(event).or_else(|| model_label(&message));
    let usage = estimated_message_usage(&message, model)?;
    let timestamp = timestamp_value(event)?;
    let day = calendar.date_key(&timestamp)?;
    if !calendar.contains(&day) {
        return None;
    }
    Some(ExplicitUsageEvent {
        usage,
        day,
        session_key: session_scope(event).as_deref().map(opaque_scope),
        cumulative_key: None,
    })
}

fn append_message_envelope(
    adapter: HistoryAdapter,
    event: &Value,
    event_type: &str,
) -> Option<Value> {
    let candidates = [
        event.get("message"),
        event.get("data"),
        event.get("payload"),
        Some(event),
    ];
    let text = candidates.into_iter().flatten().find_map(message_text)?;
    let role = candidates
        .into_iter()
        .flatten()
        .find_map(|candidate| text_field(candidate, &["role", "author"]))
        .or_else(|| {
            if event_type.contains("assistant") || event_type.contains("agent") {
                Some("agent".to_owned())
            } else if event_type.contains("user") || event_type.contains("prompt") {
                Some("user".to_owned())
            } else if adapter == HistoryAdapter::Pi && event_type == "message" {
                event
                    .pointer("/message/role")
                    .and_then(Value::as_str)
                    .map(str::to_owned)
            } else if adapter == HistoryAdapter::LicoAgent && event_type == "message" {
                event.get("role").and_then(Value::as_str).map(str::to_owned)
            } else {
                None
            }
        })
        .unwrap_or_else(|| "user".to_owned());
    Some(json!({
        "role": role,
        "text": text,
        "model": model_label(event)
    }))
}

fn message_text(value: &Value) -> Option<String> {
    if let Some(text) = value
        .as_str()
        .map(str::trim)
        .filter(|text| !text.is_empty())
    {
        return Some(text.to_owned());
    }
    if let Some(items) = value.as_array() {
        let parts = items.iter().filter_map(message_text).collect::<Vec<_>>();
        return (!parts.is_empty()).then(|| parts.join("\n"));
    }
    let object = value.as_object()?;
    for key in [
        "text", "content", "message", "thinking", "prompt", "response",
    ] {
        if let Some(text) = object.get(key).and_then(message_text) {
            return Some(text);
        }
    }
    None
}

fn explicit_usage_event(
    adapter: HistoryAdapter,
    event: &Value,
    calendar: &UsageWindow,
) -> Option<ExplicitUsageEvent> {
    let event_type = event_type(event).to_ascii_lowercase();
    if event_type.contains("quota")
        || event_type.contains("credit")
        || event_type.contains("context_window")
        || event_type.contains("contextwindow")
    {
        return None;
    }
    if adapter == HistoryAdapter::KimiCode
        && !matches!(
            event_type.as_str(),
            "usage.record" | "status.update" | "status_update" | "statusupdate"
        )
        && event.get("token_usage").is_none()
        && event.get("tokenUsage").is_none()
    {
        return None;
    }
    let normalized = extract_token_usage(event)?;
    let model = model_label(event).or_else(|| model_label(&normalized));
    let envelope = json!({
        "model": model,
        "usage": normalized
    });
    let mut usage = message_usage(&envelope, model)?;
    usage.variant = UsageVariant::from_metadata(event);
    let timestamp = timestamp_value(event)?;
    let day = calendar.date_key(&timestamp)?;
    if !calendar.contains(&day) {
        return None;
    }
    let session_scope = session_scope(event);
    let usage_scope = text_field(event, &["usageScope", "usage_scope"])
        .unwrap_or_default()
        .to_ascii_lowercase();
    let kimi_usage_record = adapter == HistoryAdapter::KimiCode && event_type == "usage.record";
    // Legacy Kimi status streams may expose a session-wide snapshot. Current
    // `usage.record` entries are different: both `turn` and `session` scopes
    // are exact, additive model calls, while the scope only controls the live
    // current-turn projection.
    if adapter == HistoryAdapter::KimiCode && usage_scope == "session" && !kimi_usage_record {
        return None;
    }
    let is_cumulative = (usage_scope == "session" && !kimi_usage_record)
        || event.get("total_usage").is_some()
        || event.get("totalUsage").is_some()
        || event.pointer("/conversation/total_usage").is_some()
        || event.pointer("/conversation/totalUsage").is_some()
        || event_type.contains("session_usage");
    let session_key = session_scope.as_deref().map(opaque_scope);
    // A session counter is one cumulative stream even when the actual model
    // or effort changes. Splitting its key would count the running total again.
    let cumulative_key =
        is_cumulative.then(|| opaque_scope(session_scope.as_deref().unwrap_or("source-session")));
    Some(ExplicitUsageEvent {
        usage,
        day,
        session_key,
        cumulative_key,
    })
}

fn read_only_connection(path: &Path) -> Option<Connection> {
    Connection::open_with_flags(
        path,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .ok()
}

fn table_columns(connection: &Connection, table: &str) -> BTreeSet<String> {
    let Ok(mut statement) = connection.prepare(&format!("PRAGMA table_info({table})")) else {
        return BTreeSet::new();
    };
    let Ok(rows) = statement.query_map([], |row| row.get::<_, String>(1)) else {
        return BTreeSet::new();
    };
    rows.flatten().collect()
}

fn sqlite_table_exists(connection: &Connection, table: &str) -> bool {
    connection
        .query_row(
            "SELECT 1 FROM sqlite_master WHERE type='table' AND name=?1 LIMIT 1",
            [table],
            |_| Ok(()),
        )
        .is_ok()
}

fn event_type(value: &Value) -> String {
    [
        value.get("type"),
        value.get("event"),
        value.get("eventType"),
        value.get("event_type"),
        value.pointer("/data/type"),
        value.pointer("/payload/type"),
    ]
    .into_iter()
    .flatten()
    .find_map(Value::as_str)
    .unwrap_or_default()
    .trim()
    .to_owned()
}

fn timestamp_value(value: &Value) -> Option<String> {
    for candidate in [
        value.get("timestamp"),
        value.get("time"),
        value.get("createdAt"),
        value.get("created_at"),
        value.get("date"),
        value.pointer("/data/timestamp"),
        value.pointer("/data/time"),
        value.pointer("/message/timestamp"),
        value.pointer("/message/time"),
        value.pointer("/payload/timestamp"),
        value.pointer("/payload/time"),
    ]
    .into_iter()
    .flatten()
    {
        if let Some(text) = candidate
            .as_str()
            .map(str::trim)
            .filter(|text| !text.is_empty())
        {
            return Some(text.to_owned());
        }
        if let Some(number) = candidate.as_i64() {
            return Some(number.to_string());
        }
        if let Some(number) = candidate.as_u64() {
            return Some(number.to_string());
        }
    }
    None
}

fn apply_request_context(event: &Value, usage: &mut MessageUsage, context: &UsageRequestContext) {
    usage.variant = UsageVariant::from_metadata(event);
    let current = model_label(event);
    let same_model = current.as_ref().is_none_or(|model| {
        context
            .model
            .as_ref()
            .is_none_or(|previous| compatible_recorded_models(previous, model))
    });
    if same_model {
        usage.model = prefer_recorded_model(usage.model.take(), context.model.clone()).map(|raw| {
            context
                .model
                .as_deref()
                .map(|prior| model_with_provider_fallback(&raw, prior))
                .unwrap_or(raw)
        });
        usage.variant = usage.variant.with_fallback(&context.variant);
    }
}

/// Only explicit historical request boundaries or recorded selection changes
/// establish context. A usage event never becomes the next request's default.
fn update_request_context(
    adapter: HistoryAdapter,
    event: &Value,
    context: &mut UsageRequestContext,
) {
    let kind = event_type(event).to_ascii_lowercase();
    if matches!(
        kind.as_str(),
        "session"
            | "session.start"
            | "session_start"
            | "session.started"
            | "turn.completed"
            | "turn_complete"
            | "task_complete"
            | "task_failed"
    ) {
        *context = UsageRequestContext::default();
    }
    if adapter == HistoryAdapter::Pi
        && matches!(kind.as_str(), "model_change" | "thinking_level_change")
    {
        if kind == "model_change" {
            context.model = model_label(event);
        } else {
            context.variant = UsageVariant::from_metadata(event);
        }
        return;
    }
    let request_boundary = matches!(
        kind.as_str(),
        "turn_context"
            | "request"
            | "request.start"
            | "request_start"
            | "turn.start"
            | "turn_start"
            | "turn.started"
            | "step.start"
            | "step_start"
    ) || (matches!(kind.as_str(), "user" | "user.message" | "user_message")
        && adapter != HistoryAdapter::Pi);
    if request_boundary {
        *context = UsageRequestContext {
            model: model_label(event),
            variant: UsageVariant::from_metadata(event),
        };
    }
}

fn session_scope(value: &Value) -> Option<String> {
    [Some(value), value.get("data"), value.get("payload")]
        .into_iter()
        .flatten()
        .find_map(|candidate| {
            text_field(
                candidate,
                &[
                    "sessionId",
                    "session_id",
                    "conversationId",
                    "conversation_id",
                    "threadId",
                    "thread_id",
                ],
            )
        })
}

fn opaque_scope(value: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(b"lico-native-usage-scope-v1\0");
    hasher.update(value.as_bytes());
    format!("{:x}", hasher.finalize())
}

fn extension(path: &Path) -> String {
    path.extension()
        .and_then(|value| value.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase()
}

fn positive(value: i64) -> u64 {
    value.max(0) as u64
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::io::Write;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn temp_file(name: &str) -> std::path::PathBuf {
        std::env::temp_dir().join(format!(
            "lico-native-usage-parser-{name}-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ))
    }

    fn window() -> UsageWindow {
        UsageWindow::from_params(&json!({"now": "2026-07-15T12:00:00Z"}))
    }

    #[test]
    fn append_parser_reads_only_complete_exact_usage_records() {
        let path = temp_file("append");
        fs::write(
            &path,
            concat!(
                "{\"type\":\"assistant\",\"timestamp\":\"2026-07-15T10:00:00Z\",",
                "\"model\":\"claude-test\",\"usage\":{\"input_tokens\":10,\"output_tokens\":2}}\n",
                "{\"type\":\"assistant\",\"timestamp\":\"2026-07-15T10:01:00Z\",",
                "\"usage\":{\"input_tokens\":99"
            ),
        )
        .unwrap();
        let first = parse_append_source(
            HistoryAdapter::ClaudeCode,
            &path,
            0,
            &window(),
            false,
            Default::default(),
        )
        .unwrap();
        assert_eq!(first.summary.total_tokens(), 12);
        assert!(first.parsed_bytes < fs::metadata(&path).unwrap().len());
        fs::OpenOptions::new()
            .append(true)
            .open(&path)
            .unwrap()
            .write_all(b",\"output_tokens\":1}}\n")
            .unwrap();
        let second = parse_append_source(
            HistoryAdapter::ClaudeCode,
            &path,
            first.parsed_bytes,
            &window(),
            true,
            first.request_context.clone(),
        )
        .unwrap();
        assert_eq!(second.summary.total_tokens(), 100);
        fs::remove_file(path).unwrap();
    }

    #[test]
    fn openclaw_append_parser_does_not_estimate_gateway_consumption_from_text() {
        let path = temp_file("openclaw-gateway-append");
        fs::write(
            &path,
            [
                r#"{"type":"user.message","timestamp":"2026-07-15T10:00:00Z","sessionId":"synthetic-session","message":"history text is not gateway usage"}"#,
                r#"{"type":"assistant.message","timestamp":"2026-07-15T10:00:01Z","sessionId":"synthetic-session","message":"response text is not gateway usage"}"#,
            ]
            .join("\n"),
        )
        .unwrap();

        let parsed = parse_append_source(
            HistoryAdapter::OpenClaw,
            &path,
            0,
            &window(),
            false,
            Default::default(),
        )
        .unwrap();
        assert_eq!(parsed.summary.total_tokens(), 0);
        assert_eq!(parsed.summary.explicit_records, 0);
        assert_eq!(parsed.summary.estimated_records, 0);
        fs::remove_file(path).unwrap();
    }

    #[test]
    fn kimi_exact_usage_scopes_and_gemini_usage_metadata_are_normalized() {
        let kimi = explicit_usage_event(
            HistoryAdapter::KimiCode,
            &json!({
                "type": "StatusUpdate",
                "time": "2026-07-15T10:00:00Z",
                "model": "kimi-test",
                "token_usage": {
                    "input_other": 80,
                    "input_cache_read": 20,
                    "input_cache_creation": 5,
                    "output": 15
                }
            }),
            &window(),
        )
        .unwrap();
        assert_eq!(kimi.usage.total_tokens, 120);

        let kimi_session = explicit_usage_event(
            HistoryAdapter::KimiCode,
            &json!({
                "type": "usage.record",
                "time": "2026-07-15T10:01:00Z",
                "model": "kimi-test",
                "usageScope": "session",
                "usage": {
                    "inputOther": 40,
                    "inputCacheRead": 10,
                    "inputCacheCreation": 3,
                    "output": 12
                }
            }),
            &window(),
        )
        .unwrap();
        assert_eq!(kimi_session.usage.prompt_tokens, 53);
        assert_eq!(kimi_session.usage.cached_input_tokens, 10);
        assert_eq!(kimi_session.usage.completion_tokens, 12);
        assert_eq!(kimi_session.usage.total_tokens, 65);
        assert!(kimi_session.cumulative_key.is_none());

        let gemini = explicit_usage_event(
            HistoryAdapter::Antigravity,
            &json!({
                "type": "step.completed",
                "timestamp": "2026-07-15T10:00:00Z",
                "usageMetadata": {
                    "promptTokenCount": 100,
                    "cachedContentTokenCount": 40,
                    "candidatesTokenCount": 12,
                    "totalTokenCount": 112
                }
            }),
            &window(),
        )
        .unwrap();
        assert_eq!(gemini.usage.prompt_tokens, 100);
        assert_eq!(gemini.usage.cached_input_tokens, 40);
        assert_eq!(gemini.usage.completion_tokens, 12);

        assert!(
            explicit_usage_event(
                HistoryAdapter::KimiCode,
                &json!({
                    "type": "StatusUpdate",
                    "time": "2026-07-15T10:02:00Z",
                    "usageScope": "session",
                    "token_usage": {"input_other": 100, "output": 10}
                }),
                &window(),
            )
            .is_none()
        );
    }

    #[test]
    fn copilot_nested_usage_metadata_is_read_without_context_estimation() {
        let event = explicit_usage_event(
            HistoryAdapter::Copilot,
            &json!({
                "type": "assistant.message",
                "data": {
                    "timestamp": "2026-07-15T10:00:00Z",
                    "sessionId": "copilot-session",
                    "model": "copilot-test",
                    "usage": {"input_tokens": 18, "output_tokens": 4},
                    "conversationTokens": 999_999,
                    "currentTokens": 999_999
                }
            }),
            &window(),
        )
        .unwrap();

        assert_eq!(event.usage.total_tokens, 22);
        assert_eq!(event.usage.model.as_deref(), Some("copilot-test"));
    }

    #[test]
    fn openagent_database_projects_session_watermarks_and_cross_day_messages() {
        let path = temp_file("openagent.sqlite");
        let connection = Connection::open(&path).unwrap();
        connection
            .execute_batch(
                "CREATE TABLE session (
                   id TEXT, model TEXT, time_created INTEGER, time_updated INTEGER,
                   tokens_input INTEGER, tokens_output INTEGER, tokens_reasoning INTEGER,
                   tokens_cache_read INTEGER, tokens_cache_write INTEGER
                 );
                 CREATE TABLE message (
                   id TEXT, session_id TEXT, time_created INTEGER, data TEXT
                 );
                 INSERT INTO session VALUES(
                   'single-day','model-a',1784080800000,1784080860000,60,5,2,30,10
                 );
                 INSERT INTO session VALUES(
                   'cross-day','model-b',1783994400000,1784080800000,999,999,0,0,0
                 );",
            )
            .unwrap();
        for (id, session, timestamp, data) in [
            (
                "partial-message",
                "single-day",
                1_784_080_800_000i64,
                json!({"effort":"high", "tokens": {"input": 50, "output": 5}}),
            ),
            (
                "cross-day-1",
                "cross-day",
                1_783_994_400_000i64,
                json!({"model":"model-b", "tokens": {"input": 10, "output": 2}}),
            ),
            (
                "cross-day-2",
                "cross-day",
                1_784_080_800_000i64,
                json!({"model":"model-b", "tokens": {"input": 20, "output": 3}}),
            ),
        ] {
            connection
                .execute(
                    "INSERT INTO message VALUES(?1,?2,?3,?4)",
                    rusqlite::params![id, session, timestamp, data.to_string()],
                )
                .unwrap();
        }
        drop(connection);

        let parsed = parse_openagent_usage_database(&path, &window()).unwrap();
        let summary = parsed.summary;
        assert_eq!(summary.total_tokens(), 142);
        assert_eq!(summary.session_count, 2);
        assert_eq!(parsed.cumulative_snapshots.len(), 2);
        assert!(
            parsed
                .cumulative_snapshots
                .iter()
                .all(|snapshot| !snapshot.projects_usage)
        );
        assert_eq!(
            summary.daily_usage["2026-07-15"].model_usage["model-b"].total_tokens,
            23
        );
        assert_eq!(
            summary.daily_usage["2026-07-14"].model_usage["model-b"].total_tokens,
            12
        );
        fs::remove_file(path).unwrap();
    }
    #[test]
    fn actual_request_context_survives_append_and_resets_before_missing_effort() {
        let path = temp_file("request-options");
        fs::write(&path, [
            json!({"type":"request.start","model":"k3","reasoning":{"effort":"high"}}),
            json!({"type":"usage.record","timestamp":"2026-07-15T10:00:00Z","usage":{"inputOther":10,"output":2}})
        ].iter().map(Value::to_string).collect::<Vec<_>>().join("\n")+"\n").unwrap();
        let first = parse_append_source(
            HistoryAdapter::KimiCode,
            &path,
            0,
            &window(),
            false,
            Default::default(),
        )
        .unwrap();
        assert_eq!(
            first.summary.daily_usage["2026-07-15"].model_variants[&(
                "k3".to_owned(),
                UsageVariant {
                    effort: Some("high".into()),
                    fast: None
                }
            )]
                .total_tokens,
            12
        );
        fs::OpenOptions::new().append(true).open(&path).unwrap().write_all(([
            json!({"type":"usage.record","timestamp":"2026-07-15T10:01:00Z","usage":{"inputOther":3,"output":1}}),
            json!({"type":"request.start","model":"k3"}),
            json!({"type":"usage.record","timestamp":"2026-07-15T10:02:00Z","usage":{"inputOther":5,"output":1}})
        ].iter().map(Value::to_string).collect::<Vec<_>>().join("\n")+"\n").as_bytes()).unwrap();
        let second = parse_append_source(
            HistoryAdapter::KimiCode,
            &path,
            first.parsed_bytes,
            &window(),
            true,
            first.request_context,
        )
        .unwrap();
        let day = &second.summary.daily_usage["2026-07-15"];
        assert_eq!(
            day.model_variants[&(
                "k3".to_owned(),
                UsageVariant {
                    effort: Some("high".into()),
                    fast: None
                }
            )]
                .total_tokens,
            4
        );
        assert_eq!(
            day.model_variants[&("k3".to_owned(), UsageVariant::default())].total_tokens,
            6
        );
        assert_eq!(day.total_tokens, 10);
        fs::remove_file(path).unwrap();
    }

    #[test]
    fn catalog_file_adapters_preserve_actual_options_and_counter_conservation() {
        use crate::domain::conversation::source_catalog::usage_adapter_for_agent;
        let path = temp_file("all-file-adapters");
        fs::write(&path,json!({"type":"usage.record","timestamp":"2026-07-15T10:00:00Z","model":"synthetic-model","effort":"medium","fast":true,"usage":{"input_tokens":10,"output_tokens":2}}).to_string()+"\n").unwrap();
        for agent in licoup_statistics::contract::supported_agents() {
            let Some(adapter) = usage_adapter_for_agent(agent.id) else {
                continue;
            };
            if matches!(
                adapter,
                HistoryAdapter::Codex
                    | HistoryAdapter::Cursor
                    | HistoryAdapter::OpenClaw
                    | HistoryAdapter::Hermes
            ) {
                continue;
            }
            let parsed =
                parse_append_source(adapter, &path, 0, &window(), false, Default::default())
                    .unwrap();
            assert_eq!(parsed.summary.total_tokens(), 12, "{}", agent.id);
            let values = parsed.summary.daily_usage["2026-07-15"]
                .model_variants
                .values();
            assert_eq!(
                values.map(|usage| usage.total_tokens).sum::<u64>(),
                12,
                "{}",
                agent.id
            );
            assert!(
                parsed.summary.daily_usage["2026-07-15"]
                    .model_variants
                    .keys()
                    .all(|(_, variant)| variant.effort.as_deref() == Some("medium")
                        && variant.fast == Some(true)),
                "{}",
                agent.id
            );
        }
        fs::remove_file(path).unwrap();
    }

    #[test]
    fn file_adapters_resolve_placeholders_from_same_record_or_current_request_only() {
        use crate::domain::conversation::source_catalog::usage_adapter_for_agent;
        let path = temp_file("placeholder-file-adapters");
        let records = [
            json!({"type":"request.start","model":"default","effort":"high"}),
            json!({"type":"usage.record","model":"default","message":{"model":"actual-a"},"timestamp":"2026-07-15T10:00:00Z","usage":{"input_tokens":10,"output_tokens":2}}),
            json!({"type":"request.start","model":"actual-b","effort":"medium"}),
            json!({"type":"usage.record","model":"unknown","timestamp":"2026-07-15T10:00:01Z","usage":{"input_tokens":7,"output_tokens":1}}),
            json!({"type":"turn.completed"}),
            json!({"type":"usage.record","model":"auto","timestamp":"2026-07-15T10:00:02Z","usage":{"input_tokens":4,"output_tokens":1}}),
            json!({"type":"usage.record","model":"default","response":{"model":"actual-c"},"timestamp":"2026-07-15T10:00:03Z","usage":{"input_tokens":8,"output_tokens":2}}),
            json!({"type":"request.start","model":{"id":"default","providerID":"provider-a"},"effort":"max"}),
            json!({"type":"usage.record","model":{"id":"actual-d","providerID":"provider-b"},"timestamp":"2026-07-15T10:00:04Z","usage":{"input_tokens":2,"output_tokens":1}}),
        ];
        fs::write(
            &path,
            records
                .iter()
                .map(Value::to_string)
                .collect::<Vec<_>>()
                .join("\n")
                + "\n",
        )
        .unwrap();
        for agent in licoup_statistics::contract::supported_agents() {
            let Some(adapter) = usage_adapter_for_agent(agent.id) else {
                continue;
            };
            if matches!(
                adapter,
                HistoryAdapter::Codex
                    | HistoryAdapter::Cursor
                    | HistoryAdapter::DeepSeekHarness
                    | HistoryAdapter::OpenClaw
                    | HistoryAdapter::Hermes
            ) {
                continue;
            }
            let parsed =
                parse_append_source(adapter, &path, 0, &window(), false, Default::default())
                    .unwrap();
            let day = &parsed.summary.daily_usage["2026-07-15"];
            assert_eq!(day.total_tokens, 38, "{}", agent.id);
            assert_eq!(day.model_usage["actual-a"].total_tokens, 12, "{}", agent.id);
            assert_eq!(day.model_usage["actual-b"].total_tokens, 8, "{}", agent.id);
            assert_eq!(day.model_usage["actual-c"].total_tokens, 10, "{}", agent.id);
            assert_eq!(day.model_usage["auto"].total_tokens, 5, "{}", agent.id);
            assert_eq!(
                day.model_variants[&(
                    "actual-a".into(),
                    UsageVariant {
                        effort: Some("high".into()),
                        fast: None
                    }
                )]
                    .total_tokens,
                12
            );
            assert_eq!(
                day.model_variants[&(
                    "actual-b".into(),
                    UsageVariant {
                        effort: Some("medium".into()),
                        fast: None
                    }
                )]
                    .total_tokens,
                8
            );
            let (_, recorded) = day
                .model_variants
                .iter()
                .find(|((raw, variant), _)| {
                    let selected = licoup_statistics::variant::model_selection(raw);
                    selected.id == "actual-d"
                        && selected.provider_id.as_deref() == Some("provider-b")
                        && variant == &UsageVariant::default()
                })
                .expect("different explicit provider does not inherit request effort");
            assert_eq!(recorded.total_tokens, 3);
        }
        fs::remove_file(path).unwrap();
    }

    #[test]
    fn snapshot_adapters_preserve_actual_model_inside_the_same_native_record() {
        let cases = [
            (
                HistoryAdapter::ClaudeCode,
                json!({"type":"assistant","model":"default","timestamp":"2026-07-15T10:00:00Z","message":{"role":"assistant","content":"synthetic reply","model":"actual-model","usage":{"input_tokens":10,"output_tokens":2}}}),
            ),
            (
                HistoryAdapter::Pi,
                json!({"type":"message","model":"default","timestamp":"2026-07-15T10:00:00Z","message":{"role":"assistant","content":"synthetic reply","model":"actual-model","usage":{"input_tokens":10,"output_tokens":2}}}),
            ),
            (
                HistoryAdapter::Copilot,
                json!({"type":"assistant.message","timestamp":"2026-07-15T10:00:00Z","data":{"content":"synthetic reply","model":"default","usage":{"model":"actual-model","input_tokens":10,"output_tokens":2}}}),
            ),
            (
                HistoryAdapter::KimiCode,
                json!({"type":"usage.record","usageScope":"turn","model":"default","timestamp":"2026-07-15T10:00:00Z","usage":{"model":"actual-model","input_tokens":10,"output_tokens":2}}),
            ),
            (
                HistoryAdapter::LicoAgent,
                json!({"type":"usage.record","model":"default","timestamp":"2026-07-15T10:00:00Z","usage":{"model":"actual-model","input_tokens":10,"output_tokens":2}}),
            ),
        ];
        for (adapter, record) in cases {
            let directory = temp_file("placeholder-native-snapshot");
            let path = if adapter == HistoryAdapter::KimiCode {
                directory.join("agents").join("worker").join("wire.jsonl")
            } else {
                directory.join("transcript.jsonl")
            };
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(&path, record.to_string() + "\n").unwrap();
            let parsed = parse_snapshot_source(
                adapter,
                &path,
                "fixture",
                &fs::metadata(&path).unwrap(),
                &window(),
            )
            .unwrap();
            assert_eq!(parsed.summary.total_tokens(), 12, "{}", adapter.id());
            assert_eq!(
                parsed.summary.daily_usage["2026-07-15"].model_usage["actual-model"].total_tokens,
                12,
                "{}",
                adapter.id()
            );
            fs::remove_dir_all(directory).unwrap();
        }
    }
    #[test]
    fn observed_estimated_variant_survives_an_explicit_model_change() {
        let path = temp_file("estimated-model-change");
        fs::write(&path,[
            json!({"type":"request.start","model":"model-a","effort":"high"}),
            json!({"type":"assistant","model":"model-b","effort":"low","fast":true,"timestamp":"2026-07-15T10:00:00Z","text":"synthetic"})
        ].iter().map(Value::to_string).collect::<Vec<_>>().join("\n")+"\n").unwrap();
        let parsed = parse_append_source(
            HistoryAdapter::ClaudeCode,
            &path,
            0,
            &window(),
            false,
            Default::default(),
        )
        .unwrap();
        let day = &parsed.summary.daily_usage["2026-07-15"];
        assert!(day.model_variants.contains_key(&(
            "model-b".to_owned(),
            UsageVariant {
                effort: Some("low".into()),
                fast: Some(true)
            }
        )));
        assert_eq!(day.model_variants.len(), 1);
        fs::remove_file(path).unwrap();
    }
    #[test]
    fn lico_snapshot_counts_each_model_call_once_and_covers_turn_text() {
        let path = temp_file("lico-usage").with_extension("jsonl");
        fs::write(&path,[
            json!({"type":"session","id":"synthetic-lico"}),
            json!({"type":"message","role":"user","text":"synthetic prompt","timestamp":"2026-07-15T10:00:00Z"}),
            json!({"type":"message","role":"assistant","text":"synthetic response","timestamp":"2026-07-15T10:00:00Z"}),
            json!({"type":"usage.record","model":"model-a","timestamp":"2026-07-15T10:00:00Z","usage":{"promptTokens":10,"completionTokens":2,"reasoningEffort":"high"}}),
            json!({"type":"usage.record","model":"model-a","timestamp":"2026-07-15T10:00:00Z","usage":{"promptTokens":10,"completionTokens":2,"reasoningEffort":"high"}}),
            json!({"type":"usage.record","model":"model-b","timestamp":"2026-07-15T10:00:00Z","usage":{"promptTokens":10,"completionTokens":2,"reasoningEffort":"low","fast":true}})
        ].iter().map(Value::to_string).collect::<Vec<_>>().join("\n")+"\n").unwrap();
        let metadata = fs::metadata(&path).unwrap();
        let parsed = parse_snapshot_source(
            HistoryAdapter::LicoAgent,
            &path,
            "fixture",
            &metadata,
            &window(),
        )
        .unwrap();
        assert_eq!(parsed.summary.total_tokens(), 36);
        assert_eq!(parsed.summary.estimated_total_tokens, 0);
        assert_eq!(parsed.summary.explicit_records, 3);
        let variants = &parsed.summary.daily_usage["2026-07-15"].model_variants;
        assert_eq!(
            variants[&(
                "model-a".to_owned(),
                UsageVariant {
                    effort: Some("high".into()),
                    fast: None
                }
            )]
                .total_tokens,
            24
        );
        assert_eq!(
            variants[&(
                "model-b".to_owned(),
                UsageVariant {
                    effort: Some("low".into()),
                    fast: Some(true)
                }
            )]
                .total_tokens,
            12
        );
        fs::remove_file(path).unwrap();
    }
}
