use serde_json::{Value, json};

pub fn thread_wire_message_from_tagged(message: &Value) -> Option<Value> {
    let event = thread_event_from_message(message)?;
    let role = event
        .get("role")
        .and_then(Value::as_str)
        .unwrap_or("assistant");
    let wire_role = if role == "assistant" { "agent" } else { role };
    let original_role = message.get("role").and_then(Value::as_str).unwrap_or("");
    let wire_role = if matches!(original_role, "transcript" | "record") {
        original_role
    } else {
        wire_role
    };
    let mut out = json!({
        "id": event.get("id").cloned().unwrap_or_else(|| json!("")),
        "layer": "thread",
        "role": wire_role,
        "text": event.get("text").cloned().unwrap_or_else(|| json!("")),
        "createdAt": event.get("createdAt").cloned().unwrap_or_else(|| json!("")),
        "eventKind": event.get("eventKind").cloned().unwrap_or_else(|| json!("")),
        "sourcePath": message.get("sourcePath").cloned().unwrap_or_else(|| json!(""))
    });
    if let Some(object) = out.as_object_mut() {
        for key in [
            "images",
            "usage",
            "usageScope",
            "model",
            "sourceEventType",
            "sourceTable",
            "sourceKey",
            "sourceFields",
            "sourceMessageId",
            "sourceTurnId",
        ] {
            if let Some(value) = message.get(key) {
                object.insert(key.to_string(), value.clone());
            }
        }
    }
    Some(out)
}

pub(super) fn thread_event_from_message(message: &Value) -> Option<Value> {
    let raw_role = message.get("role").and_then(Value::as_str).unwrap_or("");
    let role = if matches!(raw_role, "transcript" | "record") {
        "user"
    } else {
        normalize_thread_role(raw_role)?
    };
    let text = message
        .get("text")
        .and_then(Value::as_str)
        .unwrap_or("")
        .trim();
    if text.is_empty() {
        return None;
    }
    Some(json!({
        "id": message.get("id").cloned().unwrap_or_else(|| json!("")),
        "layer": "thread",
        "role": role,
        "eventKind": if role == "user" { "user-message" } else { "assistant-message" },
        "text": text,
        "createdAt": message.get("createdAt").cloned().unwrap_or_else(|| json!("")),
        "sourceEventId": message.get("id").cloned().unwrap_or_else(|| json!(""))
    }))
}

pub(super) fn append_timeline_messages(semantic: &Value, out: &mut Vec<Value>) {
    let Some(thread) = semantic.get("thread").and_then(Value::as_array) else {
        return;
    };
    for event in thread {
        let role = event
            .get("role")
            .and_then(Value::as_str)
            .unwrap_or("assistant");
        let wire_role = if role == "assistant" { "agent" } else { role };
        out.push(json!({
            "id": event.get("id").cloned().unwrap_or_else(|| json!("")),
            "layer": "thread",
            "role": wire_role,
            "text": event.get("text").cloned().unwrap_or_else(|| json!("")),
            "createdAt": event.get("createdAt").cloned().unwrap_or_else(|| json!("")),
            "eventKind": event.get("eventKind").cloned().unwrap_or_else(|| json!(""))
        }));
    }
}

fn normalize_thread_role(role: &str) -> Option<&'static str> {
    match role.trim().to_ascii_lowercase().as_str() {
        "user" | "human" => Some("user"),
        "assistant" | "agent" | "model" | "ai" | "planner-response" | "planner_response"
        | "generic" => Some("assistant"),
        _ => None,
    }
}
