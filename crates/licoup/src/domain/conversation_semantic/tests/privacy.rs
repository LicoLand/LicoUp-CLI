use serde_json::json;

use super::super::privacy::{
    assert_no_default_view_leakage, redact_path_ref, sanitize_default_view_text,
};

#[test]
fn privacy_sanitizer_redacts_tokens_paths_context_and_tool_payload_markers() {
    let token = format!("{}{}", "sk", "-private");
    let home = format!("{}/{}", concat!("/", "Users"), "person/project");
    let text = format!(
        "{token} {home} <system>hidden</system> {}",
        r#"{"arguments":{"path":"private"}}"#
    );
    let sanitized = sanitize_default_view_text(&text);
    assert!(!sanitized.contains(&token));
    assert!(!sanitized.contains(&home));
    assert!(!sanitized.contains("<system>"));
    assert!(!sanitized.contains(r#""arguments":{"#));
    assert!(sanitized.contains("[redacted-token]"));
    assert!(sanitized.contains("[user-home]/"));
}

#[test]
fn privacy_validation_rejects_unsanitized_default_layers_and_redacts_path_refs() {
    let exposed = format!("{}/{}", concat!("/", "home"), "person/private");
    let semantic = json!({
        "thread": [{"text": exposed}],
        "execution": []
    });
    assert!(assert_no_default_view_leakage(&semantic).is_err());
    assert_eq!(redact_path_ref(&exposed), "private");
    assert_eq!(
        redact_path_ref("fixture://semantic/one.json"),
        "fixture://semantic/one.json"
    );
}
