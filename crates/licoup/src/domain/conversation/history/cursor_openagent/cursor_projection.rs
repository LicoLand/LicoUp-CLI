//! Cursor composer-metadata projection shared by the IDE-store catalog.

use serde_json::Value;

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

pub(super) fn normalize_cursor_model_name(value: &str) -> String {
    let trimmed = value.trim();
    if trimmed.is_empty() || trimmed.eq_ignore_ascii_case("default") {
        "cursor-auto".to_string()
    } else {
        trimmed.to_string()
    }
}
