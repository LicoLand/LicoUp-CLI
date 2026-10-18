use serde_json::{Value, json};

pub(super) fn artifact_from_message(message: &Value) -> Value {
    let label = message
        .get("cardTitle")
        .or_else(|| message.get("text"))
        .and_then(Value::as_str)
        .unwrap_or("Artifact");
    json!({
        "id": message.get("id").cloned().unwrap_or_else(|| json!("")),
        "layer": "artifacts",
        "kind": "document",
        "label": label,
        "ref": message.get("sourcePath").and_then(Value::as_str).unwrap_or("artifact"),
        "contentHash": message.get("contentHash").cloned().unwrap_or_else(|| json!(""))
    })
}
