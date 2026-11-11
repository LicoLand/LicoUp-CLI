use std::sync::OnceLock;

use regex::Regex;

pub(super) fn message_role_is_visible(role: &str) -> bool {
    matches!(
        role,
        "user" | "human" | "planner_response" | "agent" | "assistant" | "generic"
    )
}

pub(in crate::domain::conversation::history) fn extract_user_request(text: &str) -> String {
    let cleaned = strip_system_messages(text);
    let requests = user_request_regex()
        .captures_iter(&cleaned)
        .filter_map(|capture| capture.get(1).map(|match_| match_.as_str()))
        .map(strip_protocol_tags)
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .collect::<Vec<_>>();
    if requests.is_empty() {
        strip_protocol_tags(&cleaned)
    } else {
        requests.join("\n\n")
    }
}

pub(super) fn strip_system_messages(text: &str) -> String {
    let normalized = text.replace("\r\n", "\n").replace('\r', "\n");
    let without_blocks = system_block_regex()
        .replace_all(&normalized, "\n")
        .to_string();
    let without_paragraphs = without_blocks
        .split("\n\n")
        .filter(|paragraph| !system_boilerplate_text(paragraph))
        .collect::<Vec<_>>()
        .join("\n\n");
    let without_lines = without_paragraphs
        .lines()
        .filter(|line| !system_boilerplate_text(line))
        .collect::<Vec<_>>()
        .join("\n");
    strip_protocol_tags(&without_lines)
}

pub(super) fn strip_protocol_tags(text: &str) -> String {
    protocol_tag_regex()
        .replace_all(text, "")
        .trim()
        .to_string()
}

pub(in crate::domain::conversation::history) fn strip_artifact_noise(text: &str) -> String {
    let normalized = text.replace("\r\n", "\n").replace('\r', "\n");
    let lines = normalized.lines().collect::<Vec<_>>();
    if looks_like_artifact_dump(&lines) {
        return String::new();
    }
    lines
        .into_iter()
        .filter(|line| !internal_event_line(line))
        .map(strip_line_gutter)
        .collect::<Vec<_>>()
        .join("\n")
        .trim()
        .to_string()
}

pub(super) fn looks_like_artifact_dump(lines: &[&str]) -> bool {
    let non_blank = lines.iter().filter(|line| !line.trim().is_empty()).count();
    if non_blank < 6 {
        return false;
    }
    let gutter_lines = lines
        .iter()
        .filter(|line| line_gutter_regex().is_match(line))
        .count();
    gutter_lines >= 4 && gutter_lines * 100 / non_blank >= 35
}

pub(super) fn strip_line_gutter(line: &str) -> String {
    if ordered_list_line_regex().is_match(line) {
        return line.trim_end().to_string();
    }
    if let Some(capture) = line_gutter_regex().captures(line) {
        let indent = capture.get(1).map(|value| value.as_str()).unwrap_or("");
        let content = capture.get(2).map(|value| value.as_str()).unwrap_or("");
        format!("{indent}{content}").trim_end().to_string()
    } else {
        line.trim_end().to_string()
    }
}

pub(super) fn internal_event_line(line: &str) -> bool {
    matches!(
        line.trim().to_ascii_lowercase().as_str(),
        "conversation_history"
            | "user_input"
            | "planner_response"
            | "list_directory"
            | "view_file"
            | "grep_search"
            | "run_command"
            | "code_action"
            | "generate_image"
            | "read_url_content"
    )
}

pub(super) fn has_user_request_tag(text: &str) -> bool {
    user_request_regex().is_match(text)
}

pub(super) fn system_boilerplate_text(text: &str) -> bool {
    let lower = text.trim().to_ascii_lowercase();
    !lower.is_empty()
        && ((lower.contains("<system_message>") && lower.contains("not actually sent by the user"))
            || (lower.contains("not actually sent by the user")
                && lower.contains("important information to pay attention"))
            || lower.starts_with("the following is a <system_message>")
            || lower.starts_with("the following is a <system-message>"))
}

fn user_request_regex() -> &'static Regex {
    static REGEX: OnceLock<Regex> = OnceLock::new();
    REGEX.get_or_init(|| {
        Regex::new(r"(?is)<\s*USER[_-]?REQUEST\b[^>]*>(.*?)<\s*/\s*USER[_-]?REQUEST\s*>")
            .expect("valid Antigravity user request regex")
    })
}

fn system_block_regex() -> &'static Regex {
    static REGEX: OnceLock<Regex> = OnceLock::new();
    REGEX.get_or_init(|| {
        Regex::new(r"(?is)<\s*SYSTEM[_-]?MESSAGE\b[^>]*>.*?<\s*/\s*SYSTEM[_-]?MESSAGE\s*>")
            .expect("valid Antigravity system block regex")
    })
}

fn protocol_tag_regex() -> &'static Regex {
    static REGEX: OnceLock<Regex> = OnceLock::new();
    REGEX.get_or_init(|| {
        Regex::new(r"(?i)</?\s*(?:USER[_-]?REQUEST|SYSTEM[_-]?MESSAGE)\b[^>]*>")
            .expect("valid Antigravity protocol tag regex")
    })
}

fn line_gutter_regex() -> &'static Regex {
    static REGEX: OnceLock<Regex> = OnceLock::new();
    REGEX.get_or_init(|| {
        Regex::new(r"^(\s*)\d{1,6}\s*(?:[│|:]\s?|\s{2,})(.*)$")
            .expect("valid Antigravity line gutter regex")
    })
}

fn ordered_list_line_regex() -> &'static Regex {
    static REGEX: OnceLock<Regex> = OnceLock::new();
    REGEX.get_or_init(|| Regex::new(r"^\s*\d+[.)]\s+\S").expect("valid ordered list guard regex"))
}
