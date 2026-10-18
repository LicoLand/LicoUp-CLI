//! Query matching, text classification, stable identifiers, and time normalization.

use super::*;

pub(super) fn looks_like_text_conversation(raw: &str) -> bool {
    let lower = raw.to_ascii_lowercase();
    let structured_text = looks_like_structured_text(raw);
    let has_user_marker = (structured_text
        && (lower.contains("\"role\":\"user\"") || lower.contains("\"role\": \"user\"")))
        || lower.contains("role: user")
        || lower.lines().any(|line| {
            let line = line.trim_start();
            line.starts_with("user:")
                || line.starts_with("human:")
                || line.starts_with("prompt:")
                || line.starts_with("question:")
        });
    let has_response_marker = (structured_text
        && (lower.contains("\"role\":\"assistant\"")
            || lower.contains("\"role\": \"assistant\"")
            || lower.contains("\"role\":\"agent\"")
            || lower.contains("\"role\": \"agent\"")))
        || lower.contains("role: assistant")
        || lower.contains("role: agent")
        || lower.lines().any(|line| {
            let line = line.trim_start();
            line.starts_with("assistant:")
                || line.starts_with("agent:")
                || line.starts_with("response:")
                || line.starts_with("answer:")
        });
    if has_user_marker && has_response_marker {
        return true;
    }
    structured_text && lower.contains("\"messages\"") && (has_user_marker || has_response_marker)
}

pub(super) fn looks_like_structured_text(raw: &str) -> bool {
    let trimmed = raw.trim_start();
    trimmed.starts_with('{') || trimmed.starts_with('[')
}

pub(super) fn history_match_text(session: &Value) -> String {
    let mut parts = Vec::<String>::new();
    for key in ["title", "nativeSessionId"] {
        if let Some(text) = session.get(key).and_then(Value::as_str) {
            parts.push(text.to_string());
        }
    }
    if let Some(messages) = session.get("messages").and_then(Value::as_array) {
        for message in messages {
            if !history_message_is_matchable(message) {
                continue;
            }
            if let Some(text) = message.get("text").and_then(Value::as_str) {
                parts.push(text.to_string());
            }
        }
    }
    parts.join("\n")
}

pub(super) fn history_match_path_text(session: &Value) -> String {
    let mut parts = Vec::<String>::new();
    for key in ["sourcePath", "workingDirectory", "cwd", "projectPath"] {
        if let Some(text) = session.get(key).and_then(Value::as_str) {
            parts.push(text.to_string());
        }
    }
    if let Some(messages) = session.get("messages").and_then(Value::as_array) {
        for message in messages {
            for key in ["sourcePath", "workingDirectory", "cwd", "projectPath"] {
                if let Some(text) = message.get(key).and_then(Value::as_str) {
                    parts.push(text.to_string());
                }
            }
        }
    }
    parts.join("\n")
}

pub(super) fn source_path_is_sqlite(session: &Value) -> bool {
    let extension = session
        .get("sourcePath")
        .and_then(Value::as_str)
        .and_then(|path| Path::new(path).extension())
        .and_then(|value| value.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase();
    matches!(extension.as_str(), "sqlite" | "sqlite3" | "db" | "vscdb")
}

pub(super) fn history_message_is_matchable(message: &Value) -> bool {
    let role = message
        .get("role")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_ascii_lowercase();
    matches!(
        role.as_str(),
        "user" | "human" | "assistant" | "agent" | "model"
    ) || (matches!(role.as_str(), "transcript" | "")
        && message
            .get("text")
            .and_then(Value::as_str)
            .map(looks_like_text_conversation)
            .unwrap_or(false))
        || (role == "record"
            && message
                .get("text")
                .and_then(Value::as_str)
                .map(looks_like_database_record)
                .unwrap_or(false))
}

pub(super) fn history_session_has_real_conversation(session: &Value) -> bool {
    session
        .get("messages")
        .and_then(Value::as_array)
        .map(|messages| messages.iter().any(history_message_is_matchable))
        .unwrap_or(false)
}

pub(super) fn history_message_is_user_authored(message: &Value) -> bool {
    let role = message
        .get("role")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_ascii_lowercase();
    let text = message
        .get("text")
        .and_then(Value::as_str)
        .unwrap_or_default();
    if matches!(role.as_str(), "user" | "human") {
        return title_candidate_text(text);
    }
    if matches!(role.as_str(), "transcript" | "record" | "") {
        return title_from_conversation_marker(text).is_some();
    }
    false
}

pub(super) fn history_session_has_user_authored_message(session: &Value) -> bool {
    if session.get("adapterId").and_then(Value::as_str) == Some("copilot") {
        return true;
    }
    session
        .get("messages")
        .and_then(Value::as_array)
        .map(|messages| messages.iter().any(history_message_is_user_authored))
        .unwrap_or(false)
}

pub(super) fn looks_like_database_record(raw: &str) -> bool {
    let lower = raw.to_ascii_lowercase();
    lower.contains("message:")
        || lower.contains("messages:")
        || lower.contains("conversation:")
        || lower.contains("conversations:")
        || lower.contains("chat:")
        || lower.contains("chats:")
}

pub(super) fn normalize_history_match_text(value: &str) -> String {
    let mut out = String::new();
    let mut separator = false;
    for ch in value.chars() {
        if ch.is_whitespace() || ch == '-' || ch == '_' {
            separator = true;
            continue;
        }
        if separator && !out.is_empty() {
            out.push('-');
        }
        separator = false;
        for lower in ch.to_lowercase() {
            if lower.is_ascii_alphanumeric() || !lower.is_control() {
                out.push(lower);
            }
        }
    }
    out
}

pub(super) fn normalized_contains_history_term(normalized: &str, term: &str) -> bool {
    normalized.match_indices(term).any(|(index, _)| {
        let before = normalized[..index].chars().next_back();
        let after = normalized[index + term.len()..].chars().next();
        history_identity_boundary(before) && history_identity_boundary(after)
    })
}

pub(super) fn history_identity_boundary(ch: Option<char>) -> bool {
    ch.map(|ch| !ch.is_ascii_alphanumeric()).unwrap_or(true)
}

pub(super) fn extract_json_from_text(text: &str) -> Option<Value> {
    for part in text.split('\n') {
        let trimmed = part.trim();
        if let Some(start) = trimmed.find('{') {
            if let Ok(value) = serde_json::from_str::<Value>(&trimmed[start..]) {
                return Some(value);
            }
        }
        if let Some(start) = trimmed.find('[') {
            if let Ok(value) = serde_json::from_str::<Value>(&trimmed[start..]) {
                return Some(value);
            }
        }
    }
    None
}

pub(super) fn title_from_text(text: &str) -> String {
    let compact = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if compact.chars().count() <= 64 {
        compact
    } else {
        format!("{}...", compact.chars().take(64).collect::<String>())
    }
}

pub(super) fn session_id(agent_id: &str, path: &Path, native_session_id: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(agent_id.as_bytes());
    hasher.update(path.to_string_lossy().as_bytes());
    hasher.update(native_session_id.as_bytes());
    format!("native-{:x}", hasher.finalize())
}

pub(super) fn message_id(agent_id: &str, path: &Path, index: usize) -> String {
    let mut hasher = Sha256::new();
    hasher.update(agent_id.as_bytes());
    hasher.update(path.to_string_lossy().as_bytes());
    hasher.update(index.to_string().as_bytes());
    format!("msg-{:x}", hasher.finalize())
}

pub(super) fn system_time(time: SystemTime) -> String {
    let duration = time.duration_since(UNIX_EPOCH).unwrap_or_default();
    OffsetDateTime::from_unix_timestamp(duration.as_secs() as i64)
        .unwrap_or(OffsetDateTime::UNIX_EPOCH)
        .format(&Rfc3339)
        .unwrap_or_else(|_| "1970-01-01T00:00:00Z".to_string())
}

pub(super) fn epoch_value_to_rfc3339(value: &Value) -> Option<String> {
    value
        .as_i64()
        .or_else(|| {
            value
                .as_str()
                .and_then(|text| text.trim().parse::<i64>().ok())
        })
        .and_then(epoch_number_to_rfc3339)
}

pub(super) fn epoch_number_to_rfc3339(value: i64) -> Option<String> {
    if value <= 0 {
        return None;
    }
    let absolute = (value as i128).abs();
    let seconds = if absolute >= 1_000_000_000_000_000 {
        value / 1_000_000
    } else if absolute >= 10_000_000_000 {
        value / 1_000
    } else {
        value
    };
    OffsetDateTime::from_unix_timestamp(seconds)
        .ok()
        .and_then(|time| time.format(&Rfc3339).ok())
}

pub(super) fn display_path(path: &Path) -> String {
    path.to_string_lossy().to_string()
}
