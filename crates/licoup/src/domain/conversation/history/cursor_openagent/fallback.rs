use std::fs;
use std::path::Path;
use std::time::SystemTime;

use rusqlite::Connection;
use serde_json::{Value, json};

use super::super::generic::{collect_messages_from_value, push_grouped_message};
use super::super::query_filter::{display_path, extract_json_from_text, message_id, system_time};
use super::super::session_metadata::session_from_messages;
use super::super::{HistoryAdapter, HistoryScanConfig};
use super::codec::{sqlite_fields_json, sqlite_row_fields, sqlite_row_key};

pub(super) fn parse_generic_sqlite_sessions(
    adapter: HistoryAdapter,
    path: &Path,
    source_kind: &str,
    metadata: &fs::Metadata,
    scan_config: &HistoryScanConfig,
    connection: &Connection,
) -> Vec<Value> {
    let mut sessions = Vec::<Value>::new();
    let requested_session_id = scan_config.single_session_id();
    let mut table_statement = match connection
        .prepare("SELECT name FROM sqlite_master WHERE type='table' AND name NOT LIKE 'sqlite_%'")
    {
        Ok(statement) => statement,
        Err(_) => return sessions,
    };
    let table_rows = match table_statement.query_map([], |row| row.get::<_, String>(0)) {
        Ok(rows) => rows,
        Err(_) => return sessions,
    };
    let mut table_names = Vec::new();
    for row in table_rows {
        let Ok(name) = row else {
            return Vec::new();
        };
        if adapter.sqlite_table_may_hold_history(&name) {
            table_names.push(name);
        }
    }

    for table in table_names {
        let mut grouped = Vec::<(String, Vec<Value>)>::new();
        let query = format!("SELECT * FROM \"{}\"", table.replace('"', "\"\""));
        let mut statement = match connection.prepare(&query) {
            Ok(statement) => statement,
            Err(_) => return Vec::new(),
        };
        let column_names = statement
            .column_names()
            .iter()
            .map(|name| name.to_string())
            .collect::<Vec<_>>();
        let mut rows = match statement.query([]) {
            Ok(rows) => rows,
            Err(_) => return Vec::new(),
        };
        let mut index = 0usize;
        loop {
            let row = match rows.next() {
                Ok(Some(row)) => row,
                Ok(None) => break,
                Err(_) => return Vec::new(),
            };
            let fields = match sqlite_row_fields(row, &column_names) {
                Ok(fields) => fields,
                Err(_) => return Vec::new(),
            };
            let row_index = index;
            index = index.saturating_add(1);
            let row_text = fields
                .iter()
                .map(|(name, value)| format!("{}: {}", name, value))
                .collect::<Vec<_>>()
                .join("\n");
            let row_key = sqlite_row_key(&fields);
            if !adapter.sqlite_row_may_hold_history(&table, row_key.as_deref(), &row_text) {
                continue;
            }
            let source_fields = sqlite_fields_json(&fields);
            let session_id = row_key
                .clone()
                .unwrap_or_else(|| format!("{}:{}", table, row_index));
            if requested_session_id.is_some_and(|requested| requested != session_id.as_str()) {
                continue;
            }
            let row_key_value = row_key.unwrap_or_default();
            let mut row_messages = Vec::<Value>::new();
            for (_, value) in &fields {
                let json_value = serde_json::from_str::<Value>(value)
                    .ok()
                    .or_else(|| extract_json_from_text(value));
                if let Some(json_value) = json_value {
                    collect_messages_from_value(adapter, path, &json_value, &mut row_messages);
                }
            }
            if row_messages.is_empty() {
                row_messages.push(json!({
                    "id": message_id(adapter.id(), path, row_index),
                    "role": "record",
                    "text": row_text,
                    "createdAt": system_time(metadata.modified().unwrap_or(SystemTime::UNIX_EPOCH)),
                    "sourcePath": display_path(path),
                    "sourceTable": table,
                    "sourceKey": row_key_value.clone(),
                    "sourceFields": source_fields.clone()
                }));
            }
            for mut message in row_messages {
                if let Some(object) = message.as_object_mut() {
                    object
                        .entry("sourceTable".to_string())
                        .or_insert_with(|| json!(table.clone()));
                    object
                        .entry("sourceKey".to_string())
                        .or_insert_with(|| json!(row_key_value.clone()));
                    object
                        .entry("sourceFields".to_string())
                        .or_insert_with(|| source_fields.clone());
                }
                push_grouped_message(&mut grouped, session_id.clone(), message);
            }
        }
        for (native_session_id, messages) in grouped {
            sessions.push(session_from_messages(
                adapter,
                path,
                metadata,
                source_kind,
                native_session_id,
                messages,
            ));
        }
    }
    sessions
}
