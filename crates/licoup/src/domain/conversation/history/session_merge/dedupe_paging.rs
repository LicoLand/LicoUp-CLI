use std::collections::BTreeMap;

use serde_json::Value;

use super::super::HistoryPageConfig;

pub(crate) fn dedupe_history_sessions(sessions: Vec<Value>) -> Vec<Value> {
    collapse_sessions_by_native_identity(sessions)
}

/// Collapse sessions that share one native identity into the richest of them,
/// carrying over metadata the richest one lacks.
///
/// Cursor records the same conversation in the IDE store, the CLI chat store,
/// and the CLI project tree, and only some of them know the project directory.
/// This runs before the delegated-task merge: leaving the copies in place makes
/// every delegated task attach to an arbitrary copy, so the conversation the
/// user opens is missing most of its tasks.
pub(crate) fn collapse_sessions_by_native_identity(sessions: Vec<Value>) -> Vec<Value> {
    let mut kept = BTreeMap::<String, usize>::new();
    let mut merged = Vec::<Value>::with_capacity(sessions.len());
    for session in sessions {
        let identity = history_session_native_id(&session);
        if identity.is_empty() {
            merged.push(session);
            continue;
        }
        match kept.get(&identity).copied() {
            Some(index) => {
                if session_richness(&session) > session_richness(&merged[index]) {
                    let previous = std::mem::replace(&mut merged[index], session);
                    absorb_session_metadata(&mut merged[index], &previous);
                } else {
                    absorb_session_metadata(&mut merged[index], &session);
                }
            }
            None => {
                kept.insert(identity, merged.len());
                merged.push(session);
            }
        }
    }
    merged
}

/// Ordering key for choosing which recorded copy of a conversation to keep. The
/// copy with the most messages holds the most of the conversation; a known
/// project directory breaks ties.
fn session_richness(session: &Value) -> (usize, usize, usize, u8) {
    let materialized = session
        .get("messages")
        .and_then(Value::as_array)
        .map(Vec::len)
        .unwrap_or(0);
    let semantic = session
        .get("semantic")
        .and_then(Value::as_object)
        .map(|value| {
            value
                .values()
                .filter_map(Value::as_array)
                .map(Vec::len)
                .sum()
        })
        .unwrap_or(0);
    (
        history_session_message_count(session),
        materialized,
        semantic,
        u8::from(
            session
                .get("workingDirectory")
                .and_then(Value::as_str)
                .is_some_and(|value| !value.trim().is_empty()),
        ),
    )
}

/// Fields a discarded copy may know that the kept copy does not.
fn absorb_session_metadata(kept: &mut Value, discarded: &Value) {
    let Some(object) = kept.as_object_mut() else {
        return;
    };
    for field in [
        "workingDirectory",
        "model",
        "title",
        "createdAt",
        "updatedAt",
        "subagentTitle",
        "subagentType",
        "parentSessionId",
        "runtime",
        "archivePath",
        "archivedAt",
        "attachmentPath",
        "attachments",
        "semantic",
    ] {
        let present = object.get(field).is_some_and(value_is_present);
        if present {
            continue;
        }
        if let Some(value) = discarded.get(field).filter(|value| value_is_present(value)) {
            object.insert(field.to_string(), value.clone());
        }
    }
    for field in ["delegatedSubagent", "running", "archived"] {
        if discarded.get(field).and_then(Value::as_bool) == Some(true) {
            object.insert(field.to_string(), Value::Bool(true));
        }
    }
}

fn value_is_present(value: &Value) -> bool {
    match value {
        Value::Null => false,
        Value::String(text) => !text.trim().is_empty(),
        Value::Array(values) => !values.is_empty(),
        Value::Object(values) => !values.is_empty(),
        _ => true,
    }
}

pub(crate) fn paged_history_sessions(sessions: Vec<Value>, page: &HistoryPageConfig) -> Vec<Value> {
    if page.offset >= sessions.len() {
        return Vec::new();
    }
    let end = page
        .end()
        .map(|end| end.min(sessions.len()))
        .unwrap_or(sessions.len());
    sessions
        .into_iter()
        .skip(page.offset)
        .take(end.saturating_sub(page.offset))
        .collect()
}

pub(crate) fn history_session_dedupe_key(session: &Value) -> String {
    let adapter_id = history_session_adapter_id(session);
    let native_session_id = history_session_native_id(session);
    // Record-native identity is authoritative. Source paths are hydration
    // locations and must not split one Agent conversation into several rows.
    if !native_session_id.is_empty() {
        return format!("{adapter_id}\n{native_session_id}");
    }
    let source_path = session
        .get("sourcePath")
        .and_then(Value::as_str)
        .unwrap_or_default();
    format!("{adapter_id}\n{source_path}\n{native_session_id}")
}

pub(super) fn history_session_adapter_id(session: &Value) -> &str {
    session
        .get("adapterId")
        .and_then(Value::as_str)
        .unwrap_or_default()
}

pub(super) fn history_session_native_id(session: &Value) -> String {
    session
        .get("nativeSessionId")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or_default()
        .to_string()
}

pub(super) fn history_session_message_count(session: &Value) -> usize {
    session
        .get("messageCount")
        .and_then(Value::as_u64)
        .map(|value| value as usize)
        .or_else(|| {
            session
                .get("messages")
                .and_then(Value::as_array)
                .map(Vec::len)
        })
        .unwrap_or(0)
}
