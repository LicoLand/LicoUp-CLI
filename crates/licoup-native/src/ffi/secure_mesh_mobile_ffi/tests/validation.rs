use super::test_support::*;

#[test]
fn mobile_ffi_rejects_oversized_text_before_json_parsing() {
    let oversized = "x".repeat(MAX_FFI_REQUEST_BYTES + 1);
    let error = dispatch_json_with_files_dir(&oversized, "/unused", "unsupported")
        .unwrap_err()
        .to_string();
    assert!(error.contains("byte limit"));
    assert!(!error.contains(&oversized[..128]));
}

#[test]
fn mobile_ffi_rejects_deep_wide_and_oversized_string_values() {
    let mut deep = Value::Null;
    for _ in 0..=MAX_FFI_JSON_DEPTH {
        deep = json!({"nested": deep});
    }
    let deep_error = dispatch_json(&deep, "unsupported").unwrap_err().to_string();
    assert!(deep_error.contains("depth limit"));

    let mut fields = serde_json::Map::new();
    for index in 0..=MAX_FFI_OBJECT_FIELDS {
        fields.insert(format!("field-{index}"), Value::Null);
    }
    let wide_error = dispatch_json(&Value::Object(fields), "unsupported")
        .unwrap_err()
        .to_string();
    assert!(wide_error.contains("oversized object"));

    let oversized_string = json!({
        "action": "unsupported",
        "params": {"body": "x".repeat(MAX_FFI_STRING_BYTES + 1)}
    });
    let string_error = dispatch_json(&oversized_string, "unsupported")
        .unwrap_err()
        .to_string();
    assert!(string_error.contains("oversized string"));
}
