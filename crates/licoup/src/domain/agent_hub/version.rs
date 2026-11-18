//! Fail-closed Agent Hub version compare. Policy words are not versions.

use regex::Regex;
use semver::Version;
use std::sync::OnceLock;

const POLICY_TOKENS: &[&str] = &["latest", "latest-stable", "vendor-latest"];

pub fn is_policy_token(raw: &str) -> bool {
    let trimmed = raw.trim();
    POLICY_TOKENS
        .iter()
        .any(|token| trimmed.eq_ignore_ascii_case(token))
}

pub fn parse_comparable(raw: &str) -> Option<Version> {
    let trimmed = raw.trim();
    if trimmed.is_empty() || is_policy_token(trimmed) {
        return None;
    }
    if let Some(version) = parse_loose(trimmed) {
        return Some(version);
    }
    extract_version_token(trimmed)
        .as_deref()
        .and_then(parse_loose)
}

pub fn concrete_display(raw: &str) -> String {
    let trimmed = raw.trim();
    if trimmed.is_empty() || is_policy_token(trimmed) {
        return String::new();
    }
    let stripped = trimmed.trim_start_matches(['v', 'V']);
    if parse_loose(stripped).is_some() && !stripped.chars().any(char::is_whitespace) {
        return stripped.to_string();
    }
    let Some(token) = extract_version_token(trimmed) else {
        return String::new();
    };
    if parse_loose(&token).is_none() {
        return String::new();
    }
    token.trim_start_matches(['v', 'V']).to_string()
}

pub fn update_available(installed: &str, latest: &str) -> bool {
    match (parse_comparable(latest), parse_comparable(installed)) {
        (Some(latest), Some(installed)) => latest > installed,
        _ => false,
    }
}

fn parse_loose(raw: &str) -> Option<Version> {
    let value = raw.trim().trim_start_matches(['v', 'V']);
    if value.is_empty() || is_policy_token(value) {
        return None;
    }
    if let Ok(version) = Version::parse(value) {
        return Some(version);
    }
    let (core, suffix) = match value.find(['-', '+']) {
        Some(index) => (&value[..index], &value[index..]),
        None => (value, ""),
    };
    let parts = core.split('.').collect::<Vec<_>>();
    if !(1..=3).contains(&parts.len())
        || parts
            .iter()
            .any(|part| part.is_empty() || !part.chars().all(|ch| ch.is_ascii_digit()))
    {
        return None;
    }
    let mut padded = parts.join(".");
    while padded.bytes().filter(|byte| *byte == b'.').count() < 2 {
        padded.push_str(".0");
    }
    padded.push_str(suffix);
    Version::parse(&padded).ok()
}

fn extract_version_token(raw: &str) -> Option<String> {
    let regex = version_token_regex();
    regex
        .captures_iter(raw)
        .filter_map(|capture| capture.get(1).map(|item| item.as_str().to_string()))
        .find(|token| parse_loose(token).is_some())
}

fn version_token_regex() -> &'static Regex {
    static REGEX: OnceLock<Regex> = OnceLock::new();
    REGEX.get_or_init(|| {
        Regex::new(r"v?(\d+\.\d+(?:\.\d+)?(?:-[0-9A-Za-z.-]+)?(?:\+[0-9A-Za-z.-]+)?)")
            .expect("agent hub version token regex")
    })
}
