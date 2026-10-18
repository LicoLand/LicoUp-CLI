use anyhow::{Result, anyhow};
use serde_json::{Map, Value};

use super::model::{SEMANTIC_KIND, SEMANTIC_SCHEMA_VERSION};
use super::privacy::assert_no_default_view_leakage;

pub fn validate_semantic_conversation(value: &Value) -> Result<()> {
    let object = value
        .as_object()
        .ok_or_else(|| anyhow!("semantic conversation must be an object"))?;
    require_const_u64(object, "schemaVersion", SEMANTIC_SCHEMA_VERSION)?;
    require_const_str(object, "kind", SEMANTIC_KIND)?;
    require_const_bool(object, "readOnly", true)?;
    validate_privacy_defaults(object.get("privacyDefaults"))?;
    validate_thread(object.get("thread"))?;
    validate_execution(object.get("execution"))?;
    validate_artifacts(object.get("artifacts"))?;
    validate_audit(object.get("audit"))?;
    validate_raw(object.get("raw"))?;
    assert_no_default_view_leakage(value)?;
    Ok(())
}

fn validate_privacy_defaults(value: Option<&Value>) -> Result<()> {
    let object = value
        .and_then(Value::as_object)
        .ok_or_else(|| anyhow!("privacyDefaults required"))?;
    require_const_str(object, "defaultView", "thread")?;
    require_const_bool(object, "hideRawInDefaultView", true)?;
    require_const_bool(object, "hideAuditInDefaultView", true)?;
    require_const_bool(object, "redactPaths", true)?;
    require_const_bool(object, "redactTokens", true)?;
    require_const_bool(object, "redactFullCommandPayloads", true)?;
    Ok(())
}

fn validate_thread(value: Option<&Value>) -> Result<()> {
    let items = value
        .and_then(Value::as_array)
        .ok_or_else(|| anyhow!("thread must be an array"))?;
    for item in items {
        let object = item
            .as_object()
            .ok_or_else(|| anyhow!("thread event must be an object"))?;
        require_const_str(object, "layer", "thread")?;
        require_non_empty(object, "id")?;
        require_enum(object, "role", &["user", "assistant"])?;
        require_enum(object, "eventKind", &["user-message", "assistant-message"])?;
        require_non_empty(object, "text")?;
        require_string(object, "createdAt")?;
    }
    Ok(())
}

fn validate_execution(value: Option<&Value>) -> Result<()> {
    let items = value
        .and_then(Value::as_array)
        .ok_or_else(|| anyhow!("execution must be an array"))?;
    for item in items {
        let object = item
            .as_object()
            .ok_or_else(|| anyhow!("execution event must be an object"))?;
        require_const_str(object, "layer", "execution")?;
        require_non_empty(object, "id")?;
        require_enum(
            object,
            "eventKind",
            &[
                "tool-call",
                "tool-result",
                "terminal",
                "plan",
                "progress",
                "retry",
                "error",
                "reasoning",
                "event",
            ],
        )?;
        require_string(object, "title")?;
        require_string(object, "summary")?;
        require_string(object, "createdAt")?;
        if object.get("collapsed").and_then(Value::as_bool).is_none() {
            return Err(anyhow!("execution.collapsed must be boolean"));
        }
    }
    Ok(())
}

fn validate_artifacts(value: Option<&Value>) -> Result<()> {
    let items = value
        .and_then(Value::as_array)
        .ok_or_else(|| anyhow!("artifacts must be an array"))?;
    for item in items {
        let object = item
            .as_object()
            .ok_or_else(|| anyhow!("artifact must be an object"))?;
        require_const_str(object, "layer", "artifacts")?;
        require_non_empty(object, "id")?;
        require_enum(
            object,
            "kind",
            &[
                "file",
                "diff",
                "document",
                "summary",
                "index",
                "validation",
                "archive-path",
            ],
        )?;
        require_string(object, "label")?;
    }
    Ok(())
}

fn validate_audit(value: Option<&Value>) -> Result<()> {
    let object = value
        .and_then(Value::as_object)
        .ok_or_else(|| anyhow!("audit required"))?;
    for key in [
        "adapterId",
        "hostApp",
        "sourceKind",
        "nativeSessionId",
        "createdAt",
        "updatedAt",
    ] {
        require_string(object, key)?;
    }
    require_enum(object, "redactionStatus", &["applied", "partial", "none"])?;
    require_enum(object, "validationStatus", &["unchecked", "ok", "failed"])?;
    if object
        .get("parseWarnings")
        .and_then(Value::as_array)
        .is_none()
    {
        return Err(anyhow!("audit.parseWarnings must be an array"));
    }
    validate_evidence(object.get("sourceEvidence"))?;
    Ok(())
}

fn validate_raw(value: Option<&Value>) -> Result<()> {
    let object = value
        .and_then(Value::as_object)
        .ok_or_else(|| anyhow!("raw required"))?;
    let refs = object
        .get("evidenceRefs")
        .and_then(Value::as_array)
        .ok_or_else(|| anyhow!("raw.evidenceRefs must be an array"))?;
    if refs.is_empty() {
        return Err(anyhow!("raw.evidenceRefs must be non-empty"));
    }
    for evidence in refs {
        validate_evidence(Some(evidence))?;
    }
    Ok(())
}

fn validate_evidence(value: Option<&Value>) -> Result<()> {
    let object = value
        .and_then(Value::as_object)
        .ok_or_else(|| anyhow!("evidence ref required"))?;
    require_enum(
        object,
        "kind",
        &["jsonl", "json", "sqlite-row", "markdown", "text", "unknown"],
    )?;
    require_non_empty(object, "pathRef")?;
    require_non_empty(object, "contentHash")?;
    Ok(())
}

fn require_const_u64(object: &Map<String, Value>, key: &str, expected: u64) -> Result<()> {
    match object.get(key).and_then(Value::as_u64) {
        Some(value) if value == expected => Ok(()),
        _ => Err(anyhow!("{key} must be {expected}")),
    }
}

fn require_const_str(object: &Map<String, Value>, key: &str, expected: &str) -> Result<()> {
    match object.get(key).and_then(Value::as_str) {
        Some(value) if value == expected => Ok(()),
        _ => Err(anyhow!("{key} must be `{expected}`")),
    }
}

fn require_const_bool(object: &Map<String, Value>, key: &str, expected: bool) -> Result<()> {
    match object.get(key).and_then(Value::as_bool) {
        Some(value) if value == expected => Ok(()),
        _ => Err(anyhow!("{key} must be {expected}")),
    }
}

fn require_string(object: &Map<String, Value>, key: &str) -> Result<()> {
    match object.get(key).and_then(Value::as_str) {
        Some(_) => Ok(()),
        None => Err(anyhow!("{key} must be a string")),
    }
}

fn require_non_empty(object: &Map<String, Value>, key: &str) -> Result<()> {
    match object.get(key).and_then(Value::as_str) {
        Some(value) if !value.trim().is_empty() => Ok(()),
        _ => Err(anyhow!("{key} must be a non-empty string")),
    }
}

fn require_enum(object: &Map<String, Value>, key: &str, allowed: &[&str]) -> Result<()> {
    match object.get(key).and_then(Value::as_str) {
        Some(value) if allowed.contains(&value) => Ok(()),
        Some(value) => Err(anyhow!("{key} has unsupported value `{value}`")),
        None => Err(anyhow!("{key} must be a string enum")),
    }
}
