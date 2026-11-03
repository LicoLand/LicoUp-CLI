use std::collections::HashSet;

use rusqlite::Connection;
use serde_json::Value;

use super::super::project_workspace::bounded_project_workspace;
use super::super::query_filter::epoch_value_to_rfc3339;
use super::codec::sqlite_value_text;
use super::cursor_projection::cursor_composer_model_from_config;

pub(super) struct CursorComposerMeta {
    id: String,
    title: Option<String>,
    model: String,
    created_at: Option<String>,
    updated_at: Option<String>,
    bubble_ids: Vec<String>,
    parent_composer_id: Option<String>,
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
            workspace_path,
        });
    }
    composers
}
