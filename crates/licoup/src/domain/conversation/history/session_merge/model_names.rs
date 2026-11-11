use std::collections::BTreeSet;

use serde_json::Value;

const MAX_MODEL_DISCOVERY_DEPTH: usize = 8;
const MAX_MODEL_NAME_CHARS: usize = 160;
const MAX_MODEL_NAME_BYTES: usize = 160;

pub(in crate::domain::conversation::history) fn collect_history_model_names(
    value: &Value,
    names: &mut BTreeSet<String>,
    depth: usize,
) {
    if depth > MAX_MODEL_DISCOVERY_DEPTH {
        return;
    }
    match value {
        Value::Object(object) => {
            for (key, child) in object {
                if is_history_model_key(key) {
                    collect_history_model_name_value(child, names);
                    continue;
                }
                if matches!(
                    key.as_str(),
                    "messages" | "usage" | "metadata" | "payload" | "request" | "response"
                ) {
                    collect_history_model_names(child, names, depth + 1);
                }
            }
        }
        Value::Array(items) => {
            for item in items {
                collect_history_model_names(item, names, depth + 1);
            }
        }
        _ => {}
    }
}

pub(super) fn collect_history_model_name_value(value: &Value, names: &mut BTreeSet<String>) {
    match value {
        Value::String(value) => {
            if let Some(name) = sanitize_history_model_name(value) {
                names.insert(name);
            }
        }
        Value::Array(items) => {
            for item in items {
                collect_history_model_name_value(item, names);
            }
        }
        Value::Object(object) => {
            for key in [
                "displayName",
                "display_name",
                "label",
                "name",
                "model",
                "modelName",
                "model_name",
                "id",
                "modelId",
                "model_id",
            ] {
                if let Some(name) = object
                    .get(key)
                    .and_then(Value::as_str)
                    .and_then(sanitize_history_model_name)
                {
                    names.insert(name);
                    return;
                }
            }
        }
        _ => {}
    }
}

pub(super) fn is_history_model_key(key: &str) -> bool {
    let normalized = key
        .chars()
        .filter(|ch| ch.is_ascii_alphanumeric())
        .flat_map(|ch| ch.to_lowercase())
        .collect::<String>();
    matches!(
        normalized.as_str(),
        "model" | "modelid" | "modelname" | "modellabel" | "currentmodel" | "selectedmodel"
    )
}

pub(super) fn sanitize_history_model_name(value: &str) -> Option<String> {
    let trimmed = value.trim();
    if trimmed.is_empty()
        || trimmed.len() > MAX_MODEL_NAME_BYTES
        || trimmed.chars().count() > MAX_MODEL_NAME_CHARS
        || trimmed.contains('\n')
        || trimmed.contains('\r')
        || trimmed.starts_with('{')
        || trimmed.starts_with('[')
        || trimmed.starts_with('$')
        || trimmed.starts_with("http://")
        || trimmed.starts_with("https://")
    {
        return None;
    }
    Some(trimmed.to_string())
}
