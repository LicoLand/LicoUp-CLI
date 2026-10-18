use std::path::Path;

use serde_json::{Value, json};

use crate::domain::conversation::usage::{UsageFields, collect_token_usage};

use super::super::HistoryAdapter;
use super::super::message_projection::{
    clean_native_message_text, extract_role, extract_text, extract_timestamp,
    native_history_message_id, native_message_timestamp,
};
use super::super::query_filter::{display_path, epoch_value_to_rfc3339};

pub(super) fn cursor_message_from_bubble(
    bubble: &Value,
    fallback_model: &str,
    path: &Path,
    index: usize,
) -> Option<Value> {
    let role = cursor_bubble_role(bubble)?;
    let created_at = epoch_value_to_rfc3339(bubble.get("createdAt").unwrap_or(&Value::Null))
        .or_else(|| extract_timestamp(bubble));
    let text = clean_native_message_text(
        HistoryAdapter::Cursor,
        role,
        &extract_text(bubble).unwrap_or_default(),
    )
    .unwrap_or_default();
    let model = cursor_bubble_model(bubble).unwrap_or_else(|| fallback_model.to_string());
    let usage = cursor_bubble_usage(bubble, &model);
    let attachments = cursor_bubble_attachments(bubble);
    let has_usage = usage
        .as_ref()
        .and_then(|value| value.get("totalTokens"))
        .and_then(Value::as_u64)
        .unwrap_or(0)
        > 0;
    if text.trim().is_empty() && !has_usage && attachments.is_empty() {
        return None;
    }
    let mut message = json!({
        "id": native_history_message_id(HistoryAdapter::Cursor, path, index, 0),
        "role": role,
        "text": text,
        "createdAt": created_at.unwrap_or_else(native_message_timestamp),
        "sourcePath": display_path(path),
        "model": model,
    });
    if let Some(usage) = usage
        && let Some(object) = message.as_object_mut()
    {
        object.insert("usage".to_string(), usage);
        object.insert("usageScope".to_string(), json!("request-response"));
    }
    if !attachments.is_empty()
        && let Some(object) = message.as_object_mut()
    {
        object.insert("attachments".to_string(), Value::Array(attachments));
    }
    Some(message)
}

fn cursor_bubble_attachments(bubble: &Value) -> Vec<Value> {
    [
        "attachments",
        "attachedFiles",
        "attachedFileCodeChunks",
        "fileSelections",
        "images",
        "imageAttachments",
    ]
    .iter()
    .filter_map(|key| bubble.get(*key))
    .flat_map(|value| match value {
        Value::Array(items) => items.clone(),
        Value::Null => Vec::new(),
        other => vec![other.clone()],
    })
    .collect()
}

pub(super) fn cursor_bubble_role(bubble: &Value) -> Option<&'static str> {
    match bubble.get("type").and_then(Value::as_i64) {
        Some(1) => Some("user"),
        Some(2) | Some(0) => Some("agent"),
        _ => {
            let role = extract_role(bubble);
            if role == "user" {
                Some("user")
            } else if role == "agent" || role == "assistant" {
                Some("agent")
            } else {
                None
            }
        }
    }
}

pub(super) fn cursor_composer_model_from_config(json: &Value) -> String {
    if let Some(selected) = json
        .pointer("/modelConfig/selectedModels")
        .and_then(Value::as_array)
    {
        for item in selected {
            let Some(model_id) = item
                .get("modelId")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|value| !value.is_empty())
            else {
                continue;
            };
            if !model_id.eq_ignore_ascii_case("default") {
                return normalize_cursor_model_name(model_id);
            }
        }
    }
    normalize_cursor_model_name(
        json.pointer("/modelConfig/modelName")
            .and_then(Value::as_str)
            .or_else(|| json.get("modelName").and_then(Value::as_str))
            .or_else(|| json.get("model").and_then(Value::as_str))
            .unwrap_or("default"),
    )
}

pub(super) fn cursor_bubble_model(bubble: &Value) -> Option<String> {
    bubble
        .pointer("/modelInfo/modelName")
        .and_then(Value::as_str)
        .or_else(|| bubble.get("modelName").and_then(Value::as_str))
        .or_else(|| bubble.get("model").and_then(Value::as_str))
        .map(str::trim)
        .filter(|value| !value.is_empty() && !value.eq_ignore_ascii_case("default"))
        .map(normalize_cursor_model_name)
}

pub(super) fn cursor_bubble_usage(bubble: &Value, model: &str) -> Option<Value> {
    let token_count = bubble.get("tokenCount")?;
    let mut usage = UsageFields::default();
    collect_token_usage(token_count, 0, &mut usage);
    let mut json = usage.to_json()?;
    if let Some(object) = json.as_object_mut() {
        if object
            .get("totalTokens")
            .and_then(Value::as_u64)
            .unwrap_or(0)
            == 0
        {
            return None;
        }
        if !model.trim().is_empty() {
            object.insert("model".to_string(), json!(model));
        }
    }
    Some(json)
}

pub(super) fn normalize_cursor_model_name(value: &str) -> String {
    let trimmed = value.trim();
    if trimmed.is_empty() || trimmed.eq_ignore_ascii_case("default") {
        "cursor-auto".to_string()
    } else {
        trimmed.to_string()
    }
}
