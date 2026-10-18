use std::collections::HashMap;
use std::fs;
use std::path::Path;

use rusqlite::{Connection, TransactionBehavior};
use serde_json::{Value, json};

use super::super::HistoryAdapter;
use super::super::generic::messages_from_json;
use super::super::message_projection::{extract_role, native_message_timestamp};
use super::super::project_workspace::bounded_project_workspace;
use super::super::query_filter::epoch_number_to_rfc3339;
use super::super::session_metadata::session_from_messages_with_title;
use super::codec::{sqlite_table_exists, sqlite_value_text};

#[derive(Clone, Debug)]
pub(super) struct OpenAgentSessionMeta {
    id: String,
    parent_id: Option<String>,
    title: Option<String>,
    directory: Option<String>,
    path: Option<String>,
    agent: Option<String>,
    model: Option<String>,
    created_at: Option<String>,
    updated_at: Option<String>,
    usage: Option<Value>,
}

pub(super) fn parse_openagent_sqlite_sessions(
    adapter: HistoryAdapter,
    path: &Path,
    source_kind: &str,
    metadata: &fs::Metadata,
    connection: &mut Connection,
    only_session_id: Option<&str>,
) -> Vec<Value> {
    let transaction = match connection.transaction_with_behavior(TransactionBehavior::Deferred) {
        Ok(transaction) => transaction,
        Err(_) => return Vec::new(),
    };
    if !sqlite_table_exists(&transaction, "session")
        || !sqlite_table_exists(&transaction, "message")
        || !sqlite_table_exists(&transaction, "part")
    {
        return Vec::new();
    }

    let sessions =
        retain_requested_session_lineage(openagent_session_rows(&transaction), only_session_id)
            .into_iter()
            .filter_map(|meta| {
                let messages =
                    openagent_messages_for_session(adapter, path, &transaction, &meta.id);
                if messages.is_empty() {
                    return None;
                }
                let mut session = session_from_messages_with_title(
                    adapter,
                    path,
                    metadata,
                    source_kind,
                    meta.id,
                    messages,
                    meta.title,
                );
                if let Some(object) = session.as_object_mut() {
                    if let Some(created_at) = meta.created_at {
                        object.insert("createdAt".to_string(), json!(created_at));
                    }
                    if let Some(updated_at) = meta.updated_at {
                        object.insert("updatedAt".to_string(), json!(updated_at));
                    }
                    if let Some(directory) = meta.directory.filter(|value| !value.trim().is_empty())
                    {
                        object.insert("workingDirectory".to_string(), json!(directory));
                    }
                    if let Some(path) = meta.path.filter(|value| !value.trim().is_empty()) {
                        object.insert("projectPath".to_string(), json!(path));
                    }
                    if let Some(agent) = meta.agent.filter(|value| !value.trim().is_empty()) {
                        object.insert("nativeAgent".to_string(), json!(agent));
                    }
                    if let Some(model) = meta.model.filter(|value| !value.trim().is_empty()) {
                        object.insert("model".to_string(), json!(model));
                    }
                    if let Some(usage) = meta.usage {
                        object.insert("usage".to_string(), usage);
                    }
                }
                Some(session)
            })
            .collect();
    if transaction.commit().is_err() {
        return Vec::new();
    }
    sessions
}

/// Columns the session projection can use when the store happens to have them.
/// `id` is the only one every schema is required to carry.
const OPENAGENT_SESSION_COLUMNS: [&str; 14] = [
    "id",
    "title",
    "directory",
    "path",
    "agent",
    "model",
    "time_created",
    "time_updated",
    "tokens_input",
    "tokens_output",
    "tokens_reasoning",
    "tokens_cache_read",
    "tokens_cache_write",
    "parent_id",
];

/// Session rows from one OpenCode-shaped store.
///
/// The column set differs between builds — a Kilo Code or OpenCode `session`
/// table has no `agent`, `model`, or token columns at all. Selecting a fixed
/// list makes `prepare` fail on those stores, which silently produced zero
/// sessions and left every conversation of the agent rendering as an empty row.
/// Missing columns are projected as `NULL` instead, so a narrower schema still
/// yields its conversations.
pub(super) fn openagent_session_rows(connection: &Connection) -> Vec<OpenAgentSessionMeta> {
    let Ok(available) = sqlite_table_columns(connection, "session") else {
        return Vec::new();
    };
    if !available.contains("id") {
        return Vec::new();
    }
    let projection = OPENAGENT_SESSION_COLUMNS
        .iter()
        .map(|column| {
            if available.contains(*column) {
                format!("\"{column}\"")
            } else {
                "NULL".to_string()
            }
        })
        .collect::<Vec<_>>()
        .join(", ");
    let order = if available.contains("time_updated") {
        "ORDER BY time_updated DESC, id ASC"
    } else {
        "ORDER BY id ASC"
    };
    let mut statement =
        match connection.prepare(&format!("SELECT {projection} FROM session {order}")) {
            Ok(statement) => statement,
            Err(_) => return Vec::new(),
        };
    let rows = match statement.query_map([], |row| {
        let Some(id) = sqlite_value_text(row.get_ref(0)?).filter(|value| !value.trim().is_empty())
        else {
            return Ok(None);
        };
        let tokens_input = row.get::<_, Option<i64>>(8)?;
        let tokens_output = row.get::<_, Option<i64>>(9)?;
        let tokens_reasoning = row.get::<_, Option<i64>>(10)?;
        let tokens_cache_read = row.get::<_, Option<i64>>(11)?;
        let tokens_cache_write = row.get::<_, Option<i64>>(12)?;
        Ok(Some(OpenAgentSessionMeta {
            id,
            parent_id: sqlite_value_text(row.get_ref(13)?),
            title: sqlite_value_text(row.get_ref(1)?),
            directory: sqlite_value_text(row.get_ref(2)?)
                .as_deref()
                .and_then(bounded_project_workspace),
            path: sqlite_value_text(row.get_ref(3)?),
            agent: sqlite_value_text(row.get_ref(4)?),
            model: sqlite_value_text(row.get_ref(5)?),
            created_at: row
                .get::<_, Option<i64>>(6)?
                .and_then(epoch_number_to_rfc3339),
            updated_at: row
                .get::<_, Option<i64>>(7)?
                .and_then(epoch_number_to_rfc3339),
            usage: openagent_usage_from_columns(
                tokens_input,
                tokens_output,
                tokens_reasoning,
                tokens_cache_read,
                tokens_cache_write,
            ),
        }))
    }) {
        Ok(rows) => rows,
        Err(_) => return Vec::new(),
    };
    let mut sessions = Vec::new();
    for row in rows {
        match row {
            Ok(Some(session)) => sessions.push(session),
            Ok(None) => {}
            Err(_) => return Vec::new(),
        }
    }
    sessions
}

/// Narrow an OpenCode/Kilo database before reading message parts. Child
/// sessions remain in scope so the normal lineage fold can still produce the
/// parent conversation's delegated-task cards.
fn retain_requested_session_lineage(
    sessions: Vec<OpenAgentSessionMeta>,
    only_session_id: Option<&str>,
) -> Vec<OpenAgentSessionMeta> {
    let Some(requested) = only_session_id.map(str::trim).filter(|id| !id.is_empty()) else {
        return sessions;
    };
    let mut wanted = std::collections::HashSet::<String>::from([requested.to_owned()]);
    loop {
        let before = wanted.len();
        for session in &sessions {
            if session
                .parent_id
                .as_deref()
                .is_some_and(|parent| wanted.contains(parent))
            {
                wanted.insert(session.id.clone());
            }
        }
        if wanted.len() == before {
            break;
        }
    }
    sessions
        .into_iter()
        .filter(|session| wanted.contains(&session.id))
        .collect()
}

fn sqlite_table_columns(
    connection: &Connection,
    table: &str,
) -> std::result::Result<std::collections::HashSet<String>, rusqlite::Error> {
    // Table names at every call site are compile-time constants.
    let mut statement = connection.prepare(&format!("PRAGMA table_info({table})"))?;
    statement
        .query_map([], |row| row.get::<_, String>(1))?
        .collect()
}

pub(super) fn openagent_messages_for_session(
    adapter: HistoryAdapter,
    path: &Path,
    connection: &Connection,
    session_id: &str,
) -> Vec<Value> {
    let Some(mut parts_by_message) = openagent_parts_by_message(connection, session_id) else {
        return Vec::new();
    };
    let Ok(available) = sqlite_table_columns(connection, "message") else {
        return Vec::new();
    };
    if !["id", "session_id", "data"]
        .iter()
        .all(|column| available.contains(*column))
    {
        return Vec::new();
    }
    let created = available
        .contains("time_created")
        .then_some("time_created")
        .unwrap_or("NULL");
    let updated = available
        .contains("time_updated")
        .then_some("time_updated")
        .unwrap_or("NULL");
    let order = if available.contains("time_created") {
        "time_created ASC, id ASC"
    } else {
        "id ASC"
    };
    let query = format!(
        "SELECT id, {created}, {updated}, data FROM message \
         WHERE session_id=?1 ORDER BY {order}"
    );
    let mut statement = match connection.prepare(&query) {
        Ok(statement) => statement,
        Err(_) => return Vec::new(),
    };
    let rows = match statement.query_map([session_id], |row| {
        Ok((
            sqlite_value_text(row.get_ref(0)?),
            row.get::<_, Option<i64>>(1)?,
            row.get::<_, Option<i64>>(2)?,
            sqlite_value_text(row.get_ref(3)?),
        ))
    }) {
        Ok(rows) => rows,
        Err(_) => return Vec::new(),
    };

    let mut messages = Vec::<Value>::new();
    for (index, row) in rows.enumerate() {
        let Ok((message_id, created_at, updated_at, data)) = row else {
            return Vec::new();
        };
        let (Some(message_id), Some(data)) = (message_id, data) else {
            continue;
        };
        let data_value = serde_json::from_str::<Value>(&data).unwrap_or(Value::Null);
        let role = extract_role(&data_value);
        let parts = parts_by_message.remove(&message_id).unwrap_or_default();
        let created = openagent_json_time(&data_value, "created")
            .or_else(|| created_at.and_then(epoch_number_to_rfc3339))
            .or_else(|| openagent_json_time(&data_value, "completed"))
            .or_else(|| updated_at.and_then(epoch_number_to_rfc3339))
            .unwrap_or_else(native_message_timestamp);
        let mut envelope = data_value;
        if !envelope.is_object() {
            envelope = json!({});
        }
        if let Some(object) = envelope.as_object_mut() {
            object
                .entry("role".to_string())
                .or_insert_with(|| json!(role));
            object.insert("createdAt".to_string(), json!(created));
            if !parts.is_empty() {
                object.insert("content".to_string(), Value::Array(parts));
            }
        }
        let mut expanded = messages_from_json(adapter, path, index, &envelope);
        let expanded_len = expanded.len();
        for (block_index, mut message) in expanded.drain(..).enumerate() {
            if let Some(object) = message.as_object_mut() {
                object.insert(
                    "id".to_string(),
                    json!(if expanded_len == 1 {
                        format!("{message_id}:{index}")
                    } else {
                        format!("{message_id}:{index}:{block_index}")
                    }),
                );
                object.insert("sourceMessageId".to_string(), json!(message_id.clone()));
            }
            messages.push(message);
        }
    }
    messages
}

pub(super) fn openagent_parts_by_message(
    connection: &Connection,
    session_id: &str,
) -> Option<HashMap<String, Vec<Value>>> {
    let Ok(available) = sqlite_table_columns(connection, "part") else {
        return None;
    };
    if !["message_id", "session_id", "data"]
        .iter()
        .all(|column| available.contains(*column))
    {
        return None;
    }
    let order = match (available.contains("time_created"), available.contains("id")) {
        (true, true) => "time_created ASC, id ASC",
        (true, false) => "time_created ASC",
        (false, true) => "id ASC",
        (false, false) => "rowid ASC",
    };
    let query = format!("SELECT message_id, data FROM part WHERE session_id=?1 ORDER BY {order}");
    let mut statement = match connection.prepare(&query) {
        Ok(statement) => statement,
        Err(_) => return None,
    };
    let rows = match statement.query_map([session_id], |row| {
        Ok((
            sqlite_value_text(row.get_ref(0)?),
            sqlite_value_text(row.get_ref(1)?),
        ))
    }) {
        Ok(rows) => rows,
        Err(_) => return None,
    };
    let mut out = HashMap::<String, Vec<Value>>::new();
    for row in rows {
        let Ok((message_id, data)) = row else {
            return None;
        };
        let (Some(message_id), Some(data)) = (message_id, data) else {
            continue;
        };
        let value = serde_json::from_str::<Value>(&data).unwrap_or_else(|_| json!(data));
        out.entry(message_id).or_default().push(value);
    }
    Some(out)
}

pub(super) fn openagent_json_time(value: &Value, key: &str) -> Option<String> {
    value
        .get("time")
        .and_then(|time| time.get(key))
        .and_then(super::super::query_filter::epoch_value_to_rfc3339)
}

pub(super) fn openagent_usage_from_columns(
    input: Option<i64>,
    output: Option<i64>,
    reasoning: Option<i64>,
    cache_read: Option<i64>,
    cache_write: Option<i64>,
) -> Option<Value> {
    let cached_input_tokens = cache_read.unwrap_or(0).max(0);
    let prompt_tokens = [input, cache_read, cache_write]
        .into_iter()
        .flatten()
        .filter(|value| *value > 0)
        .sum::<i64>();
    let completion_tokens = [output, reasoning]
        .into_iter()
        .flatten()
        .filter(|value| *value > 0)
        .sum::<i64>();
    let total_tokens = prompt_tokens + completion_tokens;
    if total_tokens <= 0 {
        return None;
    }
    Some(json!({
        "promptTokens": prompt_tokens,
        "cachedInputTokens": cached_input_tokens,
        "completionTokens": completion_tokens,
        "totalTokens": total_tokens,
        "source": "openagent-sqlite"
    }))
}
