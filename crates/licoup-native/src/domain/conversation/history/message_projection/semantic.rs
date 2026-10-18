use serde_json::Value;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(in crate::domain::conversation::history) enum HistoryMessageKind {
    Text,
    ToolCall,
    ToolResult,
    Reasoning,
    Metadata,
    Error,
    Event,
}

pub(in crate::domain::conversation::history) fn history_message_kind_from_semantic(
    value: &str,
) -> HistoryMessageKind {
    let semantic = normalize_history_message_semantic(value);
    if semantic.is_empty()
        || matches!(
            semantic.as_str(),
            "text"
                | "input-text"
                | "output-text"
                | "markdown"
                | "message"
                | "summary-text"
                | "user"
                | "human"
                | "assistant"
                | "agent"
                | "model"
                | "ai"
                | "planner-response"
                | "generic"
        )
        || semantic.ends_with("-user-message")
        || semantic.ends_with("-assistant-message")
        || matches!(semantic.as_str(), "user-message" | "assistant-message")
    {
        return HistoryMessageKind::Text;
    }
    if semantic.contains("reasoning")
        || semantic.contains("analysis")
        || semantic.contains("thinking")
    {
        return HistoryMessageKind::Reasoning;
    }
    if semantic.contains("error")
        || semantic.contains("failure")
        || semantic.contains("failed")
        || semantic.contains("exception")
    {
        return HistoryMessageKind::Error;
    }
    if semantic == "metadata"
        || matches!(
            semantic.as_str(),
            "image" | "image-url" | "document" | "attachment" | "input-json-delta"
        )
    {
        return HistoryMessageKind::Metadata;
    }
    if matches!(
        semantic.as_str(),
        "tool-result"
            | "tool-output"
            | "function-result"
            | "function-output"
            | "function-call-output"
    ) || ((semantic.contains("tool") || semantic.contains("function"))
        && [
            "result",
            "output",
            "complete",
            "completed",
            "response",
            "end",
        ]
        .iter()
        .any(|marker| semantic.contains(marker)))
    {
        return HistoryMessageKind::ToolResult;
    }
    if matches!(
        semantic.as_str(),
        "tool"
            | "tool-call"
            | "tool-use"
            | "function"
            | "function-call"
            | "run-command"
            | "view-file"
            | "list-directory"
            | "grep-search"
            | "read-url-content"
            | "generate-image"
            | "code-action"
    ) || semantic.contains("tool")
        || semantic.contains("function")
    {
        return HistoryMessageKind::ToolCall;
    }
    HistoryMessageKind::Event
}

pub(in crate::domain::conversation::history) fn normalize_history_message_semantic(
    value: &str,
) -> String {
    let mut normalized = String::new();
    let mut separator = false;
    for character in value.trim().chars() {
        if character.is_ascii_alphanumeric() {
            if separator && !normalized.is_empty() {
                normalized.push('-');
            }
            separator = false;
            normalized.push(character.to_ascii_lowercase());
        } else {
            separator = true;
        }
    }
    normalized
}

pub(super) fn humanize_history_semantic(value: &str) -> String {
    let mut words = value.split('-').filter(|word| !word.is_empty());
    let Some(first) = words.next() else {
        return "Native event".to_string();
    };
    let mut label = first.to_string();
    if let Some(first_character) = label.get_mut(0..1) {
        first_character.make_ascii_uppercase();
    }
    for word in words {
        label.push(' ');
        label.push_str(word);
    }
    label
}

pub(super) fn structured_name(value: &Value) -> Option<&str> {
    [
        "name",
        "toolName",
        "tool_name",
        "functionName",
        "function_name",
        "tool",
    ]
    .iter()
    .find_map(|key| value.get(*key).and_then(Value::as_str))
}
