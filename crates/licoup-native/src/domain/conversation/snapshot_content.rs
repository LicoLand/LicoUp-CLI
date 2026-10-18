use super::snapshot_identity::text_value;
use serde_json::Value;

pub(crate) fn candidate_has_real_conversation(candidate: &Value) -> bool {
    if candidate
        .get("archiveDiscoveryHasConversation")
        .and_then(Value::as_bool)
        .unwrap_or(false)
    {
        return true;
    }
    candidate
        .get("messages")
        .and_then(Value::as_array)
        .map(|messages| messages.iter().any(message_has_real_conversation_content))
        .unwrap_or(false)
}

pub(crate) fn message_has_real_conversation_content(message: &Value) -> bool {
    let role = text_value(message, "role")
        .unwrap_or_default()
        .to_ascii_lowercase();
    let Some(text) = archive_message_text(message) else {
        return false;
    };
    if metadata_like_archive_text(&text) {
        return false;
    }
    if matches!(
        role.as_str(),
        "user" | "human" | "assistant" | "agent" | "model"
    ) {
        return true;
    }
    if matches!(role.as_str(), "transcript" | "record" | "") {
        return looks_like_archive_text_conversation(&text)
            || looks_like_archive_database_record(&text);
    }
    false
}

pub(crate) fn metadata_like_archive_text(text: &str) -> bool {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return true;
    }
    let lower = trimmed.to_ascii_lowercase();
    if lower.starts_with("cwd:")
        || lower.starts_with("workingdirectory:")
        || lower.starts_with("projectpath:")
        || lower.starts_with("codex event:")
        || lower.starts_with("<environment_context>")
    {
        return true;
    }
    let line_count = trimmed.lines().count().max(1);
    let key_value_lines = trimmed
        .lines()
        .filter(|line| {
            let line = line.trim();
            line.contains(':') && !line.contains(' ') && line.len() < 80
        })
        .count();
    key_value_lines == line_count && line_count <= 4
}

pub(crate) fn looks_like_archive_text_conversation(raw: &str) -> bool {
    let lower = raw.to_ascii_lowercase();
    let structured_text = looks_like_structured_archive_text(raw);
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

pub(crate) fn looks_like_structured_archive_text(raw: &str) -> bool {
    let trimmed = raw.trim_start();
    trimmed.starts_with('{') || trimmed.starts_with('[')
}

pub(crate) fn looks_like_archive_database_record(raw: &str) -> bool {
    let lower = raw.to_ascii_lowercase();
    lower.contains("message:")
        || lower.contains("messages:")
        || lower.contains("conversation:")
        || lower.contains("conversations:")
        || lower.contains("chat:")
        || lower.contains("chats:")
}

fn archive_message_text(message: &Value) -> Option<String> {
    for key in ["text", "content", "message"] {
        if let Some(text) = message.get(key).and_then(Value::as_str) {
            let trimmed = text.trim();
            if !trimmed.is_empty() {
                return Some(trimmed.to_string());
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn distinguishes_conversation_messages_from_environment_metadata() {
        assert!(!message_has_real_conversation_content(&json!({
            "role": "user",
            "text": "cwd:/workspace"
        })));
        assert!(message_has_real_conversation_content(&json!({
            "role": "user",
            "text": "Explain this module boundary"
        })));
        assert!(looks_like_archive_text_conversation(
            "user: question\nassistant: answer"
        ));
    }
}
