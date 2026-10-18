use std::collections::BTreeSet;
use std::path::Path;

use serde_json::{Value, json};

use super::antigravity::system_boilerplate_text;

pub(super) fn extract_user_authored_text(text: &str) -> String {
    let request_text = if let Some(index) = find_case_insensitive(text, "## My request for Codex:")
    {
        &text[index + "## My request for Codex:".len()..]
    } else if let Some(index) = find_case_insensitive(text, "## My request:") {
        &text[index + "## My request:".len()..]
    } else if let Some(index) = find_case_insensitive(text, "My request for Codex:") {
        &text[index + "My request for Codex:".len()..]
    } else {
        text
    };
    let unwrapped = unwrap_user_query(&strip_generated_context_blocks(request_text));
    strip_generated_context_blocks(&unwrapped)
}

/// Keep only the text between Cursor `<userquery>` wrappers. A missing close
/// tag fails closed: strip the raw markers and keep remaining visible text.
fn unwrap_user_query(text: &str) -> String {
    const PAIRS: [(&str, &str); 2] = [
        ("<userquery>", "</userquery>"),
        ("<user_query>", "</user_query>"),
    ];
    let lower = text.to_ascii_lowercase();
    let mut best_match: Option<(usize, usize)> = None;
    let mut unclosed_open: Option<(usize, usize)> = None;
    for (open, close) in PAIRS {
        let Some(open_at) = lower.find(open) else {
            continue;
        };
        let inner_start = open_at + open.len();
        if let Some(close_rel) = lower[inner_start..].find(close) {
            let inner_end = inner_start + close_rel;
            if best_match
                .map(|(start, _)| inner_start < start)
                .unwrap_or(true)
            {
                best_match = Some((inner_start, inner_end));
            }
        } else if unclosed_open.map(|(at, _)| open_at < at).unwrap_or(true) {
            unclosed_open = Some((open_at, inner_start));
        }
    }
    if let Some((start, end)) = best_match {
        return strip_userquery_markers(text[start..end].trim());
    }
    if let Some((open_at, inner_start)) = unclosed_open {
        let mut visible = String::with_capacity(text.len());
        visible.push_str(&text[..open_at]);
        visible.push_str(&text[inner_start..]);
        return strip_userquery_markers(&visible);
    }
    strip_userquery_markers(text)
}

fn strip_userquery_markers(text: &str) -> String {
    const MARKERS: [&str; 4] = [
        "<userquery>",
        "</userquery>",
        "<user_query>",
        "</user_query>",
    ];
    let mut result = text.to_string();
    loop {
        let lower = result.to_ascii_lowercase();
        let mut found: Option<(usize, usize)> = None;
        for marker in MARKERS {
            if let Some(at) = lower.find(marker) {
                if found.map(|(pos, _)| at < pos).unwrap_or(true) {
                    found = Some((at, marker.len()));
                }
            }
        }
        let Some((at, len)) = found else {
            break;
        };
        result.replace_range(at..at + len, "");
    }
    result
}

pub(in crate::domain::conversation::history) fn extract_user_image_attachments(
    text: &str,
) -> Vec<Value> {
    if find_case_insensitive(text, "## My request for Codex:").is_none()
        && find_case_insensitive(text, "## My request:").is_none()
        && find_case_insensitive(text, "My request for Codex:").is_none()
    {
        return Vec::new();
    }

    let mut paths = BTreeSet::<String>::new();
    let mut attachments = Vec::<Value>::new();
    for line in text.lines() {
        if attachments.len() >= 4 || !line.trim_start().to_ascii_lowercase().starts_with("<image") {
            continue;
        }
        let Some(path) = tag_attribute(line, "path").filter(|path| !path.trim().is_empty()) else {
            continue;
        };
        let Some(media_type) = image_media_type(&path) else {
            continue;
        };
        if !paths.insert(path.clone()) {
            continue;
        }
        let name = Path::new(&path)
            .file_name()
            .and_then(|value| value.to_str())
            .unwrap_or_default();
        attachments.push(json!({
            "mediaType": media_type,
            "path": path,
            "name": name,
        }));
    }
    attachments
}

fn tag_attribute(line: &str, attribute: &str) -> Option<String> {
    let lower = line.to_ascii_lowercase();
    let needle = format!("{attribute}=");
    let index = lower.find(&needle)?;
    let remainder = line[index + needle.len()..].trim_start();
    let first = remainder.chars().next()?;
    let value = if matches!(first, '\"' | '\'') {
        remainder[1..].split(first).next()?
    } else if first == '[' {
        remainder[1..].split(']').next()?
    } else {
        remainder
            .split(|character: char| character.is_whitespace() || character == '>')
            .next()?
    };
    Some(value.trim().to_string())
}

fn image_media_type(path: &str) -> Option<&'static str> {
    let extension = Path::new(path)
        .extension()
        .and_then(|value| value.to_str())?
        .to_ascii_lowercase();
    match extension.as_str() {
        "png" => Some("image/png"),
        "jpg" | "jpeg" => Some("image/jpeg"),
        "gif" => Some("image/gif"),
        "webp" => Some("image/webp"),
        _ => None,
    }
}

pub(in crate::domain::conversation::history) fn strip_generated_context_blocks(
    text: &str,
) -> String {
    let mut lines = Vec::<String>::new();
    let mut close_marker: Option<&'static str> = None;
    for line in text.lines() {
        let lower = line.trim_start().to_ascii_lowercase();
        if let Some(close) = close_marker {
            if generated_context_line_contains_close(&lower, close) {
                close_marker = None;
                if let Some(after) = trailing_text_after_context_close(line, close) {
                    if !after.trim().is_empty() {
                        lines.push(after);
                    }
                }
            }
            continue;
        }
        if lower.starts_with("# files mentioned by the user:") {
            continue;
        }
        if let Some(close) = generated_context_block_close_marker(&lower) {
            if generated_context_line_contains_close(&lower, close) {
                if let Some(after) = trailing_text_after_context_close(line, close) {
                    if !after.trim().is_empty() {
                        lines.push(after);
                    }
                }
            } else {
                close_marker = Some(close);
            }
            continue;
        }
        lines.push(line.to_string());
    }
    lines.join("\n")
}

fn trailing_text_after_context_close(line: &str, close: &str) -> Option<String> {
    let lower = line.to_ascii_lowercase();
    let close_lower = close.to_ascii_lowercase();
    lower
        .find(&close_lower)
        .map(|index| line[index + close.len()..].to_string())
}

fn generated_context_block_close_marker(lower_line: &str) -> Option<&'static str> {
    for (prefix, close) in [
        ("<command-name", "</command-name>"),
        ("<command", "</command>"),
        ("<image", "</image>"),
        ("<system_message", "</system_message>"),
        ("<system-message", "</system-message>"),
        ("<environment_context", "</environment_context>"),
        ("<app-context", "</app-context>"),
        ("<apps_instructions", "</apps_instructions>"),
        ("<apps-instructions", "</apps-instructions>"),
        ("<plugins_instructions", "</plugins_instructions>"),
        ("<recommended_plugins", "</recommended_plugins>"),
        ("<additional_metadata", "</additional_metadata>"),
        ("<timestamp", "</timestamp>"),
        ("<collaboration_mode", "</collaboration_mode>"),
        ("<permissions instructions", "</permissions instructions>"),
        ("<system", "</system>"),
        ("<developer", "</developer>"),
        ("<instructions", "</instructions>"),
        ("<local-command-caveat", "</local-command-caveat>"),
        ("<local-command-output", "</local-command-output>"),
        ("<local-command-stdout", "</local-command-stdout>"),
        ("<local-command-stderr", "</local-command-stderr>"),
    ] {
        if lower_line.starts_with(prefix) {
            return Some(close);
        }
    }
    None
}

fn generated_context_line_contains_close(lower_line: &str, close: &str) -> bool {
    lower_line.contains(close)
        || compact_context_marker(lower_line).contains(&compact_context_marker(close))
}

fn compact_context_marker(value: &str) -> String {
    value
        .chars()
        .filter(|character| !matches!(character, '_' | '-' | ' ' | '\t'))
        .collect()
}

pub(in crate::domain::conversation::history) fn background_context_prompt_text(text: &str) -> bool {
    let lower = text.trim_start().to_ascii_lowercase();
    system_boilerplate_text(text)
        || lower.starts_with("# agents.md instructions")
        || lower.starts_with("agents.md instructions")
        || lower.starts_with("<instructions>")
        || lower.starts_with("you are codex, a coding agent")
        || lower.starts_with("you are chatgpt")
        || lower.starts_with("knowledge cutoff:")
        || lower.starts_with("current date:")
        || lower.starts_with("filesystem sandboxing defines")
        || lower.starts_with("sandbox_mode")
        || lower.starts_with("<system")
        || lower.starts_with("<system_message")
        || lower.starts_with("<system-message")
        || lower.starts_with("<developer")
        || lower.starts_with("<app-context")
        || lower.starts_with("<apps_instructions")
        || lower.starts_with("<apps-instructions")
        || lower.starts_with("<environment_context")
        || lower.starts_with("<plugins_instructions")
        || lower.starts_with("<collaboration_mode")
}

pub(in crate::domain::conversation::history) fn generated_control_text(text: &str) -> bool {
    let lower = text.trim_start().to_ascii_lowercase();
    lower.starts_with("<local-command-caveat>")
        || lower.starts_with("<command-name")
        || lower.starts_with("<command")
        || lower.starts_with("<local-command-output>")
        || lower.starts_with("<local-command-stdout>")
        || lower.starts_with("<local-command-stderr>")
        || lower.starts_with("<local-command-exit-code>")
        || lower.starts_with("<local-command-timeout>")
        || lower.starts_with("<environment_context>")
        || lower.starts_with("<apps_instructions>")
        || lower.starts_with("<apps-instructions>")
        || lower.starts_with("<recommended_plugins")
        || lower.starts_with("<additional_metadata")
        || lower.starts_with("<plugins_instructions")
        || background_context_prompt_text(text)
        || (lower.contains("<local-command-caveat>") && lower.contains("do not respond"))
}

pub(in crate::domain::conversation::history) fn normalize_generated_metadata_message(
    message: &mut Value,
) -> bool {
    let role = message.get("role").and_then(Value::as_str).unwrap_or("");
    if !matches!(role, "user" | "human") {
        return false;
    }
    let Some((title, detail)) = message
        .get("text")
        .and_then(Value::as_str)
        .and_then(generated_metadata_envelope)
    else {
        return false;
    };
    let detail = super::structured_privacy::sanitize_structured_event_text(&detail)
        .unwrap_or_else(|| "Recorded by the native agent runtime.".to_string());
    let Some(object) = message.as_object_mut() else {
        return false;
    };
    object.insert("role".to_string(), Value::String("metadata".to_string()));
    object.insert("text".to_string(), Value::String(detail));
    object.insert(
        "cardType".to_string(),
        Value::String("metadata".to_string()),
    );
    object.insert("cardTitle".to_string(), Value::String(title.to_string()));
    object.insert(
        "cardSubtitle".to_string(),
        Value::String("Agent runtime metadata".to_string()),
    );
    object.insert("collapsed".to_string(), Value::Bool(true));
    object.insert(
        "sourceItemType".to_string(),
        Value::String("generated-metadata".to_string()),
    );
    crate::domain::conversation_semantic::annotate_message_layer(
        message,
        crate::domain::conversation_semantic::SemanticLayer::Execution,
    );
    true
}

fn generated_metadata_envelope(text: &str) -> Option<(&'static str, String)> {
    let trimmed = text.trim();
    let lower = trimmed.to_ascii_lowercase();
    for (tag, title) in [
        ("task-notification", "Task notification"),
        ("task-result", "Task result"),
        ("system-reminder", "System reminder"),
        ("ide-opened-file", "IDE context"),
        ("ide_opened_file", "IDE context"),
    ] {
        let open = format!("<{tag}>");
        let close = format!("</{tag}>");
        if lower.starts_with(&open) && lower.ends_with(&close) {
            let detail = if tag == "task-notification" || tag == "task-result" {
                task_metadata_summary(trimmed)
            } else {
                "Recorded by the native agent runtime.".to_string()
            };
            return Some((title, detail));
        }
    }
    None
}

fn task_metadata_summary(text: &str) -> String {
    let status = generated_tag_value(text, "status");
    let summary = generated_tag_value(text, "summary");
    match (status, summary) {
        (Some(status), Some(summary)) => format!("Status: {status}\n{summary}"),
        (Some(status), None) => format!("Status: {status}"),
        (None, Some(summary)) => summary,
        (None, None) => "Recorded by the native agent runtime.".to_string(),
    }
}

fn generated_tag_value(text: &str, tag: &str) -> Option<String> {
    let lower = text.to_ascii_lowercase();
    let open = format!("<{tag}>");
    let close = format!("</{tag}>");
    let start = lower.find(&open)? + open.len();
    let end = lower[start..].find(&close)? + start;
    let value = text[start..end].trim();
    (!value.is_empty()).then(|| value.to_string())
}

fn find_case_insensitive(haystack: &str, needle: &str) -> Option<usize> {
    haystack
        .to_ascii_lowercase()
        .find(&needle.to_ascii_lowercase())
}
