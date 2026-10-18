use std::sync::OnceLock;

use regex::Regex;
use serde_json::Value;

use super::semantic::{HistoryMessageKind, normalize_history_message_semantic};

/// Displayable detail for one structured event. Reasoning, metadata, and tool
/// calls are formatted from their recorded payloads instead of being blanked;
/// every candidate still passes the same secret/path sanitization as free
/// text. When nothing usable is recorded, the caller's localized fallback
/// explains the absence.
pub(super) fn structured_event_text(
    kind: HistoryMessageKind,
    value: &Value,
    fallback: &str,
) -> String {
    let candidate = match kind {
        HistoryMessageKind::Reasoning => structured_reasoning_detail(value),
        HistoryMessageKind::Metadata => structured_metadata_detail(value),
        HistoryMessageKind::ToolCall => structured_tool_call_detail(value),
        _ => structured_event_detail_candidate(value)
            .and_then(|text| structured_formatted_text_payload(&text, RESULT_SKIP_KEYS)),
    };
    candidate
        .and_then(|text| sanitize_structured_event_text(&text))
        .unwrap_or_else(|| fallback.to_string())
}

/// Recorded chain-of-thought detail for a reasoning event. Only provider-
/// recorded thinking text is eligible (`text` for Codex/Kimi, `thinking`/
/// `think` for Claude Code content blocks). The provider summary is handled
/// separately by [structured_reasoning_summary] and becomes the collapsed
/// headline preview rather than the detail body.
pub(super) fn structured_reasoning_detail(value: &Value) -> Option<String> {
    for key in ["text", "thinking", "think", "thought", "thinking_text"] {
        if let Some(text) = value.get(key).and_then(Value::as_str) {
            let trimmed = text.trim();
            if !trimmed.is_empty() {
                return Some(trimmed.to_string());
            }
        }
    }
    None
}

/// Only provider-owned summary fields are eligible for the summary channel.
/// Generic reasoning or thinking fields can contain chain-of-thought and
/// stay inside the detail body, never duplicated into the headline.
pub(super) fn structured_reasoning_summary(value: &Value) -> Option<String> {
    for key in ["summary", "reasoningSummary", "reasoning_summary"] {
        let Some(candidate) = value.get(key) else {
            continue;
        };
        if let Some(summary) = structured_reasoning_summary_value(candidate, 0) {
            return Some(summary);
        }
    }
    None
}

pub(super) fn structured_reasoning_summary_value(value: &Value, _depth: usize) -> Option<String> {
    let mut pending = vec![value];
    let mut summaries = Vec::new();
    while let Some(candidate) = pending.pop() {
        match candidate {
            Value::String(text) => {
                let text = text.trim();
                if !text.is_empty() {
                    summaries.push(text.to_string());
                }
            }
            Value::Array(items) => pending.extend(items.iter().rev()),
            Value::Object(object) => {
                if let Some(kind) = object
                    .get("type")
                    .or_else(|| object.get("kind"))
                    .and_then(Value::as_str)
                {
                    let normalized = normalize_history_message_semantic(kind);
                    if !matches!(
                        normalized.as_str(),
                        "summary" | "summary-text" | "reasoning-summary" | "text"
                    ) {
                        continue;
                    }
                }
                if let Some(child) = ["text", "content", "summary"]
                    .iter()
                    .find_map(|key| object.get(*key))
                {
                    pending.push(child);
                }
            }
            _ => {}
        }
    }
    (!summaries.is_empty()).then(|| summaries.join("\n"))
}

/// Identity bookkeeping skipped when formatting tool-call arguments. The tool
/// name already lives in the card title.
const TOOL_CALL_SKIP_KEYS: &[&str] = &[
    "type",
    "id",
    "callId",
    "call_id",
    "tool_use_id",
    "toolCallId",
    "tool_call_id",
    "name",
    "toolName",
    "tool_name",
    "functionName",
    "function_name",
];

/// Provider framing keys skipped when formatting tool results and event
/// payloads; output data (including `id`) is kept.
const RESULT_SKIP_KEYS: &[&str] = &["type", "tool_use_id", "call_id"];

/// Tool invocation detail: the argument payload formatted as `key: value`
/// lines (nested values stay compact JSON). Provider identity keys are
/// skipped — the tool name already lives in the card title.
pub(super) fn structured_tool_call_detail(value: &Value) -> Option<String> {
    let arguments = ["input", "arguments", "args", "params", "parameters"]
        .iter()
        .find_map(|key| value.get(*key))
        .or_else(|| {
            value.get("data").and_then(|data| {
                ["input", "arguments", "args"]
                    .iter()
                    .find_map(|key| data.get(*key))
            })
        })
        .or_else(|| value.get("content"));
    let Some(arguments) = arguments else {
        return structured_event_detail_candidate(value);
    };
    structured_formatted_payload(arguments, TOOL_CALL_SKIP_KEYS)
        .or_else(|| structured_event_detail_candidate(value))
}

/// Metadata key-value detail. Identity/timestamp/source bookkeeping and raw
/// image payloads are skipped; remaining entries become `key: value` lines
/// (nested values stay compact JSON). JSON-encoded `content` objects are
/// unfolded into entries.
pub(super) fn structured_metadata_detail(value: &Value) -> Option<String> {
    const SKIP: &[&str] = &[
        "type",
        "kind",
        "role",
        "name",
        "status",
        "timestamp",
        "time",
        "createdAt",
        "updatedAt",
        "sourcePath",
        "sourceItemType",
        "sourceEventType",
        "sourceTable",
        "sourceKey",
        "sourceMessageId",
        "contentHash",
        "byteLength",
        "pathRef",
        "schemaVersion",
        "event",
        "sessionId",
        "session_id",
        "turnId",
        "turn_id",
        "requestId",
        "correlationId",
        "data",
        "base64",
        "imageData",
        "image_data",
    ];
    let Some(object) = value.as_object() else {
        return structured_event_detail_candidate(value);
    };
    let mut lines = Vec::new();
    for (key, item) in object {
        if SKIP.contains(&key.as_str()) {
            continue;
        }
        match item {
            Value::String(text) => {
                let trimmed = text.trim();
                if trimmed.is_empty() {
                    continue;
                }
                if key == "content" {
                    if let Ok(decoded) = serde_json::from_str::<Value>(trimmed) {
                        if let Some(entries) = structured_entries_text(&decoded, &[]) {
                            lines.push(entries);
                            continue;
                        }
                    }
                }
                lines.push(format!("{key}: {trimmed}"));
            }
            Value::Object(_) | Value::Array(_) => {
                if let Ok(serialized) = serde_json::to_string(item) {
                    lines.push(format!("{key}: {serialized}"));
                }
            }
            Value::Null => {}
            other => lines.push(format!("{key}: {other}")),
        }
    }
    (!lines.is_empty()).then(|| lines.join("\n"))
}

fn structured_formatted_payload(value: &Value, skip: &[&str]) -> Option<String> {
    match value {
        Value::String(text) => structured_formatted_text_payload(text, skip),
        Value::Object(_) | Value::Array(_) => structured_entries_text(value, skip),
        _ => None,
    }
}

/// JSON-encoded provider payloads are unfolded into `key: value` lines so
/// raw payload blobs never surface; plain text passes through unchanged.
fn structured_formatted_text_payload(text: &str, skip: &[&str]) -> Option<String> {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return None;
    }
    if let Ok(decoded) = serde_json::from_str::<Value>(trimmed) {
        match &decoded {
            Value::String(inner) if !inner.trim().is_empty() => {
                return Some(inner.trim().to_string());
            }
            Value::Object(_) | Value::Array(_) => {
                if let Some(entries) = structured_entries_text(&decoded, skip) {
                    return Some(entries);
                }
            }
            _ => {}
        }
    }
    Some(trimmed.to_string())
}

/// `key: value` lines for an object, or entry lines for an array. Values that
/// are themselves objects/arrays stay compact JSON (sanitized afterwards).
/// `skip` filters provider identity bookkeeping that belongs to the card
/// header instead of the detail body.
fn structured_entries_text(value: &Value, skip: &[&str]) -> Option<String> {
    let mut lines = Vec::new();
    match value {
        Value::Object(object) => {
            for (key, item) in object {
                if skip.contains(&key.as_str()) {
                    continue;
                }
                push_entry(&mut lines, key, item);
            }
        }
        Value::Array(items) => {
            for item in items {
                match item {
                    Value::Object(_) => {
                        if let Some(entry) = structured_entries_text(item, skip) {
                            lines.push(entry);
                        }
                    }
                    Value::String(text) => {
                        let trimmed = text.trim();
                        if !trimmed.is_empty() {
                            lines.push(trimmed.to_string());
                        }
                    }
                    other => lines.push(other.to_string()),
                }
            }
        }
        _ => return None,
    }
    (!lines.is_empty()).then(|| lines.join("\n"))
}

fn push_entry(lines: &mut Vec<String>, key: &str, value: &Value) {
    match value {
        Value::String(text) => {
            let trimmed = text.trim();
            if !trimmed.is_empty() {
                lines.push(format!("{key}: {trimmed}"));
            }
        }
        Value::Object(_) | Value::Array(_) => {
            if let Ok(serialized) = serde_json::to_string(value) {
                lines.push(format!("{key}: {serialized}"));
            }
        }
        Value::Null => {}
        other => lines.push(format!("{key}: {other}")),
    }
}

fn structured_event_detail_candidate(value: &Value) -> Option<String> {
    for key in [
        "error",
        "reason",
        "message",
        "summary",
        "text",
        "output",
        "result",
        "content",
        "command",
        "detail",
        "description",
    ] {
        let Some(candidate) = value.get(key) else {
            continue;
        };
        match candidate {
            Value::String(text) => {
                let trimmed = text.trim();
                if !trimmed.is_empty() {
                    return Some(trimmed.to_string());
                }
            }
            Value::Object(_) => {
                if let Some(text) = nested_detail_string(candidate) {
                    return Some(text);
                }
            }
            _ => {}
        }
    }
    None
}

fn nested_detail_string(value: &Value) -> Option<String> {
    for key in [
        "message",
        "text",
        "summary",
        "command",
        "detail",
        "description",
        "reason",
    ] {
        if let Some(text) = value.get(key).and_then(Value::as_str) {
            let trimmed = text.trim();
            if !trimmed.is_empty() {
                return Some(trimmed.to_string());
            }
        }
    }
    None
}

pub(super) fn sanitize_structured_label(value: &str) -> Option<String> {
    let trimmed = value.trim();
    if trimmed.is_empty()
        || trimmed.chars().count() > 96
        || trimmed
            .chars()
            .any(|character| matches!(character, '\n' | '\r' | '/' | '\\' | '{' | '}' | '[' | ']'))
        || secret_assignment_regex().is_match(trimmed)
        || opaque_value_regex().is_match(trimmed)
    {
        return None;
    }
    Some(trimmed.to_string())
}

pub(super) fn sanitize_structured_event_text(value: &str) -> Option<String> {
    let trimmed = value.trim();
    if trimmed.is_empty() || looks_like_raw_structured_payload(trimmed) {
        return None;
    }
    let redacted = bearer_regex().replace_all(trimmed, "Bearer [redacted]");
    let redacted = secret_assignment_regex().replace_all(&redacted, "$1: [redacted]");
    let redacted = relative_path_regex().replace_all(&redacted, "$1[local path hidden]");
    let redacted = local_path_regex().replace_all(&redacted, "[local path hidden]");
    let redacted = opaque_value_regex().replace_all(&redacted, "[opaque value hidden]");
    let redacted = redacted.trim();
    if redacted.is_empty() {
        return None;
    }
    Some(redacted.to_string())
}

/// Whole-document JSON payloads are rejected: they cannot be redacted
/// reliably. Formatted `key: value` lines and inline JSON values pass and are
/// redacted by the value-level patterns above.
pub(super) fn looks_like_raw_structured_payload(value: &str) -> bool {
    let trimmed = value.trim();
    let candidate = trimmed
        .strip_prefix("```json")
        .or_else(|| trimmed.strip_prefix("```JSON"))
        .map(|text| {
            text.trim()
                .strip_suffix("```")
                .unwrap_or(text.trim())
                .trim()
        })
        .unwrap_or(trimmed);
    if !((candidate.starts_with('{') && candidate.ends_with('}'))
        || (candidate.starts_with('[') && candidate.ends_with(']')))
    {
        return false;
    }
    serde_json::from_str::<Value>(candidate)
        .map(|value| value.is_object() || value.is_array())
        .unwrap_or(true)
}

fn bearer_regex() -> &'static Regex {
    static REGEX: OnceLock<Regex> = OnceLock::new();
    REGEX.get_or_init(|| {
        Regex::new(r"(?i)\bbearer\s+[a-z0-9._~+\-/]+=*").expect("valid bearer regex")
    })
}

fn secret_assignment_regex() -> &'static Regex {
    static REGEX: OnceLock<Regex> = OnceLock::new();
    REGEX.get_or_init(|| {
        Regex::new(
            r#"(?i)\b(api[_-]?key|access[_-]?token|refresh[_-]?token|authorization|password|secret|cookie|credential)\b\s*[:=]\s*(?:\"[^\"]*\"|'[^']*'|[^\s,;]+)"#,
        )
        .expect("valid secret assignment regex")
    })
}

fn local_path_regex() -> &'static Regex {
    static REGEX: OnceLock<Regex> = OnceLock::new();
    REGEX.get_or_init(|| {
        Regex::new(
            r#"(?i)(?:file://)?/(?:[a-z0-9._-]+/)*[a-z0-9._-]+[^\s\"'<>]*|[a-z]:\\[^\s\"'<>]*|~[/\\][^\s\"'<>]*"#,
        )
        .expect("valid local path regex")
    })
}

fn relative_path_regex() -> &'static Regex {
    static REGEX: OnceLock<Regex> = OnceLock::new();
    REGEX.get_or_init(|| {
        Regex::new(r#"(?i)(^|[\s(\"'=])((?:\.{1,2}[/\\])?[a-z0-9._-]+(?:[/\\][a-z0-9._-]+)+)"#)
            .expect("valid relative local path regex")
    })
}

fn opaque_value_regex() -> &'static Regex {
    static REGEX: OnceLock<Regex> = OnceLock::new();
    REGEX.get_or_init(|| Regex::new(r"\b[a-zA-Z0-9_-]{40,}\b").expect("valid opaque value regex"))
}
