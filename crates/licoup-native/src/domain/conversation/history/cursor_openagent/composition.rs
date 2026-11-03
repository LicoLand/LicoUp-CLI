use std::fs;
use std::path::Path;

use rusqlite::types::ValueRef;
use rusqlite::{Connection, TransactionBehavior};
use serde_json::Value;

use super::super::{HistoryAdapter, HistoryScanConfig};
use super::codec::open_read_only_connection;
use super::fallback::parse_generic_sqlite_sessions;
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
