use std::collections::HashSet;
use std::fs;
use std::path::Path;

use rusqlite::{Connection, TransactionBehavior};
use serde_json::{Value, json};

use super::super::HistoryAdapter;
use super::super::project_workspace::bounded_project_workspace;
use super::super::query_filter::epoch_value_to_rfc3339;
use super::super::session_metadata::session_from_messages_with_title;
use super::codec::{sqlite_table_exists, sqlite_value_text};
use super::cursor_projection::{cursor_composer_model_from_config, cursor_message_from_bubble};

pub(super) fn parse_cursor_sqlite_sessions(
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
    if !sqlite_table_exists(&transaction, "cursorDiskKV") {
        return Vec::new();
    }

    let composers = cursor_composer_rows(&transaction);
    if composers.is_empty() {
        return Vec::new();
    }
    let composers = retain_requested_composer_lineage(composers, only_session_id);
    if composers.is_empty() {
        return Vec::new();
    }
    let mut sessions = Vec::new();
    for composer in composers {
        let bubble_ids = if composer.bubble_ids.is_empty() {
            cursor_bubble_ids_for_composer(&transaction, &composer.id)
        } else {
            composer.bubble_ids.clone()
        };
        if bubble_ids.is_empty() {
            continue;
        }

        let mut messages = Vec::new();
        for bubble_id in bubble_ids {
            let Some(raw) = cursor_disk_kv_json(
                &transaction,
                &format!("bubbleId:{}:{}", composer.id, bubble_id),
            ) else {
                continue;
            };
            if let Some(message) =
                cursor_message_from_bubble(&raw, &composer.model, path, messages.len())
            {
                messages.push(message);
            }
        }
        if messages.is_empty() {
            continue;
        }

        let mut session = session_from_messages_with_title(
            HistoryAdapter::Cursor,
            path,
            metadata,
            source_kind,
            composer.id.clone(),
            messages,
            composer.title.clone(),
        );
        if let Some(object) = session.as_object_mut() {
            object.insert("model".to_string(), json!(composer.model.clone()));
            if let Some(created_at) = composer.created_at.as_ref() {
                object.insert("createdAt".to_string(), json!(created_at));
            }
            if let Some(updated_at) = composer.updated_at.as_ref() {
                object.insert("updatedAt".to_string(), json!(updated_at));
            }
            if let Some(workspace) = composer.workspace_path.as_ref() {
                object.insert("workingDirectory".to_string(), json!(workspace));
            }
        }
        sessions.push((composer, session));
    }
    tag_delegated_subagent_sessions(&mut sessions);
    sessions.into_iter().map(|(_, session)| session).collect()
}

/// Keep only the requested conversation and the delegated tasks below it.
///
/// The IDE store holds every Cursor conversation in one database. Reading one
/// conversation must not walk the message bubbles of all the others, so the
/// composer list is narrowed before any bubble is touched. Delegated descendants
/// stay because they fold into the conversation as task cards.
fn retain_requested_composer_lineage(
    composers: Vec<CursorComposerMeta>,
    only_session_id: Option<&str>,
) -> Vec<CursorComposerMeta> {
    let Some(requested) = only_session_id.map(str::trim).filter(|id| !id.is_empty()) else {
        return composers;
    };
    let mut wanted = HashSet::<String>::new();
    wanted.insert(requested.to_string());
    // Delegated composers appear after their parent in insertion order but the
    // store gives no ordering guarantee, so repeat until the closure is stable.
    loop {
        let before = wanted.len();
        for composer in &composers {
            if let Some(parent) = composer.parent_composer_id.as_deref()
                && wanted.contains(parent)
            {
                wanted.insert(composer.id.clone());
            }
        }
        if wanted.len() == before {
            break;
        }
    }
    composers
        .into_iter()
        .filter(|composer| wanted.contains(&composer.id))
        .collect()
}

/// Marks subagent composer sessions with the explicit delegated-lineage
/// markers the session merge consumes, so each subagent thread folds into its
/// parent composer as a collapsed subagent card instead of surfacing as an
/// indistinguishable top-level session. Subagents whose parent composer did
/// not yield a session keep their flat top-level entry.
fn tag_delegated_subagent_sessions(sessions: &mut [(CursorComposerMeta, Value)]) {
    let emitted_ids = sessions
        .iter()
        .filter_map(|(_, session)| {
            session
                .get("nativeSessionId")
                .and_then(Value::as_str)
                .map(str::to_string)
        })
        .collect::<HashSet<_>>();
    for (composer, session) in sessions.iter_mut() {
        let Some(parent_id) = composer.parent_composer_id.as_deref() else {
            continue;
        };
        if !emitted_ids.contains(parent_id) {
            continue;
        }
        let Some(object) = session.as_object_mut() else {
            continue;
        };
        object.insert("delegatedSubagent".to_string(), json!(true));
        object.insert("parentSessionId".to_string(), json!(parent_id));
        if let Some(title) = composer
            .title
            .clone()
            .or_else(|| composer.subagent_type_name.clone())
        {
            object.insert("subagentTitle".to_string(), json!(title));
        }
    }
}

#[derive(Clone, Debug)]
pub(super) struct CursorComposerMeta {
    id: String,
    title: Option<String>,
    model: String,
    created_at: Option<String>,
    updated_at: Option<String>,
    bubble_ids: Vec<String>,
    parent_composer_id: Option<String>,
    subagent_type_name: Option<String>,
    /// Project directory the composer was opened against, taken from Cursor's
    /// own `workspaceIdentifier` record. Cursor is the only local agent whose
    /// conversation store knows the project path without a separate CLI file,
    /// so this is the authoritative working directory for the session.
    workspace_path: Option<String>,
}

/// Browse-list metadata for one Cursor composer, read without touching a single
/// message bubble.
///
/// The IDE store keeps every conversation in one `state.vscdb`, so a file-level
/// catalog cannot see the conversations inside it. This is the composer-level
/// tier: `composerData:*` rows already carry the title, timestamps, project
/// directory, delegated lineage, and the message header list, which is all a
/// browse row needs.
#[derive(Clone, Debug)]
pub(crate) struct CursorComposerCatalogEntry {
    pub(crate) composer_id: String,
    pub(crate) title: Option<String>,
    pub(crate) model: Option<String>,
    pub(crate) created_at: Option<String>,
    pub(crate) updated_at: Option<String>,
    pub(crate) working_directory: Option<String>,
    pub(crate) message_count: usize,
    /// Set when this composer is a delegated task of another composer in the
    /// same store. Such a composer is folded into its parent conversation and
    /// must not occupy its own browse row.
    pub(crate) parent_composer_id: Option<String>,
}

/// Composer-level catalog for one Cursor IDE store.
///
/// Composers without a message header list are drafts Cursor never populated
/// and are left out. A delegated composer keeps its parent reference only when
/// the parent is present in the same store, so orphaned delegated work stays
/// reachable as its own row.
pub(crate) fn cursor_composer_catalog(connection: &Connection) -> Vec<CursorComposerCatalogEntry> {
    let composers = cursor_composer_rows(connection);
    let present = composers
        .iter()
        .map(|composer| composer.id.as_str())
        .collect::<HashSet<_>>();
    composers
        .iter()
        .filter(|composer| !composer.bubble_ids.is_empty())
        .map(|composer| CursorComposerCatalogEntry {
            composer_id: composer.id.clone(),
            title: composer.title.clone(),
            model: (!composer.model.trim().is_empty()).then(|| composer.model.clone()),
            created_at: composer.created_at.clone(),
            updated_at: composer.updated_at.clone(),
            working_directory: composer.workspace_path.clone(),
            message_count: composer.bubble_ids.len(),
            parent_composer_id: composer
                .parent_composer_id
                .clone()
                .filter(|parent| present.contains(parent.as_str())),
        })
        .collect()
}

/// Project path recorded on one composer.
///
/// Cursor writes the workspace as a VS Code URI object. `path` is the plain
/// filesystem path, `fsPath` is the platform-native form, and `external` is the
/// `file://` URI; older records only carry one of them. Unbounded roots (the
/// filesystem root, the home directory and its ancestors, personal library
/// roots, media library bundles) are dropped here so a residual record can
/// never become a bindable project directory.
fn cursor_composer_workspace_path(value: &Value) -> Option<String> {
    let uri = value.get("workspaceIdentifier")?.get("uri")?;
    ["path", "fsPath", "external"]
        .iter()
        .filter_map(|key| uri.get(*key).and_then(Value::as_str))
        .filter_map(decode_workspace_location)
        .find_map(|candidate| bounded_project_workspace(&candidate))
}

/// Plain path for one recorded workspace location. A `file://` URI is decoded
/// to its path with percent escapes resolved; any other scheme is not a local
/// project directory and is dropped.
fn decode_workspace_location(raw: &str) -> Option<String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return None;
    }
    if let Some(rest) = trimmed.strip_prefix("file://") {
        // A local file URI keeps the leading separator; a URI naming a remote
        // host is not a local project directory.
        let path = rest.strip_prefix('/')?;
        return Some(decode_percent_escapes(&format!("/{path}")));
    }
    if trimmed.contains("://") {
        return None;
    }
    Some(trimmed.to_string())
}

fn decode_percent_escapes(value: &str) -> String {
    let bytes = value.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%' && index + 2 < bytes.len() {
            let high = (bytes[index + 1] as char).to_digit(16);
            let low = (bytes[index + 2] as char).to_digit(16);
            if let (Some(high), Some(low)) = (high, low) {
                out.push((high * 16 + low) as u8);
                index += 3;
                continue;
            }
        }
        out.push(bytes[index]);
        index += 1;
    }
    String::from_utf8(out).unwrap_or_else(|_| value.to_string())
}

pub(super) fn cursor_composer_rows(connection: &Connection) -> Vec<CursorComposerMeta> {
    let Ok(mut statement) = connection.prepare(
        "SELECT key, value FROM cursorDiskKV
         WHERE key >= 'composerData:' AND key < 'composerData;'",
    ) else {
        return Vec::new();
    };
    let Ok(rows) = statement.query_map([], |row| {
        Ok((
            sqlite_value_text(row.get_ref(0)?),
            sqlite_value_text(row.get_ref(1)?),
        ))
    }) else {
        return Vec::new();
    };

    let mut composers = Vec::new();
    for (key, value) in rows.flatten() {
        let (Some(key), Some(value)) = (key, value) else {
            continue;
        };
        let Ok(json) = serde_json::from_str::<Value>(&value) else {
            continue;
        };
        let id = json
            .get("composerId")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_string)
            .or_else(|| {
                key.strip_prefix("composerData:")
                    .map(str::trim)
                    .filter(|value| !value.is_empty())
                    .map(str::to_string)
            });
        let Some(id) = id else {
            continue;
        };
        let model = cursor_composer_model_from_config(&json);
        let title = json
            .get("name")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_string);
        let bubble_ids = json
            .get("fullConversationHeadersOnly")
            .and_then(Value::as_array)
            .map(|headers| {
                headers
                    .iter()
                    .filter_map(|header| {
                        header
                            .get("bubbleId")
                            .and_then(Value::as_str)
                            .map(str::trim)
                            .filter(|value| !value.is_empty())
                            .map(str::to_string)
                    })
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        let subagent_info = json.get("subagentInfo");
        let parent_composer_id = subagent_info
            .and_then(|info| info.get("parentComposerId"))
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_string);
        let subagent_type_name = subagent_info
            .and_then(|info| info.get("subagentTypeName"))
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_string);
        let workspace_path = cursor_composer_workspace_path(&json);
        composers.push(CursorComposerMeta {
            id,
            title,
            model,
            created_at: epoch_value_to_rfc3339(json.get("createdAt").unwrap_or(&Value::Null)),
            updated_at: epoch_value_to_rfc3339(
                json.get("lastUpdatedAt")
                    .or_else(|| json.get("updatedAt"))
                    .unwrap_or(&Value::Null),
            ),
            bubble_ids,
            parent_composer_id,
            subagent_type_name,
            workspace_path,
        });
    }
    composers
}

pub(super) fn cursor_bubble_ids_for_composer(
    connection: &Connection,
    composer_id: &str,
) -> Vec<String> {
    let prefix = format!("bubbleId:{}:", composer_id);
    let upper = format!("bubbleId:{};", composer_id);
    let Ok(mut statement) =
        connection.prepare("SELECT key FROM cursorDiskKV WHERE key >= ?1 AND key < ?2")
    else {
        return Vec::new();
    };
    let Ok(rows) = statement.query_map([&prefix, &upper], |row| {
        Ok(sqlite_value_text(row.get_ref(0)?))
    }) else {
        return Vec::new();
    };
    rows.flatten()
        .flatten()
        .filter_map(|key| {
            key.strip_prefix(&prefix)
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(str::to_string)
        })
        .collect()
}

pub(super) fn cursor_disk_kv_json(connection: &Connection, key: &str) -> Option<Value> {
    let value = connection
        .query_row(
            "SELECT value FROM cursorDiskKV WHERE key = ?1 LIMIT 1",
            [key],
            |row| Ok(sqlite_value_text(row.get_ref(0)?)),
        )
        .ok()
        .flatten()?;
    serde_json::from_str(&value).ok()
}
