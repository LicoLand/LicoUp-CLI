//! Lico Agent native history parser.

use super::*;

pub(crate) fn parse_lico_agent_session(
    path: &Path,
    source_kind: &str,
    metadata: &fs::Metadata,
) -> Option<Value> {
    let raw = fs::read_to_string(path).ok()?;
    let mut native_session_id = path
        .file_stem()
        .and_then(|value| value.to_str())
        .filter(|value| !value.trim().is_empty())
        .map(str::to_string)
        .unwrap_or_else(|| "lico-agent-session".to_string());
    let mut working_directory = None::<String>;
    let mut messages = Vec::<Value>::new();

    for (index, line) in raw.lines().enumerate() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        let Ok(value) = serde_json::from_str::<Value>(trimmed) else {
            continue;
        };
        let entry_type = value
            .get("type")
            .and_then(Value::as_str)
            .unwrap_or_default();
        match entry_type {
            "session" => {
                if let Some(session_id) = value
                    .get("id")
                    .and_then(Value::as_str)
                    .filter(|value| !value.trim().is_empty())
                {
                    native_session_id = session_id.to_string();
                }
                if let Some(cwd) = value
                    .get("cwd")
                    .and_then(Value::as_str)
                    .filter(|value| !value.trim().is_empty())
                {
                    working_directory = Some(cwd.to_string());
                }
            }
            "usage.record" => {
                if let Some(usage) = extract_token_usage(&value) {
                    let mut message = json!({
                        "id": native_history_message_id(HistoryAdapter::LicoAgent, path, index, 0),
                        "role": "metadata",
                        "text": "Lico Agent token usage",
                        "createdAt": extract_timestamp(&value),
                        "sourcePath": display_path(path),
                        "sourceEventType": "usage.record",
                        "usageScope": "turn",
                        "usage": usage
                    });
                    if let Some(model) = extract_native_model(&value) {
                        message["model"] = json!(model);
                    }
                    messages.push(message);
                }
            }
            "message" => {
                let role = value
                    .get("role")
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                let normalized_role = match role {
                    "user" => "user",
                    "assistant" => "agent",
                    other if !other.is_empty() => other,
                    _ => continue,
                };
                let text = value
                    .get("text")
                    .and_then(Value::as_str)
                    .map(str::to_string)
                    .or_else(|| extract_text(&value))
                    .unwrap_or_default();
                if text.trim().is_empty() {
                    continue;
                }
                if let Some(mut message) = plain_history_message(
                    HistoryAdapter::LicoAgent,
                    path,
                    index,
                    0,
                    normalized_role,
                    &text,
                    extract_timestamp(&value),
                ) {
                    if let Some(usage) = extract_token_usage(&value) {
                        message["usage"] = usage;
                    }
                    if let Some(model) = extract_native_model(&value) {
                        message["model"] = json!(model);
                    }
                    messages.push(message);
                }
            }
            _ => {}
        }
    }

    if messages.is_empty() {
        return None;
    }
    let mut session = session_from_messages_with_title(
        HistoryAdapter::LicoAgent,
        path,
        metadata,
        source_kind,
        native_session_id,
        messages,
        None,
    );
    if let (Some(object), Some(cwd)) = (session.as_object_mut(), working_directory) {
        object.insert("workingDirectory".to_string(), Value::String(cwd));
    }
    Some(session)
}
