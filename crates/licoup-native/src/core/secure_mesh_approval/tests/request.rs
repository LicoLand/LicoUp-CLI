use super::super::{evaluate_approval_request_json, list_approval_inbox_json};
use super::support::base_request;
use serde_json::json;

#[test]
fn approval_request_registers_and_lists_without_plaintext_detail() {
    let _ = evaluate_approval_request_json(&base_request()).unwrap();
    let inbox = list_approval_inbox_json(&json!({})).unwrap();
    assert_eq!(inbox["ok"], true);
    assert!(inbox["pendingCount"].as_u64().unwrap_or(0) >= 1);
    let serialized = serde_json::to_string(&inbox).unwrap();
    assert!(!serialized.contains("toolArguments"));
    assert!(!serialized.contains("plaintextDetail"));
}

#[test]
fn plaintext_detail_fields_are_rejected() {
    let mut request = base_request();
    request
        .as_object_mut()
        .unwrap()
        .insert("pendingOperationId".into(), json!("op-plain-1"));
    request
        .as_object_mut()
        .unwrap()
        .insert("toolArguments".into(), json!({"path": "/secret"}));
    assert!(evaluate_approval_request_json(&request).is_err());
}
