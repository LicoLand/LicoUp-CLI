use anyhow::{Result, anyhow, ensure};
use regex::Regex;
use serde_json::{Value, json};
use std::sync::OnceLock;

use super::policy;

pub struct SnapshotRedaction {
    pub content: String,
    pub metadata: Value,
    pub evidence: Value,
}

pub fn redact_snapshot(content: &str, mut metadata: Value) -> Result<SnapshotRedaction> {
    let mut redacted_paths = Vec::<String>::new();
    let redacted_content = if let Ok(mut parsed) = serde_json::from_str::<Value>(content) {
        redact_json_value(&mut parsed, "$.content", 0, false, &mut redacted_paths)?;
        if redacted_paths.is_empty() {
            content.to_string()
        } else {
            serde_json::to_string_pretty(&parsed)
                .map_err(|_| anyhow!("redacted snapshot content could not be serialized"))?
        }
    } else {
        redact_text_content(content, &mut redacted_paths)?
    };
    redact_json_value(&mut metadata, "$.metadata", 0, true, &mut redacted_paths)?;
    Ok(SnapshotRedaction {
        content: redacted_content,
        metadata,
        evidence: redaction_metadata(redacted_paths),
    })
}

pub fn redact_activity_payload(mut payload: Value) -> Result<Value> {
    let mut redacted_paths = Vec::<String>::new();
    redact_json_value(&mut payload, "$.payload", 0, true, &mut redacted_paths)?;
    Ok(payload)
}

fn redaction_metadata(paths: Vec<String>) -> Value {
    json!({
        "policy": "known-credential-fields",
        "applied": !paths.is_empty(),
        "paths": paths
    })
}

fn redact_json_value(
    value: &mut Value,
    path: &str,
    depth: usize,
    redact_local_paths: bool,
    redacted_paths: &mut Vec<String>,
) -> Result<()> {
    ensure!(
        depth <= policy::MAX_REDACTION_DEPTH,
        "snapshot redaction nesting exceeds its bounded depth"
    );
    match value {
        Value::Object(map) => {
            for (key, child) in map.iter_mut() {
                let child_path = format_json_path(path, key);
                if is_sensitive_key(key) {
                    if !matches!(child, Value::Null) {
                        *child = Value::String(policy::REDACTED_SECRET.to_string());
                        record_path(redacted_paths, child_path)?;
                    }
                } else if redact_local_paths && is_local_path_key(key) {
                    if !matches!(child, Value::Null) {
                        *child = Value::String(policy::REDACTED_LOCAL_PATH.to_string());
                        record_path(redacted_paths, child_path)?;
                    }
                } else {
                    redact_json_value(
                        child,
                        &child_path,
                        depth + 1,
                        redact_local_paths,
                        redacted_paths,
                    )?;
                }
            }
        }
        Value::Array(items) => {
            for (index, child) in items.iter_mut().enumerate() {
                redact_json_value(
                    child,
                    &format!("{path}[{index}]"),
                    depth + 1,
                    redact_local_paths,
                    redacted_paths,
                )?;
            }
        }
        Value::String(text) => {
            let redacted = redact_sensitive_text_value(text);
            if redacted != *text {
                *text = redacted;
                record_path(redacted_paths, path.to_string())?;
            }
        }
        _ => {}
    }
    Ok(())
}

fn record_path(paths: &mut Vec<String>, path: String) -> Result<()> {
    ensure!(
        paths.len() < policy::MAX_REDACTION_PATHS,
        "snapshot redaction evidence exceeds its bounded size"
    );
    paths.push(path);
    Ok(())
}

fn format_json_path(base: &str, key: &str) -> String {
    if key
        .chars()
        .all(|ch| ch.is_ascii_alphanumeric() || ch == '_')
    {
        format!("{base}.{key}")
    } else {
        format!(
            "{base}[{}]",
            serde_json::to_string(key).unwrap_or_else(|_| "\"<key>\"".to_string())
        )
    }
}

fn is_sensitive_key(key: &str) -> bool {
    let normalized = normalize_key(key);
    if matches!(
        normalized.as_str(),
        "secretref" | "credentialref" | "credentialid" | "keyid"
    ) {
        return false;
    }
    normalized.contains("token")
        || normalized.contains("apikey")
        || normalized.contains("password")
        || normalized.contains("secret")
        || normalized.contains("authorization")
        || normalized.contains("authheader")
        || normalized.contains("privatekey")
        || normalized.contains("clientsecret")
        || normalized.contains("csrf")
}

fn is_local_path_key(key: &str) -> bool {
    let normalized = normalize_key(key);
    normalized.ends_with("path")
        || normalized.ends_with("root")
        || normalized.ends_with("directory")
        || normalized.ends_with("dir")
}

fn normalize_key(key: &str) -> String {
    key.chars()
        .filter(|ch| ch.is_ascii_alphanumeric())
        .flat_map(|ch| ch.to_lowercase())
        .collect()
}

fn redact_text_content(content: &str, redacted_paths: &mut Vec<String>) -> Result<String> {
    let mut private_key_block = false;
    let mut lines = Vec::with_capacity(content.lines().count());
    for (index, line) in content.split('\n').enumerate() {
        let upper = line.to_ascii_uppercase();
        if upper.contains("-----BEGIN") && upper.contains("PRIVATE KEY-----") {
            private_key_block = true;
        }
        let redacted = if private_key_block {
            policy::REDACTED_PRIVATE_KEY.to_string()
        } else {
            redact_sensitive_line_assignment(line)
                .unwrap_or_else(|| redact_sensitive_text_value(line))
        };
        if redacted != line {
            record_path(redacted_paths, format!("$.content.text.line{}", index + 1))?;
        }
        lines.push(redacted);
        if private_key_block && upper.contains("-----END") && upper.contains("PRIVATE KEY-----") {
            private_key_block = false;
        }
    }
    Ok(lines.join("\n"))
}

fn redact_sensitive_line_assignment(line: &str) -> Option<String> {
    let separator = line.find([':', '='])?;
    let key_part = &line[..separator];
    if !is_sensitive_key(key_part) {
        return None;
    }
    let rest = &line[separator + 1..];
    let leading_len = rest.len() - rest.trim_start().len();
    let leading = &rest[..leading_len];
    let value = rest[leading_len..].trim_end();
    let trailing_comma = value.ends_with(',');
    let quote = value
        .chars()
        .next()
        .filter(|ch| *ch == '"' || *ch == '\'')
        .unwrap_or('\0');
    let replacement = if quote == '\0' {
        policy::REDACTED_SECRET.to_string()
    } else {
        format!("{quote}{}{quote}", policy::REDACTED_SECRET)
    };
    Some(format!(
        "{}{}{}{}",
        &line[..separator + 1],
        leading,
        replacement,
        if trailing_comma { "," } else { "" }
    ))
}

fn redact_sensitive_text_value(value: &str) -> String {
    [
        (authorization_regex(), "$1<redacted-token>"),
        (bearer_regex(), "$1<redacted-token>"),
        (query_secret_regex(), "$1<redacted-secret>"),
    ]
    .into_iter()
    .fold(value.to_string(), |current, (regex, replacement)| {
        regex.replace_all(&current, replacement).to_string()
    })
}

fn authorization_regex() -> &'static Regex {
    static REGEX: OnceLock<Regex> = OnceLock::new();
    REGEX.get_or_init(|| {
        Regex::new(r#"(?i)\b(Authorization\s*:\s*Bearer\s+)[^\s"',;)\]}]+"#)
            .expect("authorization redaction regex must compile")
    })
}

fn bearer_regex() -> &'static Regex {
    static REGEX: OnceLock<Regex> = OnceLock::new();
    REGEX.get_or_init(|| {
        Regex::new(r#"(?i)\b(Bearer\s+)[A-Za-z0-9._~+/=-]+"#)
            .expect("bearer redaction regex must compile")
    })
}

fn query_secret_regex() -> &'static Regex {
    static REGEX: OnceLock<Regex> = OnceLock::new();
    REGEX.get_or_init(|| {
        Regex::new(
            r#"(?i)\b((?:access_token|refresh_token|id_token|api_key|apiKey|token|secret|password|client_secret)=)[^&\s"',;)\]}]+"#,
        )
        .expect("query secret redaction regex must compile")
    })
}
