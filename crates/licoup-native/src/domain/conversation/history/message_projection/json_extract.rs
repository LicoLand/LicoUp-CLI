use serde_json::{Map, Value};

use super::antigravity::has_user_request_tag;
use super::generated_context::generated_control_text;

pub(in crate::domain::conversation::history) fn extract_text(value: &Value) -> Option<String> {
    extract_text_iterative(value, true)
}

fn extract_text_iterative(value: &Value, decode_embedded_json: bool) -> Option<String> {
    let mut pending = vec![value];
    let mut parts = Vec::new();
    while let Some(candidate) = pending.pop() {
        match candidate {
            Value::String(text) => {
                let trimmed = text.trim();
                if trimmed.is_empty()
                    || (generated_control_text(trimmed) && !has_user_request_tag(trimmed))
                {
                    continue;
                }
                if decode_embedded_json
                    && let Some(decoded) = parse_embedded_json_text(trimmed)
                    && !decoded
                        .as_object()
                        .map(structured_content_object_is_tool_or_metadata)
                        .unwrap_or(false)
                    && let Some(text) = extract_text_iterative(&decoded, false)
                    && !text.trim().is_empty()
                    && (!generated_control_text(&text) || has_user_request_tag(&text))
                {
                    parts.push(text);
                    continue;
                }
                parts.push(text.to_string());
            }
            Value::Array(items) => pending.extend(items.iter().rev()),
            Value::Object(object) => {
                if structured_content_object_is_tool_or_metadata(object) {
                    continue;
                }
                let child = if structured_content_object_is_text(object) {
                    object
                        .get("text")
                        .filter(|value| value_may_have_text(value))
                        .or_else(|| object.get("content"))
                } else {
                    [
                        "text", "content", "message", "messages", "prompt", "response", "answer",
                        "summary", "value", "parts", "items", "turns",
                    ]
                    .iter()
                    .find_map(|key| object.get(*key).filter(|value| value_may_have_text(value)))
                };
                if let Some(child) = child {
                    pending.push(child);
                }
            }
            _ => {}
        }
    }
    (!parts.is_empty()).then(|| parts.join("\n"))
}

fn value_may_have_text(value: &Value) -> bool {
    match value {
        Value::Null => false,
        Value::String(text) => !text.trim().is_empty(),
        Value::Array(items) => !items.is_empty(),
        Value::Object(object) => !object.is_empty(),
        _ => false,
    }
}

fn structured_content_object_is_tool_or_metadata(object: &Map<String, Value>) -> bool {
    let kind = object
        .get("type")
        .or_else(|| object.get("kind"))
        .or_else(|| object.get("role"))
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_ascii_lowercase();
    matches!(
        kind.as_str(),
        "tool_result"
            | "tool-use"
            | "tool_use"
            | "tool"
            | "function_call"
            | "function_call_output"
            | "input_json_delta"
            | "thinking"
            | "redacted_thinking"
            | "image"
            | "image_url"
            | "document"
            | "attachment"
            | "metadata"
            | "system"
    )
}

fn structured_content_object_is_text(object: &Map<String, Value>) -> bool {
    let kind = object
        .get("type")
        .or_else(|| object.get("kind"))
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_ascii_lowercase();
    matches!(
        kind.as_str(),
        "text" | "input_text" | "output_text" | "markdown"
    )
}

fn parse_embedded_json_text(text: &str) -> Option<Value> {
    let trimmed = text.trim();
    let structured = (trimmed.starts_with('{') && trimmed.ends_with('}'))
        || (trimmed.starts_with('[') && trimmed.ends_with(']'));
    if !structured {
        return None;
    }
    let value = serde_json::from_str::<Value>(trimmed).ok()?;
    embedded_json_may_hold_message_text(&value).then_some(value)
}

fn embedded_json_may_hold_message_text(value: &Value) -> bool {
    let mut pending = vec![value];
    while let Some(candidate) = pending.pop() {
        match candidate {
            Value::Array(items) => pending.extend(items),
            Value::Object(object) => {
                if object.keys().any(|key| {
                    matches!(
                        key.as_str(),
                        "text"
                            | "content"
                            | "message"
                            | "messages"
                            | "prompt"
                            | "response"
                            | "answer"
                            | "summary"
                            | "value"
                            | "parts"
                            | "items"
                            | "turns"
                    )
                }) {
                    return true;
                }
                pending.extend(object.values());
            }
            _ => {}
        }
    }
    false
}

pub(in crate::domain::conversation::history) fn extract_role(value: &Value) -> String {
    if let Some(type_code) = value.get("type").and_then(Value::as_i64) {
        match type_code {
            1 => return "user".to_string(),
            0 | 2 => return "agent".to_string(),
            _ => {}
        }
    }
    let role = find_string(value, &["role", "author", "speaker", "type", "source"])
        .unwrap_or_else(|| "system".to_string())
        .to_ascii_lowercase();
    if role.contains("user") || role.contains("human") || role == "1" {
        "user".to_string()
    } else if role.contains("assistant")
        || role.contains("agent")
        || role.contains("model")
        || role.contains("ai")
        || role == "0"
        || role == "2"
    {
        "agent".to_string()
    } else if role.contains("tool") {
        "tool".to_string()
    } else {
        role
    }
}

pub(in crate::domain::conversation::history) fn extract_timestamp(value: &Value) -> Option<String> {
    find_string(
        value,
        &[
            "createdAt",
            "updatedAt",
            "timestamp",
            "time",
            "date",
            "created_at",
            "updated_at",
        ],
    )
}

/// Model identifiers ride next to the message payload in native history
/// events (for example Claude Code JSONL keeps them at `message.model`), so
/// inspect the event and its structured request/response/usage metadata, skipping
/// placeholders when a concrete same-record model is present. Never crawl
/// user-authored content for a model-like string.
pub(in crate::domain::conversation::history) fn extract_native_model(
    value: &Value,
) -> Option<String> {
    crate::domain::agent_usage::recorded_usage_model(value)
}

pub(in crate::domain::conversation::history) fn find_string(
    value: &Value,
    keys: &[&str],
) -> Option<String> {
    let mut candidate = value;
    loop {
        let object = candidate.as_object()?;
        for key in keys {
            if let Some(text) = object.get(*key).and_then(Value::as_str) {
                return Some(text.to_string());
            }
            if let Some(number) = object.get(*key).and_then(Value::as_i64) {
                return Some(number.to_string());
            }
        }
        candidate = object.get("message")?;
    }
}

pub(in crate::domain::conversation::history) fn extract_native_session_id(
    value: &Value,
) -> Option<String> {
    find_string(
        value,
        &[
            "sessionId",
            "session_id",
            "conversationId",
            "conversation_id",
            "chatId",
            "chat_id",
            "threadId",
            "thread_id",
            "sessionKey",
            "session_key",
        ],
    )
    .filter(|value| !value.trim().is_empty())
}
