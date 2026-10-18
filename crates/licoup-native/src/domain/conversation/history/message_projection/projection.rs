use std::path::Path;

use serde_json::{Value, json};
use time::{OffsetDateTime, format_description::well_known::Rfc3339};

use super::antigravity;
use super::generated_context::{
    background_context_prompt_text, extract_user_authored_text, extract_user_image_attachments,
    generated_control_text, strip_generated_context_blocks,
};
use super::semantic::{
    HistoryMessageKind, humanize_history_semantic, normalize_history_message_semantic,
    structured_name,
};
use super::structured_privacy::{
    sanitize_structured_event_text, sanitize_structured_label, structured_event_text,
    structured_reasoning_detail, structured_reasoning_summary,
};
use crate::domain::conversation::history::query_filter::{display_path, message_id};
use crate::domain::conversation::source_catalog::HistoryAdapter;

pub(in crate::domain::conversation::history) fn plain_history_message(
    adapter: HistoryAdapter,
    path: &Path,
    index: usize,
    block_index: usize,
    role: &str,
    text: &str,
    created_at: Option<String>,
) -> Option<Value> {
    let images = if matches!(role, "user" | "human") {
        extract_user_image_attachments(text)
    } else {
        Vec::new()
    };
    let text = clean_native_message_text(adapter, role, text);
    if text.is_none() && images.is_empty() {
        return None;
    }
    let mut message = json!({
        "id": native_history_message_id(adapter, path, index, block_index),
        "role": role,
        "text": text.unwrap_or_default(),
        "createdAt": created_at.unwrap_or_default(),
        "sourcePath": display_path(path)
    });
    if !images.is_empty()
        && let Some(object) = message.as_object_mut()
    {
        object.insert("images".to_string(), json!(images));
    }
    crate::domain::conversation_semantic::annotate_message_layer(
        &mut message,
        crate::domain::conversation_semantic::SemanticLayer::Thread,
    );
    Some(message)
}

pub(in crate::domain::conversation::history) fn structured_history_message(
    adapter: HistoryAdapter,
    path: &Path,
    index: usize,
    block_index: usize,
    kind: HistoryMessageKind,
    semantic: &str,
    value: &Value,
    created_at: Option<String>,
) -> Value {
    let normalized_semantic = normalize_history_message_semantic(semantic);
    let (role, card_type, default_title, subtitle, fallback, collapsed) = match kind {
        HistoryMessageKind::ToolCall => (
            "tool_call",
            "tool-call",
            "Tool call",
            "Native agent activity".to_string(),
            "Invocation details are hidden.",
            true,
        ),
        HistoryMessageKind::ToolResult => (
            "tool_result",
            "tool-result",
            "Tool result",
            "Native agent result".to_string(),
            "The native tool result was recorded.",
            true,
        ),
        HistoryMessageKind::Reasoning => (
            "reasoning",
            "reasoning",
            "Reasoning",
            "Sensitive details hidden".to_string(),
            "Reasoning details are redacted.",
            true,
        ),
        HistoryMessageKind::Metadata => (
            "metadata",
            "metadata",
            "Metadata",
            "Sensitive details hidden".to_string(),
            "Sensitive native metadata is hidden.",
            true,
        ),
        HistoryMessageKind::Error => (
            "error",
            "error",
            "Error",
            "Native agent error".to_string(),
            "The native agent reported an error.",
            false,
        ),
        _ => (
            "event",
            "event",
            "Native event",
            "Native agent event".to_string(),
            "Native event details are hidden.",
            true,
        ),
    };
    let title = structured_event_title(value, &normalized_semantic, default_title);
    // Reasoning prefers the recorded chain-of-thought detail; the provider
    // summary remains the collapsed headline preview (or the whole body when
    // no thinking was recorded by the provider).
    let reasoning_detail = if kind == HistoryMessageKind::Reasoning {
        structured_reasoning_detail(value).and_then(|text| sanitize_structured_event_text(&text))
    } else {
        None
    };
    let provider_summary = if kind == HistoryMessageKind::Reasoning {
        structured_reasoning_summary(value).and_then(|text| sanitize_structured_event_text(&text))
    } else {
        None
    };
    let provider_summary_visible = provider_summary.is_some() && reasoning_detail.is_none();
    let text = match (&reasoning_detail, &provider_summary) {
        (Some(detail), _) => detail.clone(),
        (None, Some(summary)) => summary.clone(),
        (None, None) => structured_event_text(kind, value, fallback),
    };
    let subtitle = if provider_summary_visible {
        "Reasoning summary".to_string()
    } else if reasoning_detail.is_some() {
        reasoning_detail_subtitle(provider_summary.as_deref())
    } else {
        subtitle
    };
    let mut message = json!({
        "id": native_history_message_id(adapter, path, index, block_index),
        "role": role,
        "text": text,
        "createdAt": created_at.unwrap_or_default(),
        "cardType": card_type,
        "cardTitle": title,
        "cardSubtitle": subtitle,
        "collapsed": collapsed,
        "sourcePath": display_path(path),
        "sourceItemType": normalized_semantic
    });
    if provider_summary_visible && let Some(object) = message.as_object_mut() {
        object.insert("providerSummary".to_string(), json!(true));
    }
    crate::domain::conversation_semantic::annotate_message_layer(
        &mut message,
        crate::domain::conversation_semantic::SemanticLayer::Execution,
    );
    message
}

fn structured_event_title(value: &Value, semantic: &str, fallback: &str) -> String {
    if let Some(name) = structured_name(value).and_then(sanitize_structured_label) {
        return name;
    }
    if matches!(
        semantic,
        "run-command"
            | "view-file"
            | "list-directory"
            | "grep-search"
            | "read-url-content"
            | "generate-image"
            | "code-action"
    ) {
        return humanize_history_semantic(semantic);
    }
    fallback.to_string()
}

/// When full thinking is recorded the provider summary becomes the collapsed
/// headline preview (single line, ≤96 chars) instead of the generic
/// "Sensitive details hidden" line.
fn reasoning_detail_subtitle(summary: Option<&str>) -> String {
    let Some(summary) = summary else {
        return String::new();
    };
    let first_line = summary
        .lines()
        .find(|line| !line.trim().is_empty())
        .unwrap_or(summary)
        .trim();
    if first_line.is_empty() {
        return String::new();
    }
    if first_line.chars().count() <= 96 {
        first_line.to_string()
    } else {
        format!("{}…", first_line.chars().take(93).collect::<String>())
    }
}

pub(in crate::domain::conversation::history) fn native_history_message_id(
    adapter: HistoryAdapter,
    path: &Path,
    index: usize,
    block_index: usize,
) -> String {
    let id = message_id(adapter.id(), path, index);
    if block_index == 0 {
        id
    } else {
        format!("{id}:{block_index}")
    }
}

pub(in crate::domain::conversation::history) fn native_message_timestamp() -> String {
    OffsetDateTime::now_utc()
        .format(&Rfc3339)
        .unwrap_or_else(|_| "1970-01-01T00:00:00Z".to_string())
}

pub(in crate::domain::conversation::history) fn clean_native_message_text(
    adapter: HistoryAdapter,
    role: &str,
    text: &str,
) -> Option<String> {
    let visible = if matches!(adapter, HistoryAdapter::Antigravity) {
        clean_antigravity_message_text(role, text)?
    } else if matches!(role, "user" | "human") {
        extract_user_authored_text(text)
    } else {
        strip_generated_context_blocks(text)
    };
    let trimmed = visible.trim();
    if trimmed.is_empty()
        || generated_control_text(trimmed)
        || antigravity::system_boilerplate_text(trimmed)
        || background_context_prompt_text(trimmed)
    {
        None
    } else {
        Some(trimmed.to_string())
    }
}

pub(super) fn clean_antigravity_message_text(role: &str, text: &str) -> Option<String> {
    let normalized_role = role.trim().to_ascii_lowercase();
    if !antigravity::message_role_is_visible(&normalized_role) {
        return None;
    }
    let visible = if matches!(normalized_role.as_str(), "user" | "human") {
        antigravity::extract_user_request(text)
    } else {
        antigravity::strip_system_messages(text)
    };
    let generic = if matches!(normalized_role.as_str(), "user" | "human") {
        extract_user_authored_text(&visible)
    } else {
        strip_generated_context_blocks(&visible)
    };
    Some(antigravity::strip_artifact_noise(
        &antigravity::strip_protocol_tags(&generic),
    ))
}
