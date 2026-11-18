use std::collections::BTreeSet;

use serde_json::json;

use super::super::model_names::{
    collect_history_model_names, is_history_model_key, sanitize_history_model_name,
};

#[test]
fn model_discovery_is_bounded_to_known_containers_and_normalized_keys() {
    let value = json!({
        "messages": [{"metadata": {"model_id": "model-b"}}],
        "usage": {"selectedModel": {"displayName": "model-a"}},
        "unrelated": {"model": "must-not-traverse"}
    });
    let mut names = BTreeSet::new();
    collect_history_model_names(&value, &mut names, 0);
    assert_eq!(
        names.into_iter().collect::<Vec<_>>(),
        vec!["model-a".to_string(), "model-b".to_string()]
    );
    assert!(is_history_model_key("selected_model"));
    assert!(!is_history_model_key("modelingStatus"));
}

#[test]
fn model_name_sanitizer_rejects_urls_payloads_and_oversized_values() {
    assert_eq!(
        sanitize_history_model_name(" model-a ").as_deref(),
        Some("model-a")
    );
    assert!(sanitize_history_model_name("https://example.invalid/model").is_none());
    assert!(sanitize_history_model_name("{\"model\":\"private\"}").is_none());
    assert!(sanitize_history_model_name(&"x".repeat(161)).is_none());
}
