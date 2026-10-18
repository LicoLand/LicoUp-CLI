use serde_json::{Map, Value};

pub(crate) fn filter_json_session(value: &Value, native_id: &str) -> Option<Value> {
    if extract_native_session_id(value).as_deref() == Some(native_id) || native_id == "file" {
        return Some(value.clone());
    }
    let object = value.as_object()?;
    for key in ["sessions", "conversations", "chats", "chatSessions"] {
        if let Some(items) = object.get(key).and_then(Value::as_array) {
            let selected = items
                .iter()
                .filter(|item| extract_native_session_id(item).as_deref() == Some(native_id))
                .cloned()
                .collect::<Vec<_>>();
            if !selected.is_empty() {
                let mut out = Map::<String, Value>::new();
                out.insert(key.to_string(), Value::Array(selected));
                return Some(Value::Object(out));
            }
        }
    }
    None
}

pub(crate) fn native_identity(session: &Value) -> String {
    let source_client = text_value(session, "sourceClient")
        .or_else(|| text_value(session, "sourceTool"))
        .or_else(|| text_value(session, "adapterId"))
        .or_else(|| text_value(session, "agentId"))
        .unwrap_or_else(|| "unknown".to_string());
    let native_id = text_value(session, "nativeSessionId").unwrap_or_else(|| "file".to_string());
    let source_path = text_value(session, "sourcePath").unwrap_or_default();
    format!("{}:{}:{}", source_client, source_path, native_id)
}

pub(crate) fn candidate_id(value: &Value) -> Option<String> {
    text_value(value, "id").filter(|id| !id.trim().is_empty())
}

pub(crate) fn extract_native_session_id(value: &Value) -> Option<String> {
    find_string(
        value,
        &[
            "sessionId",
            "session_id",
            "conversationId",
            "conversation_id",
            "chatId",
            "chat_id",
            "threadId",
            "thread_id",
            "id",
        ],
    )
    .filter(|value| !value.trim().is_empty())
}

pub(crate) fn text_value(value: &Value, key: &str) -> Option<String> {
    value.get(key).and_then(Value::as_str).map(str::to_string)
}

fn find_string(value: &Value, keys: &[&str]) -> Option<String> {
    let object = value.as_object()?;
    for key in keys {
        if let Some(text) = object.get(*key).and_then(Value::as_str) {
            return Some(text.to_string());
        }
        if let Some(number) = object.get(*key).and_then(Value::as_i64) {
            return Some(number.to_string());
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn filters_multi_session_documents_by_native_identity() {
        let source = json!({
            "sessions": [
                {"sessionId": "one", "messages": []},
                {"conversation_id": "two", "messages": []}
            ]
        });

        let filtered = filter_json_session(&source, "two").unwrap();
        assert_eq!(filtered["sessions"].as_array().unwrap().len(), 1);
        assert_eq!(filtered["sessions"][0]["conversation_id"], "two");
    }
}
