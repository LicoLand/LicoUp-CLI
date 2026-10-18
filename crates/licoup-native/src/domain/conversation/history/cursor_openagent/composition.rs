use std::fs;
use std::path::Path;

use rusqlite::types::ValueRef;
use rusqlite::{Connection, TransactionBehavior};
use serde_json::Value;

use super::super::generic::collect_explicit_json_sessions;
use super::super::{HistoryAdapter, HistoryScanConfig};
use super::codec::{open_read_only_connection, sqlite_table_exists};
use super::cursor::parse_cursor_sqlite_sessions;
use super::cursor_cli::parse_cursor_cli_store_sessions;
use super::fallback::parse_generic_sqlite_sessions;
use super::openagent::parse_openagent_sqlite_sessions;
use crate::domain::conversation::source_catalog::COPILOT_CHAT_SESSIONS_KEY;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum CopilotChatSessionsReadError {
    SchemaUnrecognized,
    ReadFailed,
    InvalidPayload,
}

pub(crate) fn parse_sqlite_sessions(
    adapter: HistoryAdapter,
    path: &Path,
    source_kind: &str,
    metadata: &fs::Metadata,
    scan_config: HistoryScanConfig,
) -> Vec<Value> {
    let Some(mut connection) = open_read_only_connection(path) else {
        return Vec::new();
    };
    if matches!(adapter, HistoryAdapter::OpenCode | HistoryAdapter::KiloCode) {
        let precise_sessions = parse_openagent_sqlite_sessions(
            adapter,
            path,
            source_kind,
            metadata,
            &mut connection,
            scan_config.single_session_id(),
        );
        if !precise_sessions.is_empty() {
            return precise_sessions;
        }
    }
    if adapter == HistoryAdapter::Copilot {
        let Ok(Some(document)) = copilot_chat_sessions_document(&connection) else {
            return Vec::new();
        };
        return collect_explicit_json_sessions(
            adapter,
            path,
            metadata,
            source_kind,
            &document,
            scan_config.single_session_id(),
        );
    }
    if adapter == HistoryAdapter::Cursor {
        if path
            .file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| name == "store.db")
            && sqlite_table_exists(&connection, "blobs")
        {
            return parse_cursor_cli_store_sessions(path, source_kind, metadata, &connection);
        }
        // A recognized store owns an empty result too. In particular, a CLI
        // session absent from the IDE store must not make an exact read parse
        // every unrelated bubble and ItemTable record through generic fallback.
        if sqlite_table_exists(&connection, "cursorDiskKV") {
            return parse_cursor_sqlite_sessions(
                path,
                source_kind,
                metadata,
                &mut connection,
                scan_config.single_session_id(),
            );
        }
    }
    let transaction = match connection.transaction_with_behavior(TransactionBehavior::Deferred) {
        Ok(transaction) => transaction,
        Err(_) => return Vec::new(),
    };
    let sessions = parse_generic_sqlite_sessions(
        adapter,
        path,
        source_kind,
        metadata,
        &scan_config,
        &transaction,
    );
    if transaction.commit().is_err() {
        return Vec::new();
    }
    sessions
}

pub(crate) fn copilot_chat_sessions_document(
    connection: &Connection,
) -> Result<Option<Value>, CopilotChatSessionsReadError> {
    let table_exists = connection
        .query_row(
            "SELECT 1 FROM sqlite_master WHERE type='table' AND name=?1 COLLATE NOCASE LIMIT 1",
            ["ItemTable"],
            |_| Ok(()),
        )
        .is_ok();
    if !table_exists {
        return Ok(None);
    }
    let mut columns = connection
        .prepare("PRAGMA table_info(ItemTable)")
        .map_err(|_| CopilotChatSessionsReadError::SchemaUnrecognized)?;
    let columns = columns
        .query_map([], |row| row.get::<_, String>(1))
        .map_err(|_| CopilotChatSessionsReadError::SchemaUnrecognized)?
        .filter_map(Result::ok)
        .map(|name| name.to_ascii_lowercase())
        .collect::<std::collections::HashSet<_>>();
    if !columns.contains("key") || !columns.contains("value") {
        return Err(CopilotChatSessionsReadError::SchemaUnrecognized);
    }
    let mut statement = connection
        .prepare("SELECT value FROM ItemTable WHERE key = ?1 COLLATE NOCASE LIMIT 1")
        .map_err(|_| CopilotChatSessionsReadError::ReadFailed)?;
    let mut rows = statement
        .query([COPILOT_CHAT_SESSIONS_KEY])
        .map_err(|_| CopilotChatSessionsReadError::ReadFailed)?;
    let Some(row) = rows
        .next()
        .map_err(|_| CopilotChatSessionsReadError::ReadFailed)?
    else {
        return Ok(None);
    };
    let value = row
        .get_ref(0)
        .map_err(|_| CopilotChatSessionsReadError::ReadFailed)?;
    let bytes = match value {
        ValueRef::Text(bytes) | ValueRef::Blob(bytes) => bytes,
        _ => return Err(CopilotChatSessionsReadError::InvalidPayload),
    };
    serde_json::from_slice(bytes)
        .map(Some)
        .map_err(|_| CopilotChatSessionsReadError::InvalidPayload)
}
