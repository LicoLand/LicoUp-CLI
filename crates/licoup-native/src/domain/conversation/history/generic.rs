//! Service-neutral JSON, JSONL, embedded JSON, and text parsers.

use super::*;
use std::collections::HashMap;
use std::io::BufReader;

struct JsonlSessionAccumulator {
    native_session_id: String,
    messages: Vec<Value>,
    working_directory: Option<String>,
}

pub(crate) fn parse_jsonl_sessions(
    adapter: HistoryAdapter,
    path: &Path,
    source_kind: &str,
    metadata: &fs::Metadata,
    scan_config: HistoryScanConfig,
) -> Vec<Value> {
    let mut grouped = Vec::<JsonlSessionAccumulator>::new();
    let mut indexes = HashMap::<String, usize>::new();
    let requested_session_id = scan_config.single_session_id();
    let delegated_lineage = matches!(adapter, HistoryAdapter::Cursor | HistoryAdapter::ClaudeCode)
        .then(|| super::delegated_transcripts::delegated_transcript_lineage(path))
        .flatten();
    let file = match fs::File::open(path) {
        Ok(file) => file,
        Err(_) => return Vec::new(),
    };
    let reader = BufReader::new(file);
    for (index, line) in reader.lines().enumerate() {
        let Ok(line) = line else {
            return Vec::new();
        };
        push_jsonl_record(
            adapter,
            path,
            index,
            &line,
            requested_session_id,
            delegated_lineage.as_ref(),
            &mut grouped,
            &mut indexes,
        );
    }
    grouped
        .into_iter()
        .filter(|session| !session.messages.is_empty())
        .map(|session| {
            let mut messages = session.messages;
            backfill_transcript_message_times(&mut messages, path);
            let mut projection = session_from_messages(
                adapter,
                path,
                metadata,
                source_kind,
                session.native_session_id,
                messages,
            );
            if let (Some(object), Some(working_directory)) =
                (projection.as_object_mut(), session.working_directory)
            {
                object.insert(
                    "workingDirectory".to_string(),
                    Value::String(working_directory),
                );
            }
            // Cursor CLI transcripts carry neither a session field nor a working
            // directory: identity, delegated lineage, and the project directory
            // all come from the transcript's place in the project tree.
            if adapter == HistoryAdapter::Cursor
                && let (Some(workspace), Some(object)) = (
                    super::delegated_transcripts::cursor_transcript_project_workspace(path),
                    projection.as_object_mut(),
                )
            {
                object
                    .entry("workingDirectory".to_string())
                    .or_insert_with(|| Value::String(workspace));
            }
            // Cursor and Claude Code both store a delegated task beside its
            // conversation under the conversation's own identity. Marking it here
            // makes every read path fold it into the conversation instead of
            // listing it as its own conversation.
            if matches!(adapter, HistoryAdapter::Cursor | HistoryAdapter::ClaudeCode) {
                super::delegated_transcripts::apply_transcript_identity(&mut projection, path);
                super::delegated_transcripts::mark_delegated_transcript_session(
                    &mut projection,
                    path,
                );
            }
            projection
        })
        .collect()
}

/// Transcript records often carry no timestamp (Cursor and Claude Code write
/// plain role/message lines). Without a stable per-message key, delegated-task
/// cards cannot rejoin their conversation at the real position and collapse at
/// the end instead. The transcript directory records when the conversation
/// started and the file when it last wrote, so missing message times are
/// interpolated across that interval in record order: monotonic, stable across
/// rescans, and close to the real flow.
pub(super) fn backfill_transcript_message_times(messages: &mut [Value], path: &Path) {
    if messages.is_empty() {
        return;
    }
    let Some(start_ms) = path
        .parent()
        .and_then(|directory| directory.metadata().ok())
        .and_then(|metadata| metadata.modified().ok())
        .and_then(|time| time.duration_since(SystemTime::UNIX_EPOCH).ok())
        .map(|duration| duration.as_millis() as i128)
    else {
        return;
    };
    let Some(end_ms) = fs::metadata(path)
        .ok()
        .and_then(|metadata| metadata.modified().ok())
        .and_then(|time| time.duration_since(SystemTime::UNIX_EPOCH).ok())
        .map(|duration| duration.as_millis() as i128)
    else {
        return;
    };
    let count = messages.len();
    if end_ms < start_ms {
        // The directory mtime can be later than the file's: a `subagents/`
        // directory records when its last task file appeared, not when this
        // task started. Degrade to the file mtime as a single anchor so the
        // messages still carry a stable, real key instead of parse time.
        for message in messages.iter_mut() {
            let already_timed = message
                .get("createdAt")
                .and_then(Value::as_str)
                .is_some_and(|value| !value.trim().is_empty());
            if !already_timed && let Some(formatted) = epoch_millis_to_rfc3339(end_ms) {
                message["createdAt"] = json!(formatted);
            }
        }
        return;
    }
    for (index, message) in messages.iter_mut().enumerate() {
        let already_timed = message
            .get("createdAt")
            .and_then(Value::as_str)
            .is_some_and(|value| !value.trim().is_empty());
        if already_timed {
            continue;
        }
        let offset = if count > 1 {
            (end_ms - start_ms) * index as i128 / (count.saturating_sub(1)) as i128
        } else {
            0
        };
        if let Some(formatted) = epoch_millis_to_rfc3339(start_ms + offset) {
            message["createdAt"] = json!(formatted);
        }
    }
}

fn epoch_millis_to_rfc3339(millis: i128) -> Option<String> {
    use time::{OffsetDateTime, format_description::well_known::Rfc3339};
    OffsetDateTime::from_unix_timestamp(i64::try_from(millis / 1000).ok()?)
        .ok()
        .and_then(|time| time.format(&Rfc3339).ok())
}

fn push_jsonl_record(
    adapter: HistoryAdapter,
    path: &Path,
    index: usize,
    line: &str,
    requested_session_id: Option<&str>,
    delegated_lineage: Option<&(String, String)>,
    grouped: &mut Vec<JsonlSessionAccumulator>,
    indexes: &mut HashMap<String, usize>,
) {
    let trimmed = line.trim();
    if trimmed.is_empty() {
        return;
    }
    if let Ok(value) = serde_json::from_str::<Value>(trimmed) {
        let Some(session_id) = native_session_id_or_layout(adapter, &value, path) else {
            return;
        };
        if requested_session_id.is_some_and(|requested| {
            requested != session_id
                && delegated_lineage
                    .is_none_or(|(task, owner)| task != requested && owner != requested)
        }) {
            return;
        }
        let group_index = match indexes.get(&session_id).copied() {
            Some(group_index) => group_index,
            None => {
                let group_index = grouped.len();
                indexes.insert(session_id.clone(), group_index);
                grouped.push(JsonlSessionAccumulator {
                    native_session_id: session_id,
                    messages: Vec::new(),
                    working_directory: None,
                });
                group_index
            }
        };
        let group = &mut grouped[group_index];
        if group.working_directory.is_none() {
            group.working_directory = claude_launch_working_directory(adapter, &value);
        }
        group
            .messages
            .extend(messages_from_json(adapter, path, index, &value));
    }
}

fn claude_launch_working_directory(adapter: HistoryAdapter, value: &Value) -> Option<String> {
    if adapter != HistoryAdapter::ClaudeCode {
        return None;
    }
    let object = value.as_object()?;
    let directory = ["cwd", "workingDirectory", "projectPath"]
        .iter()
        .find_map(|key| object.get(*key).and_then(Value::as_str))?
        .trim();
    if directory.is_empty()
        || directory.len() > 4096
        || !Path::new(directory).is_absolute()
        || directory.chars().any(char::is_control)
    {
        return None;
    }
    // Claude Code records the directory it was launched in, which is often the
    // home directory. An unbounded personal root must not reach the client as a
    // conversation's project directory.
    super::project_workspace::bounded_project_workspace(directory)
}

pub(crate) fn parse_json_sessions(
    adapter: HistoryAdapter,
    path: &Path,
    source_kind: &str,
    metadata: &fs::Metadata,
    scan_config: HistoryScanConfig,
) -> Vec<Value> {
    let file = match fs::File::open(path) {
        Ok(file) => file,
        Err(_) => return Vec::new(),
    };
    let value = match serde_json::from_reader::<_, Value>(BufReader::new(file)) {
        Ok(value) => value,
        Err(_) => return Vec::new(),
    };
    let sessions = collect_explicit_json_sessions(
        adapter,
        path,
        metadata,
        source_kind,
        &value,
        scan_config.single_session_id(),
    );
    if !sessions.is_empty() {
        return sessions;
    }
    let Some(native_session_id) = native_session_id_or_layout(adapter, &value, path) else {
        return Vec::new();
    };
    if scan_config
        .single_session_id()
        .is_some_and(|requested| requested != native_session_id)
    {
        return Vec::new();
    }
    let mut messages = Vec::<Value>::new();
    collect_messages_from_value(adapter, path, &value, &mut messages);
    if messages.is_empty() {
        return Vec::new();
    }
    vec![session_from_messages_with_title(
        adapter,
        path,
        metadata,
        source_kind,
        native_session_id,
        messages,
        extract_conversation_title(&value),
    )]
}

pub(crate) fn parse_text_session(
    adapter: HistoryAdapter,
    path: &Path,
    source_kind: &str,
    metadata: &fs::Metadata,
) -> Vec<Value> {
    let raw = match fs::read_to_string(path) {
        Ok(raw) => raw,
        Err(_) => return Vec::new(),
    };
    if raw.trim().is_empty() || !looks_like_text_conversation(&raw) {
        return Vec::new();
    }
    let created_at = system_time(metadata.modified().unwrap_or(SystemTime::UNIX_EPOCH));
    let messages = vec![json!({
        "id": message_id(adapter.id(), path, 0),
        "role": "transcript",
        "text": raw,
        "createdAt": created_at,
        "sourcePath": display_path(path)
    })];
    let Some(native_session_id) = text_layout_session_id(adapter, path) else {
        return Vec::new();
    };
    vec![session_from_messages(
        adapter,
        path,
        metadata,
        source_kind,
        native_session_id,
        messages,
    )]
}

pub(super) fn collect_explicit_json_sessions(
    adapter: HistoryAdapter,
    path: &Path,
    metadata: &fs::Metadata,
    source_kind: &str,
    value: &Value,
    requested_session_id: Option<&str>,
) -> Vec<Value> {
    let Some(object) = value.as_object() else {
        return Vec::new();
    };
    let mut sessions = Vec::<Value>::new();
    for key in ["sessions", "conversations", "chats", "chatSessions"] {
        let Some(Value::Array(items)) = object.get(key) else {
            continue;
        };
        for (index, item) in items.iter().enumerate() {
            let native_session_id = if adapter == HistoryAdapter::Copilot {
                crate::domain::conversation::snapshot_identity::extract_native_session_id(item)
            } else {
                extract_native_session_id(item)
            };
            let Some(native_session_id) = native_session_id.or_else(|| {
                (adapter == HistoryAdapter::Copilot)
                    .then(|| session_id(adapter.id(), path, &format!("{key}-{index}")))
            }) else {
                continue;
            };
            if requested_session_id.is_some_and(|requested| requested != native_session_id) {
                continue;
            }
            let mut messages = Vec::<Value>::new();
            collect_messages_from_value(adapter, path, item, &mut messages);
            if messages.is_empty() && adapter != HistoryAdapter::Copilot {
                continue;
            }
            sessions.push(session_from_messages_with_title(
                adapter,
                path,
                metadata,
                source_kind,
                native_session_id,
                messages,
                extract_conversation_title(item),
            ));
        }
    }
    sessions
}

pub(super) fn push_grouped_message(
    groups: &mut Vec<(String, Vec<Value>)>,
    session_id: String,
    message: Value,
) {
    if let Some((_, messages)) = groups.iter_mut().find(|(id, _)| *id == session_id) {
        messages.push(message);
    } else {
        groups.push((session_id, vec![message]));
    }
}

pub(super) fn collect_messages_from_value(
    adapter: HistoryAdapter,
    path: &Path,
    value: &Value,
    out: &mut Vec<Value>,
) {
    match value {
        Value::Array(items) => {
            for (index, item) in items.iter().enumerate() {
                if value_is_conversation_container(item) {
                    collect_messages_from_value(adapter, path, item, out);
                } else {
                    let messages = messages_from_json(adapter, path, index, item);
                    if messages.is_empty() {
                        collect_messages_from_value(adapter, path, item, out);
                    } else {
                        out.extend(messages);
                    }
                }
            }
        }
        Value::Object(object) => {
            let before = out.len();
            for key in [
                "messages",
                "conversation",
                "conversations",
                "transcript",
                "turns",
                "items",
                "entries",
                "sessions",
                "chats",
                "chatSessions",
            ] {
                if let Some(child) = object.get(key) {
                    collect_messages_from_value(adapter, path, child, out);
                }
            }
            if out.len() == before {
                let messages = messages_from_json(adapter, path, 0, value);
                if !messages.is_empty() {
                    out.extend(messages);
                }
            }
        }
        _ => {}
    }
}

pub(super) fn value_is_conversation_container(value: &Value) -> bool {
    let Some(object) = value.as_object() else {
        return false;
    };
    let has_container = [
        "messages",
        "conversation",
        "conversations",
        "transcript",
        "turns",
        "entries",
        "sessions",
        "chats",
        "chatSessions",
    ]
    .iter()
    .any(|key| object.contains_key(*key));
    let has_direct_message_text = [
        "text", "content", "prompt", "response", "answer", "summary", "value",
    ]
    .iter()
    .any(|key| object.contains_key(*key));
    has_container && !has_direct_message_text
}

pub(super) fn messages_from_json(
    adapter: HistoryAdapter,
    path: &Path,
    index: usize,
    value: &Value,
) -> Vec<Value> {
    let role = extract_role(value);
    let created_at = extract_timestamp(value);
    let model = extract_native_model(value);
    if let Some(blocks) = direct_native_content_blocks(value) {
        let mut messages = blocks
            .iter()
            .enumerate()
            .filter_map(|(block_index, block)| {
                message_from_native_content_block(
                    adapter,
                    path,
                    index,
                    block_index,
                    &role,
                    block,
                    created_at.clone(),
                )
            })
            .collect::<Vec<_>>();
        if !messages.is_empty() {
            let target_index = messages.len() - 1;
            if let Some(object) = messages[target_index].as_object_mut() {
                if let Some(usage) = extract_token_usage(value) {
                    object.insert("usage".to_string(), usage);
                    object.insert("usageScope".to_string(), json!("request-response"));
                }
                if let Some(model) = model {
                    object.insert("model".to_string(), json!(model));
                }
            }
            return messages;
        }
    }
    let kind = history_message_kind_from_semantic(&role);
    if kind != HistoryMessageKind::Text {
        return vec![structured_history_message(
            adapter, path, index, 0, kind, &role, value, created_at,
        )];
    }
    let Some(text) = extract_text(value) else {
        return Vec::new();
    };
    let Some(mut message) =
        plain_history_message(adapter, path, index, 0, &role, &text, created_at)
    else {
        return Vec::new();
    };
    if let Some(object) = message.as_object_mut() {
        if let Some(usage) = extract_token_usage(value) {
            object.insert("usage".to_string(), usage);
            object.insert("usageScope".to_string(), json!("request-response"));
        }
        if let Some(model) = model {
            object.insert("model".to_string(), json!(model));
        }
    }
    vec![message]
}

pub(super) fn direct_native_content_blocks(value: &Value) -> Option<&Vec<Value>> {
    value
        .get("message")
        .and_then(|message| message.get("content"))
        .or_else(|| value.get("content"))
        .and_then(Value::as_array)
}

pub(super) fn message_from_native_content_block(
    adapter: HistoryAdapter,
    path: &Path,
    index: usize,
    block_index: usize,
    outer_role: &str,
    block: &Value,
    created_at: Option<String>,
) -> Option<Value> {
    let semantic = native_content_semantic(block);
    let kind = history_message_kind_from_semantic(&semantic);
    if kind != HistoryMessageKind::Text {
        return Some(structured_history_message(
            adapter,
            path,
            index,
            block_index,
            kind,
            &semantic,
            block,
            created_at,
        ));
    }
    let text = extract_text(block)?;
    plain_history_message(
        adapter,
        path,
        index,
        block_index,
        outer_role,
        &text,
        created_at,
    )
}

pub(super) fn native_content_semantic(value: &Value) -> String {
    value
        .as_object()
        .and_then(|object| {
            ["type", "kind", "role", "eventType", "event_type"]
                .iter()
                .find_map(|key| object.get(*key).and_then(Value::as_str))
        })
        .unwrap_or_default()
        .to_string()
}

/// Directory-layout stores (for example Antigravity
/// `brain/<conversation-uuid>/…/transcript.jsonl`) carry the conversation
/// identity in the path rather than in the records. Falling back to the
/// literal "file" collapses every conversation of the agent into a single
/// native identity, which breaks dedupe and open-session refresh targeting.
fn native_session_id_or_layout(
    adapter: HistoryAdapter,
    value: &Value,
    path: &Path,
) -> Option<String> {
    extract_native_session_id(value)
        // A single-document session record commonly names itself with a bare
        // `id`; that is still record-native identity, never a file-stem or
        // positional placeholder. The snapshot identity owner already admits
        // it, so a usage or browse read of the same document cannot diverge.
        .or_else(|| {
            crate::domain::conversation::snapshot_identity::extract_native_session_id(value)
        })
        .or_else(|| {
            matches!(adapter, HistoryAdapter::Cursor | HistoryAdapter::ClaudeCode)
                .then(|| super::delegated_transcripts::transcript_conversation_id(path))
                .flatten()
        })
        .or_else(|| directory_component_session_id(path))
}

fn text_layout_session_id(adapter: HistoryAdapter, path: &Path) -> Option<String> {
    if matches!(adapter, HistoryAdapter::Cursor | HistoryAdapter::ClaudeCode) {
        return super::delegated_transcripts::transcript_conversation_id(path);
    }
    directory_component_session_id(path).or_else(|| {
        path.file_stem()
            .and_then(|value| value.to_str())
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_string)
    })
}

fn directory_component_session_id(path: &Path) -> Option<String> {
    path.components().rev().skip(1).find_map(|component| {
        let value = component.as_os_str().to_str()?;
        is_conversation_uuid_component(value).then(|| value.to_string())
    })
}

fn is_conversation_uuid_component(value: &str) -> bool {
    let bytes = value.as_bytes();
    bytes.len() == 36
        && bytes.iter().enumerate().all(|(index, byte)| {
            if matches!(index, 8 | 13 | 18 | 23) {
                *byte == b'-'
            } else {
                byte.is_ascii_hexdigit()
            }
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs::File;
    use std::time::{Duration, SystemTime};

    #[test]
    fn missing_message_times_interpolate_across_the_transcript_interval() {
        let directory =
            std::env::temp_dir().join(format!("lico-transcript-times-{}", std::process::id()));
        let _ = fs::remove_dir_all(&directory);
        fs::create_dir_all(&directory).unwrap();
        let path = directory.join("conversation.jsonl");
        fs::write(&path, "ignored\n").unwrap();
        let start = SystemTime::UNIX_EPOCH + Duration::from_secs(1_784_000_000);
        let end = start + Duration::from_secs(600);
        File::create(&path).unwrap().set_modified(end).unwrap();
        let directory_handle = File::open(&directory).unwrap();
        directory_handle.set_modified(start).unwrap();
        drop(directory_handle);

        let session = json!({
            "nativeSessionId": "conversation",
            "messages": [
                {"role": "user", "text": "Start"},
                {"role": "agent", "text": "Working"},
                {"role": "user", "text": "Delegate"},
                {"role": "agent", "text": "Done"}
            ]
        });
        let mut messages = session["messages"].as_array().unwrap().clone();
        backfill_transcript_message_times(&mut messages, &path);
        let messages = messages;
        let keys = messages
            .iter()
            .map(|message| {
                message["createdAt"]
                    .as_str()
                    .unwrap()
                    .parse::<i128>()
                    .unwrap_or_else(|_| {
                        OffsetDateTime::parse(
                            message["createdAt"].as_str().unwrap(),
                            &time::format_description::well_known::Rfc3339,
                        )
                        .unwrap()
                        .unix_timestamp() as i128
                    })
            })
            .collect::<Vec<_>>();
        assert!(keys.windows(2).all(|pair| pair[0] < pair[1]));
        assert_eq!(keys[0], 1_784_000_000);
        assert_eq!(keys[3], 1_784_000_600);
        let _ = fs::remove_dir_all(&directory);
    }

    #[test]
    fn messages_with_their_own_timestamps_are_left_alone() {
        let directory =
            std::env::temp_dir().join(format!("lico-transcript-keep-{}", std::process::id()));
        let _ = fs::remove_dir_all(&directory);
        fs::create_dir_all(&directory).unwrap();
        let path = directory.join("conversation.jsonl");
        fs::write(&path, "ignored\n").unwrap();
        let start = SystemTime::UNIX_EPOCH + Duration::from_secs(1_784_000_000);
        let end = start + Duration::from_secs(600);
        File::create(&path).unwrap().set_modified(end).unwrap();
        File::open(&directory).unwrap().set_modified(start).unwrap();

        let session = json!({
            "nativeSessionId": "conversation",
            "messages": [
                {"role": "user", "text": "Keep me", "createdAt": "2026-07-20T00:00:00Z"},
                {"role": "agent", "text": "Backfilled"}
            ]
        });
        let mut messages = session["messages"].as_array().unwrap().clone();
        backfill_transcript_message_times(&mut messages, &path);
        assert_eq!(messages[0]["createdAt"], "2026-07-20T00:00:00Z");
        assert!(
            messages[1]["createdAt"]
                .as_str()
                .is_some_and(|v| !v.is_empty())
        );
        assert_ne!(messages[1]["createdAt"], "2026-07-20T00:00:00Z");
        let _ = fs::remove_dir_all(&directory);
    }
}
