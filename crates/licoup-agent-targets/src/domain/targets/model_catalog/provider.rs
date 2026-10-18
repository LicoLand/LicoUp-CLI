use super::*;

pub(super) fn provider_id_from_model_value(value: &Value) -> Option<String> {
    match value {
        Value::Object(object) => provider_id_from_model_object(object),
        _ => None,
    }
}

pub(super) fn provider_id_from_model_object(object: &Map<String, Value>) -> Option<String> {
    for key in [
        "providerID",
        "providerId",
        "provider_id",
        "providerKey",
        "provider_key",
    ] {
        let provider_id = object
            .get(key)
            .and_then(Value::as_str)
            .and_then(sanitize_option_name);
        if provider_id.is_some() {
            return provider_id;
        }
    }
    None
}

pub(super) fn provider_name_from_model_value(value: &Value) -> Option<String> {
    match value {
        Value::Object(object) => provider_name_from_model_object(object),
        _ => None,
    }
}

pub(super) fn provider_name_from_model_object(object: &Map<String, Value>) -> Option<String> {
    for key in [
        "providerName",
        "provider_name",
        "providerLabel",
        "provider_label",
        "vendor",
        "vendorName",
        "vendor_name",
        "owner",
        "provider",
    ] {
        let provider_name = object
            .get(key)
            .and_then(Value::as_str)
            .and_then(sanitize_option_name);
        if provider_name.is_some() {
            return provider_name;
        }
    }
    None
}

pub(super) fn provider_label_from_provider_id(provider_id: &str) -> Option<String> {
    let normalized = provider_id.trim();
    if normalized.is_empty() {
        return None;
    }
    let lower = normalized.to_ascii_lowercase();
    let label = match lower.as_str() {
        "anthropic" | "claude" => "Anthropic".to_string(),
        "chatgpt" | "openai" => "OpenAI".to_string(),
        "deepseek" => "DeepSeek".to_string(),
        "gemini" | "google" => "Google".to_string(),
        "kilo" => "Kilo".to_string(),
        "kimi" | "moonshot" => "Moonshot".to_string(),
        "kimi-code" => "Kimi Code".to_string(),
        "xai" => "xAI".to_string(),
        "nvidia" => "NVIDIA".to_string(),
        _ => normalized
            .split(['-', '_'])
            .filter(|part| !part.is_empty())
            .map(|part| {
                let mut chars = part.chars();
                match chars.next() {
                    Some(first) => {
                        let mut word = String::new();
                        word.extend(first.to_uppercase());
                        word.push_str(chars.as_str());
                        word
                    }
                    None => String::new(),
                }
            })
            .collect::<Vec<_>>()
            .join(" "),
    };
    sanitize_option_name(&label)
}
