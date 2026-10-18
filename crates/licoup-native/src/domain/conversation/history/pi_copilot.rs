//! Pi and GitHub Copilot native history parsers.

use super::*;

pub(crate) fn parse_pi_session(
    path: &Path,
    source_kind: &str,
    metadata: &fs::Metadata,
) -> Option<Value> {
    let raw = fs::read_to_string(path).ok()?;
    let header = pi_session_header(path);
    let mut title = None::<String>;
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
            "session" => {}
            "session_info" => {
                if let Some(name) = value
                    .get("name")
                    .and_then(Value::as_str)
                    .filter(|value| !value.trim().is_empty())
                {
                    title = Some(name.to_string());
                }
            }
            "message" => {
                let Some(message) = value.get("message") else {
                    continue;
                };
                let first_projected = messages.len();
                let role = message
                    .get("role")
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                let normalized_role = match role {
                    "user" => "user",
                    "assistant" => "agent",
                    "toolResult" => "tool",
                    other if !other.is_empty() => other,
                    _ => continue,
                };
                if role == "assistant" {
                    if let Some(blocks) = message.get("content").and_then(Value::as_array) {
                        for (block_index, block) in blocks.iter().enumerate() {
                            let block_type =
                                block.get("type").and_then(Value::as_str).unwrap_or("");
                            if block_type == "thinking" {
                                if let Some(text) = block.get("thinking").and_then(Value::as_str) {
                                    if let Some(message) = plain_history_message(
                                        HistoryAdapter::Pi,
                                        path,
                                        index,
                                        block_index,
                                        "reasoning",
                                        text,
                                        extract_timestamp(&value),
                                    ) {
                                        messages.push(message);
                                    }
                                }
                                continue;
                            }
                            if block_type == "toolCall" {
                                messages.push(structured_history_message(
                                    HistoryAdapter::Pi,
                                    path,
                                    index,
                                    block_index,
                                    HistoryMessageKind::ToolCall,
                                    "tool",
                                    block,
                                    extract_timestamp(&value),
                                ));
                                continue;
                            }
                            if let Some(text) = extract_text(block) {
                                if let Some(message) = plain_history_message(
                                    HistoryAdapter::Pi,
                                    path,
                                    index,
                                    block_index,
                                    normalized_role,
                                    &text,
                                    extract_timestamp(&value),
                                ) {
                                    messages.push(message);
                                }
                            }
                        }
                        attach_native_usage_to_projection(
                            &mut messages,
                            first_projected,
                            HistoryAdapter::Pi,
                            path,
                            index,
                            &value,
                            message,
                        );
                        continue;
                    }
                }
                if let Some(text) = extract_text(message).or_else(|| extract_text(&value)) {
                    if let Some(mut projected) = plain_history_message(
                        HistoryAdapter::Pi,
                        path,
                        index,
                        0,
                        normalized_role,
                        &text,
                        extract_timestamp(&value).or_else(|| extract_timestamp(message)),
                    ) {
                        attach_native_usage(&mut projected, &value, message);
                        messages.push(projected);
                    }
                } else {
                    attach_native_usage_to_projection(
                        &mut messages,
                        first_projected,
                        HistoryAdapter::Pi,
                        path,
                        index,
                        &value,
                        message,
                    );
                }
            }
            _ => {}
        }
    }

    if messages.is_empty() {
        return None;
    }
    let mut session = session_from_messages_with_title(
        HistoryAdapter::Pi,
        path,
        metadata,
        source_kind,
        header.native_session_id,
        messages,
        title,
    );
    if let Some(object) = session.as_object_mut() {
        if let Some(created_at) = header.created_at {
            object.insert("createdAt".to_string(), json!(created_at));
        }
        if let Some(working_directory) = header.working_directory {
            object.insert("workingDirectory".to_string(), json!(working_directory));
        }
    }
    Some(session)
}

pub(crate) struct PiSessionHeader {
    pub(crate) native_session_id: String,
    pub(crate) created_at: Option<String>,
    pub(crate) working_directory: Option<String>,
}

/// One identity owner for Pi browse and exact reads: the native session header
/// wins, and only the known filename suffix is the fallback when no header id
/// exists.
pub(crate) fn pi_session_header(path: &Path) -> PiSessionHeader {
    let fallback = path
        .file_stem()
        .and_then(|value| value.to_str())
        .and_then(|stem| stem.rsplit_once('_').map(|(_, id)| id.to_string()))
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| "pi-session".to_string());
    let value = fs::File::open(path)
        .ok()
        .and_then(|file| BufReader::new(file).lines().next())
        .and_then(|line| line.ok())
        .and_then(|line| serde_json::from_str::<Value>(&line).ok())
        .filter(|value| value.get("type").and_then(Value::as_str) == Some("session"));
    let native_session_id = value
        .as_ref()
        .and_then(|value| value.get("id"))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
        .unwrap_or(fallback);
    let created_at = value
        .as_ref()
        .and_then(|value| value.get("timestamp"))
        .and_then(Value::as_str)
        .map(str::to_string);
    let working_directory = value
        .as_ref()
        .and_then(|value| value.get("cwd"))
        .and_then(Value::as_str)
        .and_then(super::project_workspace::bounded_project_workspace);
    PiSessionHeader {
        native_session_id,
        created_at,
        working_directory,
    }
}

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

pub(crate) fn parse_copilot_transcript_session(
    path: &Path,
    source_kind: &str,
    metadata: &fs::Metadata,
) -> Option<Value> {
    let raw = fs::read_to_string(path).ok()?;
    let native_session_id = copilot_transcript_session_id(path)
        .or_else(|| {
            path.file_stem()
                .and_then(|value| value.to_str())
                .filter(|value| !value.trim().is_empty())
                .map(str::to_string)
        })
        .unwrap_or_else(|| "copilot-transcript".to_string());
    let mut messages = Vec::<Value>::new();

    for (index, line) in raw.lines().enumerate() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        let Ok(value) = serde_json::from_str::<Value>(trimmed) else {
            continue;
        };
        let event_type = value
            .get("type")
            .and_then(Value::as_str)
            .unwrap_or_default();
        if event_type == "session.start" {
            continue;
        }
        let structured_kind = history_message_kind_from_semantic(event_type);
        if structured_kind != HistoryMessageKind::Text {
            let data = value.get("data").unwrap_or(&value);
            let mut projected = structured_history_message(
                HistoryAdapter::Copilot,
                path,
                index,
                0,
                structured_kind,
                event_type,
                data,
                extract_timestamp(&value),
            );
            attach_native_usage(&mut projected, &value, data);
            messages.push(projected);
            continue;
        }
        let role = match event_type {
            "user.message" => "user",
            "assistant.message" => "agent",
            _ => continue,
        };
        let data = value.get("data").unwrap_or(&value);
        let text = data
            .get("content")
            .and_then(Value::as_str)
            .map(str::to_string)
            .or_else(|| extract_text(data))
            .unwrap_or_default();
        let Some(text) = clean_native_message_text(HistoryAdapter::Copilot, role, &text) else {
            continue;
        };
        let created_at = extract_timestamp(&value).unwrap_or_else(|| {
            OffsetDateTime::now_utc()
                .format(&Rfc3339)
                .unwrap_or_else(|_| "1970-01-01T00:00:00Z".to_string())
        });
        let mut projected = json!({
            "id": message_id(HistoryAdapter::Copilot.id(), path, index),
            "role": role,
            "text": text,
            "createdAt": created_at,
            "sourcePath": display_path(path),
            "sourceEventType": event_type
        });
        attach_native_usage(&mut projected, &value, data);
        messages.push(projected);
    }

    if messages.is_empty() {
        return None;
    }
    Some(session_from_messages(
        HistoryAdapter::Copilot,
        path,
        metadata,
        source_kind,
        native_session_id,
        messages,
    ))
}

pub(crate) fn copilot_transcript_session_id(path: &Path) -> Option<String> {
    let reader = BufReader::new(fs::File::open(path).ok()?);
    reader
        .lines()
        .map_while(Result::ok)
        .filter_map(|line| serde_json::from_str::<Value>(line.trim()).ok())
        .find_map(|value| copilot_session_start_id(&value).map(str::to_string))
}

fn copilot_session_start_id(value: &Value) -> Option<&str> {
    if value.get("type").and_then(Value::as_str) != Some("session.start") {
        return None;
    }
    value
        .get("data")
        .and_then(|data| data.get("sessionId"))
        .and_then(Value::as_str)
        .filter(|value| !value.trim().is_empty())
}

fn attach_native_usage(target: &mut Value, event: &Value, payload: &Value) -> bool {
    let Some(usage) = extract_token_usage(payload).or_else(|| extract_token_usage(event)) else {
        return false;
    };
    let Some(object) = target.as_object_mut() else {
        return false;
    };
    object.insert("usage".to_string(), usage);
    object.insert("usageScope".to_string(), json!("request-response"));
    if let Some(model) = crate::domain::agent_usage::prefer_recorded_model(
        extract_native_model(payload),
        extract_native_model(event),
    ) {
        object.insert("model".to_string(), json!(model));
    }
    true
}

#[allow(clippy::too_many_arguments)]
fn attach_native_usage_to_projection(
    messages: &mut Vec<Value>,
    first_projected: usize,
    adapter: HistoryAdapter,
    path: &Path,
    index: usize,
    event: &Value,
    payload: &Value,
) {
    if first_projected < messages.len() {
        let projected = &messages[first_projected..];
        let offset = projected
            .iter()
            .rposition(|message| message.get("role").and_then(Value::as_str) == Some("agent"))
            .unwrap_or(projected.len() - 1);
        if attach_native_usage(&mut messages[first_projected + offset], event, payload) {
            return;
        }
    }
    let mut projected = json!({
        "id": native_history_message_id(adapter, path, index, 0),
        "role": "metadata",
        "text": format!("{} token usage", adapter.label()),
        "createdAt": extract_timestamp(event)
            .or_else(|| extract_timestamp(payload))
            .unwrap_or_else(native_message_timestamp),
        "sourcePath": display_path(path),
        "sourceEventType": "message.usage"
    });
    if attach_native_usage(&mut projected, event, payload) {
        messages.push(projected);
    }
}
