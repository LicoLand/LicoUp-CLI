use std::path::Path;

use anyhow::{Result, anyhow};
use serde_json::Value;

pub(super) fn redact_path_ref(path: &str) -> String {
    let trimmed = path.trim();
    if trimmed.starts_with("fixture://") || !trimmed.contains('/') && !trimmed.contains('\\') {
        return trimmed.to_string();
    }
    Path::new(trimmed)
        .file_name()
        .and_then(|name| name.to_str())
        .map(str::to_string)
        .unwrap_or_else(|| "source.bin".to_string())
}

pub(super) fn sanitize_default_view_text(text: &str) -> String {
    let mut out = text.to_string();
    let replacements = [
        ("sk-", "[redacted-token]"),
        ("api_key", "[redacted-secret]"),
        ("Authorization: Bearer", "[redacted-auth]"),
        ("authorization: bearer", "[redacted-auth]"),
        (concat!("/", "Users", "/"), "[user-home]/"),
        (concat!("/", "home", "/"), "[user-home]/"),
        (concat!("C:", "\\", "Users", "\\"), "[user-home]/"),
        (concat!("c:", "\\", "users", "\\"), "[user-home]/"),
        ("<system>", "[system-context]"),
        ("<apps_instructions>", "[apps-instructions]"),
        ("\"arguments\":{", "[redacted-tool-args]{"),
        ("\"tool_input\":{", "[redacted-tool-input]{"),
    ];
    for (needle, replacement) in replacements {
        let needle_lower = needle.to_ascii_lowercase();
        loop {
            let lower = out.to_ascii_lowercase();
            let Some(index) = lower.find(&needle_lower) else {
                break;
            };
            let end = index + needle.len().min(out.len().saturating_sub(index));
            out.replace_range(index..end, replacement);
        }
    }
    out
}

pub(super) fn assert_no_default_view_leakage(value: &Value) -> Result<()> {
    let mut haystack = String::new();
    if let Some(thread) = value.get("thread").and_then(Value::as_array) {
        for event in thread {
            if let Some(text) = event.get("text").and_then(Value::as_str) {
                haystack.push_str(text);
                haystack.push('\n');
            }
        }
    }
    if let Some(execution) = value.get("execution").and_then(Value::as_array) {
        for event in execution {
            if let Some(text) = event.get("summary").and_then(Value::as_str) {
                haystack.push_str(text);
                haystack.push('\n');
            }
        }
    }
    let lower = haystack.to_ascii_lowercase();
    for needle in [
        "sk-",
        "api_key",
        "authorization: bearer",
        "/users/",
        concat!("/", "home", "/"),
        concat!("c:", "\\", "users", "\\"),
        "<system>",
        "<apps_instructions>",
    ] {
        if lower.contains(needle) {
            return Err(anyhow!(
                "semantic default layers must not expose sensitive marker `{needle}`"
            ));
        }
    }
    if lower.contains("\"arguments\":{")
        || lower.contains("\"tool_input\":{")
        || lower.contains("\"arguments\": [")
    {
        return Err(anyhow!(
            "semantic default layers must not expose full command/tool payloads"
        ));
    }
    Ok(())
}
