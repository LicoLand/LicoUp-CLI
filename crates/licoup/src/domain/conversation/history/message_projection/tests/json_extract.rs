use serde_json::{Map, Value, json};

use super::super::json_extract::{
    extract_native_session_id, extract_role, extract_text, extract_timestamp, find_string,
};
use super::drop_json_iteratively;

#[test]
fn recursive_json_extraction_keeps_text_and_rejects_tool_or_system_payloads() {
    assert_eq!(
        extract_text(&json!({"message": {"content": {"type": "text", "text": "visible"}}}))
            .as_deref(),
        Some("visible")
    );
    assert!(
        extract_text(&json!({
            "content": {"type": "tool_result", "text": "must not project"}
        }))
        .is_none()
    );
    assert_eq!(
        extract_text(&Value::String(
            r#"{"type":"text","text":"decoded"}"#.to_string()
        ))
        .as_deref(),
        Some("decoded")
    );
}

#[test]
fn extraction_and_string_lookup_cross_four_thousand_nested_nodes() {
    let mut value = json!({"role": "agent", "text": "deep text"});
    for _ in 0..4_097 {
        let mut wrapper = Map::new();
        wrapper.insert("message".to_string(), value);
        value = Value::Object(wrapper);
    }
    assert_eq!(extract_text(&value).as_deref(), Some("deep text"));
    assert_eq!(find_string(&value, &["role"]).as_deref(), Some("agent"));
    drop_json_iteratively(value);
}

#[test]
fn role_time_and_session_projection_are_stable() {
    let value = json!({
        "type": 1,
        "created_at": "2026-01-01T00:00:00Z",
        "message": {"session_id": "session-1"}
    });
    assert_eq!(extract_role(&value), "user");
    assert_eq!(
        extract_timestamp(&value).as_deref(),
        Some("2026-01-01T00:00:00Z")
    );
    assert_eq!(
        extract_native_session_id(&value).as_deref(),
        Some("session-1")
    );
}
