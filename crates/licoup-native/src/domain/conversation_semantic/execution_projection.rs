use serde_json::{Value, json};

use crate::domain::conversation::event_semantics::execution_event_kind;

pub fn execution_wire_message_from_tagged(message: &Value) -> Option<Value> {
    let event = execution_event_from_message(message)?;
    let event_kind = event
        .get("eventKind")
        .and_then(Value::as_str)
        .unwrap_or("event");
    let source_item_type = event
        .get("sourceItemType")
        .and_then(Value::as_str)
        .unwrap_or("");
    let original_role = message.get("role").and_then(Value::as_str).unwrap_or("");
    let original_card = message
        .get("cardType")
        .and_then(Value::as_str)
        .unwrap_or("");
    let (role, card_type) = if original_card == "metadata" || original_role == "metadata" {
        ("metadata", "metadata")
    } else if !original_card.is_empty() {
        (original_role, original_card)
    } else {
        match event_kind {
            "tool-call" => ("tool_call", "tool-call"),
            "tool-result" => ("tool_result", "tool-result"),
            "terminal" => ("tool_call", "tool-call"),
            "reasoning" => ("reasoning", "reasoning"),
            "error" => ("error", "error"),
            _ => ("event", "event"),
        }
    };
    let subtitle = if role == "metadata" || role == "reasoning" {
        message
            .get("cardSubtitle")
            .and_then(Value::as_str)
            .unwrap_or("Sensitive details hidden")
    } else {
        message
            .get("cardSubtitle")
            .and_then(Value::as_str)
            .unwrap_or("Native agent activity")
    };
    let mut out = json!({
        "id": event.get("id").cloned().unwrap_or_else(|| json!("")),
        "layer": "execution",
        "role": role,
        "text": event.get("summary").cloned().unwrap_or_else(|| json!("")),
        "createdAt": event.get("createdAt").cloned().unwrap_or_else(|| json!("")),
        "cardType": card_type,
        "cardTitle": message.get("cardTitle").cloned().unwrap_or_else(|| event.get("title").cloned().unwrap_or_else(|| json!(""))),
        "cardSubtitle": subtitle,
        "collapsed": message.get("collapsed").cloned().unwrap_or_else(|| event.get("collapsed").cloned().unwrap_or_else(|| json!(true))),
        "eventKind": event_kind,
        "sourceItemType": source_item_type,
        "sourcePath": message.get("sourcePath").cloned().unwrap_or_else(|| json!(""))
    });
    if let Some(object) = out.as_object_mut() {
        for key in [
            "providerSummary",
            "usage",
            "usageScope",
            "model",
            "sourceEventType",
            "sourceTable",
            "sourceKey",
            "sourceFields",
            "sourceMessageId",
            "subagentPrompt",
            "subagentTitle",
        ] {
            if let Some(value) = message.get(key) {
                object.insert(key.to_string(), value.clone());
            }
        }
        if message.get("providerSummary") == Some(&json!(true)) {
            object.insert("cardSubtitle".to_string(), json!("Reasoning summary"));
        }
        if let Some(children) = message.get("messages") {
            object.insert("messages".to_string(), children.clone());
        }
    }
    Some(out)
}

pub(super) fn execution_event_from_message(message: &Value) -> Option<Value> {
    let card_type = message
        .get("cardType")
        .and_then(Value::as_str)
        .unwrap_or("");
    let source_item_type = message
        .get("sourceItemType")
        .and_then(Value::as_str)
        .unwrap_or("");
    let role = message.get("role").and_then(Value::as_str).unwrap_or("");
    let event_kind =
        if card_type == "metadata" || role == "metadata" || source_item_type == "metadata" {
            "event"
        } else {
            execution_event_kind(card_type, source_item_type)
        };
    let effective_source = if source_item_type.trim().is_empty() {
        if card_type == "metadata" || role == "metadata" {
            "metadata"
        } else if !card_type.is_empty() {
            card_type
        } else {
            role
        }
    } else {
        source_item_type
    };
    let title = message
        .get("cardTitle")
        .and_then(Value::as_str)
        .filter(|value| !value.trim().is_empty())
        .unwrap_or(if effective_source == "metadata" {
            "Metadata"
        } else {
            "Native event"
        });
    let summary = message
        .get("text")
        .and_then(Value::as_str)
        .unwrap_or("Native event details are hidden.");
    Some(json!({
        "id": message.get("id").cloned().unwrap_or_else(|| json!("")),
        "layer": "execution",
        "eventKind": event_kind,
        "title": title,
        "summary": summary,
        "createdAt": message.get("createdAt").cloned().unwrap_or_else(|| json!("")),
        "collapsed": message.get("collapsed").cloned().unwrap_or_else(|| json!(true)),
        "sourceItemType": effective_source,
        "providerSummary": message.get("providerSummary").cloned().unwrap_or_else(|| json!(false))
    }))
}

pub(super) fn append_timeline_messages(semantic: &Value, out: &mut Vec<Value>) {
    let Some(execution) = semantic.get("execution").and_then(Value::as_array) else {
        return;
    };
    for event in execution {
        let event_kind = event
            .get("eventKind")
            .and_then(Value::as_str)
            .unwrap_or("event");
        let source_item_type = event
            .get("sourceItemType")
            .and_then(Value::as_str)
            .unwrap_or("");
        let (role, card_type) = match (event_kind, source_item_type) {
            (_, "metadata") => ("metadata", "metadata"),
            ("tool-call", _) => ("tool_call", "tool-call"),
            ("tool-result", _) => ("tool_result", "tool-result"),
            ("terminal", _) => ("tool_call", "tool-call"),
            ("reasoning", _) => ("reasoning", "reasoning"),
            ("error", _) => ("error", "error"),
            ("plan" | "progress" | "retry", _) => ("event", "event"),
            _ if source_item_type == "metadata" || event_kind == "metadata" => {
                ("metadata", "metadata")
            }
            _ => ("event", "event"),
        };
        let mut message = json!({
            "id": event.get("id").cloned().unwrap_or_else(|| json!("")),
            "layer": "execution",
            "role": role,
            "text": event.get("summary").cloned().unwrap_or_else(|| json!("")),
            "createdAt": event.get("createdAt").cloned().unwrap_or_else(|| json!("")),
            "cardType": card_type,
            "cardTitle": event.get("title").cloned().unwrap_or_else(|| json!("")),
            "cardSubtitle": if role == "metadata" || role == "reasoning" {
                "Sensitive details hidden"
            } else {
                "Native agent activity"
            },
            "collapsed": event.get("collapsed").cloned().unwrap_or_else(|| json!(true)),
            "eventKind": event_kind,
            "sourceItemType": source_item_type
        });
        if event.get("providerSummary") == Some(&json!(true))
            && let Some(object) = message.as_object_mut()
        {
            object.insert("providerSummary".to_string(), json!(true));
            object.insert("cardSubtitle".to_string(), json!("Reasoning summary"));
        }
        out.push(message);
    }
}
