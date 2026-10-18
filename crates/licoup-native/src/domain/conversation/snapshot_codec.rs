use super::snapshot_content::{
    candidate_has_real_conversation, looks_like_archive_database_record,
    looks_like_structured_archive_text, metadata_like_archive_text,
};
use super::snapshot_identity::{
    extract_native_session_id, filter_json_session, native_identity, text_value,
};
use crate::domain::conversation_semantic::hash_text;
use anyhow::Result;
use rusqlite::Connection;
use serde_json::{Map, Value, json};
use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Clone, Debug)]
pub(crate) struct RawExport {
    pub(crate) file_name: String,
    pub(crate) content: String,
    pub(crate) export_kind: String,
    pub(crate) diagnostics: Vec<Value>,
}

pub(crate) fn export_raw_content(session: &Value) -> Result<RawExport> {
    let source_path = text_value(session, "sourcePath")
        .map(PathBuf::from)
        .unwrap_or_default();
    let native_id = text_value(session, "nativeSessionId").unwrap_or_else(|| "file".to_string());
    let extension = source_path
        .extension()
        .and_then(|value| value.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase();
    let export = if source_path.exists() {
        match extension.as_str() {
            "jsonl" | "ndjson" => export_jsonl_source(&source_path, &native_id),
            "json" => export_json_source(&source_path, &native_id),
            "sqlite" | "sqlite3" | "db" | "vscdb" => export_sqlite_source(&source_path, session),
            "md" | "markdown" => export_whole_file(&source_path, "source.md", "source-file"),
            "txt" | "log" => export_whole_file(&source_path, "source.txt", "source-file"),
            _ => export_whole_file(&source_path, "source.txt", "source-file"),
        }
    } else {
        Ok(RawExport {
            file_name: "source.json".to_string(),
            content: format!("{}\n", serde_json::to_string_pretty(session)?),
            export_kind: "parsed-session-source-missing".to_string(),
            diagnostics: vec![json!({
                "stage": "raw_export",
                "status": "source_missing",
                "sourcePath": source_path.to_string_lossy()
            })],
        })
    };
    export.map(|export| parsed_session_fallback_for_empty_raw(export, session))
}

pub(crate) fn export_jsonl_source(path: &Path, native_id: &str) -> Result<RawExport> {
    let raw = fs::read_to_string(path)?;
    let mut lines = filter_codex_rollout_jsonl_source(&raw, native_id);
    let export_kind = if lines.is_empty() {
        "jsonl-native-session-records".to_string()
    } else {
        "codex-rollout-jsonl-native-session-records".to_string()
    };
    if lines.is_empty() {
        for line in raw.lines() {
            let trimmed = line.trim();
            if trimmed.is_empty() {
                continue;
            }
            let matches = serde_json::from_str::<Value>(trimmed)
                .ok()
                .and_then(|value| extract_native_session_id(&value))
                .map(|id| id == native_id)
                .unwrap_or(native_id == "file");
            if matches {
                lines.push(line.to_string());
            }
        }
    }
    let diagnostics = if lines.is_empty() {
        vec![json!({
            "stage": "raw_export",
            "status": "filter_empty_used_full_source",
            "sourcePath": path.to_string_lossy()
        })]
    } else {
        Vec::new()
    };
    Ok(RawExport {
        file_name: "source.jsonl".to_string(),
        content: if lines.is_empty() {
            raw
        } else {
            format!("{}\n", lines.join("\n"))
        },
        export_kind,
        diagnostics,
    })
}

fn filter_codex_rollout_jsonl_source(raw: &str, native_id: &str) -> Vec<String> {
    if native_id.trim().is_empty() || native_id == "file" {
        return Vec::new();
    }
    let mut lines = Vec::<String>::new();
    let mut current_session_id: Option<String> = None;
    let mut saw_rollout = false;
    for line in raw.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        let Ok(value) = serde_json::from_str::<Value>(trimmed) else {
            continue;
        };
        let event_type = value
            .get("type")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let Some(payload) = value.get("payload") else {
            continue;
        };
        if !matches!(
            event_type,
            "session_meta" | "turn_context" | "response_item" | "event_msg"
        ) {
            continue;
        }
        saw_rollout = true;
        if event_type == "session_meta" {
            current_session_id = payload
                .get("id")
                .and_then(Value::as_str)
                .map(str::to_string);
        }
        if current_session_id.as_deref() == Some(native_id)
            && codex_rollout_raw_line_is_conversation(&value)
        {
            lines.push(line.to_string());
        }
    }
    if saw_rollout { lines } else { Vec::new() }
}

fn codex_rollout_raw_line_is_conversation(value: &Value) -> bool {
    if value.get("type").and_then(Value::as_str) != Some("response_item") {
        return false;
    }
    let Some(payload) = value.get("payload") else {
        return false;
    };
    if payload.get("type").and_then(Value::as_str) != Some("message") {
        return false;
    }
    if !matches!(
        payload
            .get("role")
            .and_then(Value::as_str)
            .unwrap_or_default(),
        "user" | "assistant" | "agent" | "model"
    ) {
        return false;
    }
    payload
        .get("content")
        .or_else(|| payload.get("text"))
        .and_then(archive_extract_text)
        .map(|text| !metadata_like_archive_text(&text))
        .unwrap_or(false)
}

fn archive_extract_text(value: &Value) -> Option<String> {
    match value {
        Value::String(text) => Some(text.clone()),
        Value::Array(items) => {
            let parts = items
                .iter()
                .filter_map(archive_extract_text)
                .filter(|text| !text.trim().is_empty())
                .collect::<Vec<_>>();
            if parts.is_empty() {
                None
            } else {
                Some(parts.join("\n"))
            }
        }
        Value::Object(object) => {
            for key in ["text", "content", "message", "prompt", "response", "answer"] {
                if let Some(text) = object.get(key).and_then(archive_extract_text)
                    && !text.trim().is_empty()
                {
                    return Some(text);
                }
            }
            None
        }
        _ => None,
    }
}

fn export_json_source(path: &Path, native_id: &str) -> Result<RawExport> {
    let raw = fs::read_to_string(path)?;
    let value = serde_json::from_str::<Value>(&raw).ok();
    if let Some(filtered) = value
        .as_ref()
        .and_then(|value| filter_json_session(value, native_id))
    {
        return Ok(RawExport {
            file_name: "source.json".to_string(),
            content: format!("{}\n", serde_json::to_string_pretty(&filtered)?),
            export_kind: "json-native-session-records".to_string(),
            diagnostics: Vec::new(),
        });
    }
    Ok(RawExport {
        file_name: "source.json".to_string(),
        content: raw,
        export_kind: "json-source-file".to_string(),
        diagnostics: vec![json!({
            "stage": "raw_export",
            "status": "json_filter_unavailable_used_full_source",
            "sourcePath": path.to_string_lossy()
        })],
    })
}

fn export_whole_file(path: &Path, file_name: &str, export_kind: &str) -> Result<RawExport> {
    Ok(RawExport {
        file_name: file_name.to_string(),
        content: fs::read_to_string(path)?,
        export_kind: export_kind.to_string(),
        diagnostics: Vec::new(),
    })
}

fn parsed_session_fallback_for_empty_raw(mut export: RawExport, session: &Value) -> RawExport {
    if raw_export_has_real_conversation(&export.content)
        || !candidate_has_real_conversation(session)
    {
        return export;
    }
    export.diagnostics.push(json!({
        "stage": "raw_export",
        "status": "parsed_session_used_because_raw_export_lacked_conversation_content",
        "previousExportKind": export.export_kind
    }));
    RawExport {
        file_name: "source.json".to_string(),
        content: format!(
            "{}\n",
            serde_json::to_string_pretty(session).unwrap_or_else(|_| "{}".to_string())
        ),
        export_kind: "parsed-session-raw-fallback".to_string(),
        diagnostics: export.diagnostics,
    }
}

fn raw_export_has_real_conversation(raw: &str) -> bool {
    let lower = raw.to_ascii_lowercase();
    let structured_text = looks_like_structured_archive_text(raw);
    (structured_text
        && (lower.contains("\"role\":\"user\"")
            || lower.contains("\"role\": \"user\"")
            || lower.contains("\"role\":\"assistant\"")
            || lower.contains("\"role\": \"assistant\"")
            || lower.contains("\"type\":\"user\"")
            || lower.contains("\"type\": \"user\"")))
        || (lower.contains("\"rows\"") && looks_like_archive_database_record(raw))
        || lower.lines().any(|line| {
            let line = line.trim_start();
            line.starts_with("user:")
                || line.starts_with("human:")
                || line.starts_with("assistant:")
                || line.starts_with("agent:")
        })
}

fn export_sqlite_source(path: &Path, session: &Value) -> Result<RawExport> {
    let messages = session
        .get("messages")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let mut rows = Vec::<Value>::new();
    let connection = Connection::open_with_flags(
        path,
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY
            | rusqlite::OpenFlags::SQLITE_OPEN_URI
            | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )?;
    let mut seen = BTreeSet::<(String, String)>::new();
    for message in messages {
        let table = text_value(&message, "sourceTable").unwrap_or_default();
        let key = text_value(&message, "sourceKey").unwrap_or_default();
        let fields = message.get("sourceFields").cloned();
        let identity = if key.is_empty() {
            fields
                .as_ref()
                .map(|value| hash_text(&serde_json::to_string(value).unwrap_or_default()))
                .unwrap_or_default()
        } else {
            key.clone()
        };
        if table.is_empty() || identity.is_empty() || !seen.insert((table.clone(), identity)) {
            continue;
        }
        if !key.is_empty() {
            if let Some(row) = sqlite_row_by_key(&connection, &table, &key)? {
                rows.push(row);
            }
        } else if let Some(fields) = fields {
            rows.push(json!({
                "table": table,
                "key": null,
                "fields": fields
            }));
        }
    }
    if rows.is_empty() {
        return Ok(RawExport {
            file_name: "source.sqlite-export.json".to_string(),
            content: format!(
                "{}\n",
                serde_json::to_string_pretty(&json!({
                    "sourcePath": path.to_string_lossy(),
                    "nativeConversationIdentity": native_identity(session),
                    "exportStatus": "parsed-session-only",
                    "session": session
                }))?
            ),
            export_kind: "sqlite-parsed-session-fallback".to_string(),
            diagnostics: vec![json!({
                "stage": "raw_export",
                "status": "sqlite_row_identity_unavailable",
                "sourcePath": path.to_string_lossy()
            })],
        });
    }
    Ok(RawExport {
        file_name: "source.sqlite-export.json".to_string(),
        content: format!(
            "{}\n",
            serde_json::to_string_pretty(&json!({
                "sourcePath": path.to_string_lossy(),
                "nativeConversationIdentity": native_identity(session),
                "rows": rows
            }))?
        ),
        export_kind: "sqlite-native-session-records".to_string(),
        diagnostics: Vec::new(),
    })
}

fn sqlite_row_by_key(connection: &Connection, table: &str, key: &str) -> Result<Option<Value>> {
    let escaped_table = table.replace('"', "\"\"");
    let query = format!(
        "SELECT * FROM \"{}\" WHERE key = ?1 OR id = ?1 LIMIT 1",
        escaped_table
    );
    let mut statement = match connection.prepare(&query) {
        Ok(statement) => statement,
        Err(_) => return Ok(None),
    };
    let column_names = statement
        .column_names()
        .iter()
        .map(|name| name.to_string())
        .collect::<Vec<_>>();
    let mut rows = statement.query([key])?;
    let Some(row) = rows.next()? else {
        return Ok(None);
    };
    let mut fields = Map::<String, Value>::new();
    for (index, name) in column_names.iter().enumerate() {
        let text = row
            .get_ref(index)
            .map(sqlite_value_text)
            .unwrap_or_default();
        fields.insert(name.clone(), json!(text));
    }
    Ok(Some(json!({
        "table": table,
        "key": key,
        "fields": fields
    })))
}

fn sqlite_value_text(value: rusqlite::types::ValueRef<'_>) -> String {
    match value {
        rusqlite::types::ValueRef::Null => String::new(),
        rusqlite::types::ValueRef::Integer(value) => value.to_string(),
        rusqlite::types::ValueRef::Real(value) => value.to_string(),
        rusqlite::types::ValueRef::Text(value) => String::from_utf8_lossy(value).to_string(),
        rusqlite::types::ValueRef::Blob(value) => String::from_utf8_lossy(value).to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rollout_codec_keeps_only_conversation_lines_for_the_selected_session() {
        let raw = [
            r#"{"type":"session_meta","payload":{"id":"one"}}"#,
            r#"{"type":"response_item","payload":{"type":"message","role":"user","content":"keep"}}"#,
            r#"{"type":"session_meta","payload":{"id":"two"}}"#,
            r#"{"type":"response_item","payload":{"type":"message","role":"assistant","content":"drop"}}"#,
        ]
        .join("\n");

        let selected = filter_codex_rollout_jsonl_source(&raw, "one");
        assert_eq!(selected.len(), 1);
        assert!(selected[0].contains("keep"));
    }
}
