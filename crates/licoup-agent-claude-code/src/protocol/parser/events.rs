use serde_json::{Value, json};

pub fn partial_text_delta(message: &Value) -> Option<&str> {
    (message.get("type").and_then(Value::as_str) == Some("stream_event"))
        .then(|| message.pointer("/event/delta/text").and_then(Value::as_str))
        .flatten()
}

pub fn processing_evidence_kind(message: &Value) -> Option<&'static str> {
    let message_type = message.get("type").and_then(Value::as_str)?;
    if message_type == "assistant" {
        let blocks = message
            .pointer("/message/content")
            .and_then(Value::as_array)?;
        if blocks.iter().any(|block| {
            matches!(
                block.get("type").and_then(Value::as_str),
                Some("tool_use" | "server_tool_use")
            )
        }) {
            return Some("tool");
        }
        if blocks.iter().any(|block| {
            matches!(
                block.get("type").and_then(Value::as_str),
                Some("thinking" | "redacted_thinking")
            )
        }) {
            return Some("reasoning");
        }
    }
    if message_type == "stream_event" {
        let event_type = message.pointer("/event/type").and_then(Value::as_str);
        let block_type = message
            .pointer("/event/content_block/type")
            .and_then(Value::as_str);
        if event_type == Some("content_block_start")
            && matches!(block_type, Some("tool_use" | "server_tool_use"))
        {
            return Some("tool");
        }
        if event_type == Some("content_block_start")
            && matches!(block_type, Some("thinking" | "redacted_thinking"))
        {
            return Some("reasoning");
        }
        if matches!(event_type, Some("message_start" | "content_block_start")) {
            return Some("progress");
        }
    }
    None
}

/// The name of the first tool the assistant used in this message (for example
/// `Bash`), so the client can label the evidence step instead of a bare
/// `tool`. Bounded to the tool name only; tool input stays local.
pub fn processing_tool_name(message: &Value) -> Option<&str> {
    let blocks = message
        .pointer("/message/content")
        .and_then(Value::as_array)?;
    for block in blocks {
        if matches!(
            block.get("type").and_then(Value::as_str),
            Some("tool_use" | "server_tool_use")
        ) {
            let name = block.get("name").and_then(Value::as_str)?;
            if !name.trim().is_empty() {
                return Some(name);
            }
        }
    }
    None
}

/// Retain only the structured evidence needed to replay a process-local turn.
/// Raw tool input, reasoning text, local paths, and vendor frames never enter
/// the transcript projection.
pub fn transcript_event(message: &Value) -> Option<Value> {
    if message.get("type").and_then(Value::as_str) == Some("system")
        && message.get("subtype").and_then(Value::as_str) == Some("permission_denied")
    {
        return Some(json!({"kind": "permissionDenied"}));
    }
    let evidence_kind = processing_evidence_kind(message)?;
    let mut event = json!({
        "kind": "processing",
        "evidenceKind": evidence_kind,
    });
    if let Some(tool_name) = processing_tool_name(message)
        && let Some(object) = event.as_object_mut()
    {
        object.insert("toolName".to_string(), json!(tool_name));
    }
    Some(event)
}
