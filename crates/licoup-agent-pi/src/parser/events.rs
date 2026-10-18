use serde_json::{Value, json};

pub fn sanitized_event(message: &Value) -> Option<Value> {
    let event_type = message.get("type").and_then(Value::as_str)?;
    match event_type {
        "agent_start" | "agent_end" | "agent_settled" | "turn_start" | "turn_end"
        | "message_start" | "message_end" | "queue_update" | "compaction_start"
        | "compaction_end" | "auto_retry_start" | "auto_retry_end" | "extension_error" => {
            Some(json!({ "type": event_type }))
        }
        "message_update" => Some(json!({
            "type": event_type,
            "deltaType": message.pointer("/assistantMessageEvent/type").and_then(Value::as_str).unwrap_or("")
        })),
        "tool_execution_start" | "tool_execution_update" | "tool_execution_end" => Some(json!({
            "type": event_type,
            "toolCallId": message.get("toolCallId").and_then(Value::as_str).unwrap_or(""),
            "toolName": message.get("toolName").and_then(Value::as_str).unwrap_or(""),
            "isError": message.get("isError").and_then(Value::as_bool)
        })),
        _ => None,
    }
}

pub fn processing_evidence_kind(message: &Value) -> Option<&'static str> {
    match message.get("type").and_then(Value::as_str)? {
        "tool_execution_start" => Some("tool"),
        "agent_start" | "turn_start" | "message_start" | "compaction_start"
        | "auto_retry_start" | "queue_update" => Some("progress"),
        "message_update"
            if message
                .pointer("/assistantMessageEvent/type")
                .and_then(Value::as_str)
                == Some("thinking_start") =>
        {
            Some("reasoning")
        }
        _ => None,
    }
}
