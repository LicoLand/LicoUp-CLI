use std::fs;
use std::path::Path;

use rusqlite::Connection;
use serde_json::{Value, json};
use time::{OffsetDateTime, format_description::well_known::Rfc3339};

use super::super::HistoryAdapter;
use super::super::generic::messages_from_json;
use super::super::message_projection::{extract_role, extract_text, native_message_timestamp};
use super::super::query_filter::epoch_value_to_rfc3339;
use super::super::session_metadata::session_from_messages_with_title;
use super::codec::sqlite_table_exists;
use super::cursor_projection::normalize_cursor_model_name;

/// Cursor records a conversation's real creation and update times in the
/// session `meta.json` next to `store.db`. The store itself carries no
/// per-message timestamps, so message order keys are interpolated across that
/// interval: they stay monotonic and close to the real conversation flow,
/// which is what delegated-subagent cards use to rejoin the main thread.
#[derive(Clone, Copy)]
struct CursorCliSessionTimes {
    created_ms: i128,
    updated_ms: i128,
}

pub(super) fn parse_cursor_cli_store_sessions(
    path: &Path,
    source_kind: &str,
    metadata: &fs::Metadata,
    connection: &Connection,
) -> Vec<Value> {
    if !sqlite_table_exists(connection, "blobs") {
        return Vec::new();
    }
    let session_id = path
        .parent()
        .and_then(|parent| parent.file_name())
        .and_then(|name| name.to_str())
        .filter(|value| !value.trim().is_empty())
        .map(str::to_string)
        .unwrap_or_else(|| "cli-session".to_owned());
    let (created_at, updated_at, model) = cursor_cli_store_metadata(connection);
    let session_times = cursor_cli_session_times_from_meta_json(path);
    let mut messages = Vec::<Value>::new();
    let Ok(mut statement) = connection.prepare("SELECT id, data FROM blobs ORDER BY id ASC") else {
        return Vec::new();
    };
    let Ok(rows) = statement.query_map([], |row| {
        Ok((
            row.get::<_, Option<String>>(0)?,
            row.get::<_, Option<Vec<u8>>>(1)?,
        ))
    }) else {
        return Vec::new();
    };
    let blobs = rows.flatten().collect::<Vec<_>>();
    for (index, (_, data)) in blobs.iter().enumerate() {
        let Some(data) = data else {
            continue;
        };
        let Ok(raw) = std::str::from_utf8(data) else {
            continue;
        };
        let Ok(value) = serde_json::from_str::<Value>(raw) else {
            continue;
        };
        let role = extract_role(&value);
        if role == "system" || role == "metadata" {
            continue;
        }
        let text = extract_text(&value).unwrap_or_default();
        if text.trim().is_empty() {
            continue;
        }
        let created =
            cursor_cli_message_timestamp(session_times, created_at.as_deref(), index, blobs.len());
        let mut envelope = value;
        if !envelope.is_object() {
            envelope = json!({});
        }
        if let Some(object) = envelope.as_object_mut() {
            object
                .entry("role".to_string())
                .or_insert_with(|| json!(role));
            object.insert("createdAt".to_string(), json!(created));
            if let Some(model) = model.as_ref() {
                object
                    .entry("model".to_string())
                    .or_insert_with(|| json!(model));
            }
        }
        messages.extend(messages_from_json(
            HistoryAdapter::Cursor,
            path,
            index,
            &envelope,
        ));
    }
    if messages.is_empty() {
        return Vec::new();
    }
    let mut session = session_from_messages_with_title(
        HistoryAdapter::Cursor,
        path,
        metadata,
        source_kind,
        session_id,
        messages,
        None,
    );
    if let Some(object) = session.as_object_mut() {
        if let Some(model) = model {
            object.insert("model".to_string(), json!(model));
        }
        if let Some(created_at) = created_at {
            object.insert("createdAt".to_string(), json!(created_at));
        }
        if let Some(updated_at) = updated_at {
            object.insert("updatedAt".to_string(), json!(updated_at));
        }
    }
    vec![session]
}

/// Reads the real session creation/update times from the `meta.json` that
/// Cursor writes next to `store.db`. The store database itself only records a
/// `createdAt` in its `meta` table and never a per-message timestamp.
fn cursor_cli_session_times_from_meta_json(path: &Path) -> Option<CursorCliSessionTimes> {
    let meta_path = path.parent()?.join("meta.json");
    let raw = fs::read_to_string(meta_path).ok()?;
    let json = serde_json::from_str::<Value>(&raw).ok()?;
    let created_ms = json.get("createdAtMs").and_then(Value::as_i64)? as i128;
    let updated_ms = json
        .get("updatedAtMs")
        .and_then(Value::as_i64)
        .map(|value| value as i128)
        .filter(|value| *value >= created_ms)
        .unwrap_or(created_ms);
    Some(CursorCliSessionTimes {
        created_ms,
        updated_ms,
    })
}

/// Message timestamp within a cursor CLI store. Blobs carry no timestamp of
/// their own, so the message is placed proportionally across the session's
/// real creation/update interval. The store's own session timestamp or the
/// projection-time fallback applies only when no session time is available.
fn cursor_cli_message_timestamp(
    session_times: Option<CursorCliSessionTimes>,
    created_at: Option<&str>,
    index: usize,
    count: usize,
) -> String {
    if let Some(times) = session_times {
        let offset = if count > 1 {
            (times.updated_ms - times.created_ms) * index as i128
                / (count.saturating_sub(1)) as i128
        } else {
            0
        };
        let millis = times.created_ms.saturating_add(offset);
        if let Some(formatted) = epoch_ms_to_rfc3339(millis) {
            return formatted;
        }
    }
    created_at
        .map(str::to_string)
        .unwrap_or_else(native_message_timestamp)
}

fn epoch_ms_to_rfc3339(millis: i128) -> Option<String> {
    OffsetDateTime::from_unix_timestamp(i64::try_from(millis / 1000).ok()?)
        .ok()
        .and_then(|time| time.format(&Rfc3339).ok())
}

fn cursor_cli_store_metadata(
    connection: &Connection,
) -> (Option<String>, Option<String>, Option<String>) {
    if !sqlite_table_exists(connection, "meta") {
        return (None, None, None);
    }
    let Ok(mut statement) = connection.prepare("SELECT value FROM meta ORDER BY key ASC") else {
        return (None, None, None);
    };
    let Ok(rows) = statement.query_map([], |row| Ok(row.get::<_, Option<String>>(0)?)) else {
        return (None, None, None);
    };
    for value in rows.flatten().flatten() {
        let decoded = decode_cli_meta_value(&value);
        let Some(json) = decoded else {
            continue;
        };
        let created_at = json
            .get("createdAt")
            .and_then(epoch_value_to_rfc3339)
            .or_else(|| json.get("createdAtMs").and_then(epoch_value_to_rfc3339));
        let updated_at = json
            .get("updatedAt")
            .and_then(epoch_value_to_rfc3339)
            .or_else(|| json.get("updatedAtMs").and_then(epoch_value_to_rfc3339));
        let model = json
            .get("model")
            .or_else(|| json.get("modelId"))
            .and_then(Value::as_str)
            .map(normalize_cursor_model_name);
        if created_at.is_some() || updated_at.is_some() || model.is_some() {
            return (created_at, updated_at, model);
        }
    }
    (None, None, None)
}

fn decode_cli_meta_value(value: &str) -> Option<Value> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return None;
    }
    if trimmed.starts_with('{') {
        return serde_json::from_str(trimmed).ok();
    }
    if trimmed.len() % 2 == 0 && trimmed.chars().all(|ch| ch.is_ascii_hexdigit()) {
        let bytes = (0..trimmed.len())
            .step_by(2)
            .filter_map(|index| u8::from_str_radix(&trimmed[index..index + 2], 16).ok())
            .collect::<Vec<_>>();
        let text = String::from_utf8(bytes).ok()?;
        return serde_json::from_str(&text).ok();
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn message_timestamps_interpolate_across_the_session_interval() {
        let times = Some(CursorCliSessionTimes {
            created_ms: 1_784_600_000_000,
            updated_ms: 1_784_600_600_000,
        });
        let first = cursor_cli_message_timestamp(times, None, 0, 4);
        let second = cursor_cli_message_timestamp(times, None, 1, 4);
        let last = cursor_cli_message_timestamp(times, None, 3, 4);
        assert_eq!(first, "2026-07-21T02:13:20Z");
        assert_eq!(second, "2026-07-21T02:16:40Z");
        assert_eq!(last, "2026-07-21T02:23:20Z");
        assert_ne!(first, second);
    }

    #[test]
    fn message_timestamps_fall_back_to_the_session_time_without_meta_json() {
        let created = "2026-07-20T16:53:20Z";
        assert_eq!(
            cursor_cli_message_timestamp(None, Some(created), 0, 4),
            created
        );
        let fallback = cursor_cli_message_timestamp(None, None, 0, 4);
        assert!(!fallback.trim().is_empty());
    }
}
