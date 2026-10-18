use serde_json::json;

use super::super::stable_order::{
    history_time_order_key, message_order_key, sort_sessions_by_updated_at,
};

#[test]
fn time_and_message_order_keys_accept_rfc3339_and_numeric_values() {
    let first = history_time_order_key("2026-01-01T00:00:00Z").unwrap();
    let second = history_time_order_key("2026-01-01T00:00:01Z").unwrap();
    assert!(second > first);
    assert_eq!(history_time_order_key("42"), Some(42));
    assert_eq!(message_order_key(&json!({"createdAt": 7})), Some(7));
    assert_eq!(history_time_order_key(""), None);
}

#[test]
fn session_sort_is_newest_first_with_deterministic_identity_ties() {
    let mut sessions = vec![
        json!({"adapterId": "pi", "nativeSessionId": "b", "sourcePath": "b", "updatedAt": 2}),
        json!({"adapterId": "pi", "nativeSessionId": "a", "sourcePath": "a", "updatedAt": 2}),
        json!({"adapterId": "pi", "nativeSessionId": "old", "sourcePath": "old", "updatedAt": 1}),
    ];
    sort_sessions_by_updated_at(&mut sessions);
    assert_eq!(sessions[0]["nativeSessionId"], "a");
    assert_eq!(sessions[1]["nativeSessionId"], "b");
    assert_eq!(sessions[2]["nativeSessionId"], "old");
}
