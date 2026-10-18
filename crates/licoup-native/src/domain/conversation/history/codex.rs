//! Codex rollout parser and rollout-local usage projection.

use super::*;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

const CODEX_RUNTIME_TAIL_BYTES: u64 = 8 * 1024 * 1024;
pub(super) const CODEX_RUNTIME_SCAN_CHUNK_BYTES: u64 = 64 * 1024;

#[derive(Debug)]
pub(super) struct CodexRuntimeObservation {
    open_rollouts: BTreeSet<PathBuf>,
}

impl CodexRuntimeObservation {
    /// Capture this refresh's environment evidence through the installed port.
    pub(super) fn capture() -> Self {
        Self {
            open_rollouts: super::runtime_observation::port().open_rollouts(),
        }
    }

    #[cfg(test)]
    pub(super) fn from_open_rollouts(paths: impl IntoIterator<Item = PathBuf>) -> Self {
        Self {
            open_rollouts: paths
                .into_iter()
                .map(|path| fs::canonicalize(&path).unwrap_or(path))
                .collect(),
        }
    }

    pub(super) fn is_running(&self, path: &Path) -> bool {
        let normalized = fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
        self.open_rollouts.contains(&normalized) && codex_rollout_has_active_task(path)
    }
}

/// Codex persists task lifecycle facts in the rollout that owns the thread.
/// Scan backwards in small chunks and stop at the newest lifecycle record.
/// The byte window is explicit so a pathological transcript cannot turn a
/// sidebar refresh into an unbounded read; completed tasks normally resolve
/// from the first chunk at the end of the file.
pub(super) fn codex_rollout_has_active_task(path: &Path) -> bool {
    let Ok(mut file) = fs::File::open(path) else {
        return false;
    };
    let Ok(metadata) = file.metadata() else {
        return false;
    };
    let scan_start = metadata.len().saturating_sub(CODEX_RUNTIME_TAIL_BYTES);
    let mut cursor = metadata.len();
    let mut later_line_prefix = Vec::<u8>::new();
    while cursor > scan_start {
        let chunk_start = cursor
            .saturating_sub(CODEX_RUNTIME_SCAN_CHUNK_BYTES)
            .max(scan_start);
        let chunk_len = (cursor - chunk_start) as usize;
        if file.seek(SeekFrom::Start(chunk_start)).is_err() {
            return false;
        }
        let mut chunk = vec![0; chunk_len];
        if file.read_exact(&mut chunk).is_err() {
            return false;
        }
        chunk.extend_from_slice(&later_line_prefix);

        let complete_start = if chunk_start == 0 {
            0
        } else if let Some(first_newline) = chunk.iter().position(|byte| *byte == b'\n') {
            later_line_prefix = chunk[..first_newline].to_vec();
            first_newline + 1
        } else {
            later_line_prefix = chunk;
            cursor = chunk_start;
            continue;
        };
        if let Some(running) = newest_codex_task_lifecycle(&chunk[complete_start..]) {
            return running;
        }
        cursor = chunk_start;
    }
    false
}

fn newest_codex_task_lifecycle(lines: &[u8]) -> Option<bool> {
    for line in lines.rsplit(|byte| *byte == b'\n') {
        if line.is_empty() {
            continue;
        }
        let Ok(value) = serde_json::from_slice::<Value>(line) else {
            continue;
        };
        if value.get("type").and_then(Value::as_str) != Some("event_msg") {
            continue;
        }
        match value
            .get("payload")
            .and_then(|payload| payload.get("type"))
            .and_then(Value::as_str)
        {
            Some("task_started") => return Some(true),
            Some(
                "task_complete" | "task_cancelled" | "turn_aborted" | "turn_cancelled"
                | "turn_completed",
            ) => return Some(false),
            _ => {}
        }
    }
    None
}

pub(super) fn mark_codex_runtime_activity(
    sessions: &mut [Value],
    observation: &CodexRuntimeObservation,
) {
    for session in sessions {
        let running = session
            .get("sourcePath")
            .and_then(Value::as_str)
            .filter(|path| !path.trim().is_empty())
            .is_some_and(|path| observation.is_running(Path::new(path)));
        let Some(object) = session.as_object_mut() else {
            continue;
        };
        if running {
            object.insert("running".to_string(), json!(true));
        } else {
            object.remove("running");
        }
    }
}

#[derive(Debug)]
pub(super) struct CodexRolloutGroup {
    session_id: String,
    native_turn_id: Option<String>,
    parent_session_id: Option<String>,
    subagent_title: Option<String>,
    is_subagent: bool,
    messages: Vec<Value>,
    cwd: Option<String>,
    matched_terms: BTreeSet<String>,
    message_count: usize,
    tool_call_count: usize,
    preview_count: usize,
    opening_user_title: Option<String>,
    opening_created_at: Option<String>,
}

pub(crate) fn parse_codex_rollout_sessions(
    path: &Path,
    source_kind: &str,
    metadata: &fs::Metadata,
    scan_config: HistoryScanConfig,
) -> Option<Vec<Value>> {
    let mut groups = Vec::<CodexRolloutGroup>::new();
    let mut current_session_id = rollout_session_id_from_filename(path);
    let mut saw_rollout_record = false;

    let file = fs::File::open(path).ok()?;
    for (index, line) in BufReader::new(file).lines().enumerate() {
        let Ok(line) = line else {
            continue;
        };
        parse_codex_rollout_line(
            path,
            index,
            &line,
            &scan_config,
            &mut current_session_id,
            &mut saw_rollout_record,
            &mut groups,
        );
    }

    if !saw_rollout_record {
        return None;
    }

    codex_rollout_groups_to_sessions(groups, path, metadata, source_kind, &scan_config)
}

/// Streaming projection for browse rows and lazy child cards. Scan every record
/// for exact counts and opening facts, retaining only the newest message window
/// except for the explicitly selected session whose cursor is resolved later.
pub(super) fn parse_codex_rollout_bounded_sessions(
    path: &Path,
    source_kind: &str,
    metadata: &fs::Metadata,
    scan_config: HistoryScanConfig,
    message_limit: usize,
    complete_session: Option<&str>,
) -> Option<Vec<Value>> {
    let mut groups = Vec::<CodexRolloutGroup>::new();
    let mut current_session_id = rollout_session_id_from_filename(path);
    let mut saw_rollout_record = false;
    let file = fs::File::open(path).ok()?;
    for (index, line) in BufReader::new(file).lines().enumerate() {
        let Ok(line) = line else {
            continue;
        };
        parse_codex_rollout_line(
            path,
            index,
            &line,
            &scan_config,
            &mut current_session_id,
            &mut saw_rollout_record,
            &mut groups,
        );
        for group in &mut groups {
            if complete_session == Some(group.session_id.as_str()) {
                continue;
            }
            let overflow = group.messages.len().saturating_sub(message_limit);
            if overflow > 0 {
                group.messages.drain(0..overflow);
            }
        }
    }
    if !saw_rollout_record {
        return None;
    }
    codex_rollout_groups_to_sessions(groups, path, metadata, source_kind, &scan_config)
}

/// Project parsed rollout groups into session DTOs.
pub(super) fn codex_rollout_groups_to_sessions(
    groups: Vec<CodexRolloutGroup>,
    path: &Path,
    metadata: &fs::Metadata,
    source_kind: &str,
    scan_config: &HistoryScanConfig,
) -> Option<Vec<Value>> {
    Some(
        groups
            .into_iter()
            .filter(|group| {
                !group.messages.is_empty()
                    && (!scan_config.has_match_filters() || !group.matched_terms.is_empty())
            })
            .map(|group| {
                let message_count = group.message_count.max(group.messages.len());
                let mut session = session_from_messages_with_title(
                    HistoryAdapter::Codex,
                    path,
                    metadata,
                    source_kind,
                    group.session_id,
                    group.messages,
                    group.opening_user_title,
                );
                if let Some(object) = session.as_object_mut() {
                    object.insert("messageCount".to_string(), json!(message_count));
                    object.insert("toolCallCount".to_string(), json!(group.tool_call_count));
                    if let Some(created_at) = group.opening_created_at {
                        object.insert("createdAt".to_string(), json!(created_at));
                    }
                    if scan_config.has_match_filters() {
                        object.insert("archiveDiscoveryHasConversation".to_string(), json!(true));
                        object.insert(
                            "archiveDiscoveryMatchedTerms".to_string(),
                            json!(group.matched_terms.into_iter().collect::<Vec<_>>()),
                        );
                        object.insert(
                            "messagesTruncatedForArchiveDiscovery".to_string(),
                            json!(true),
                        );
                    }
                    if let Some(cwd) = group.cwd {
                        object.insert("workingDirectory".to_string(), json!(cwd));
                    }
                    if let Some(parent_session_id) = group.parent_session_id {
                        object.insert("parentSessionId".to_string(), json!(parent_session_id));
                    }
                    if group.is_subagent {
                        object.insert("delegatedSubagent".to_string(), json!(true));
                    }
                    if let Some(title) = group.subagent_title {
                        object.insert("subagentTitle".to_string(), json!(title));
                    }
                }
                session
            })
            .collect(),
    )
}

pub(super) fn parse_codex_rollout_line(
    path: &Path,
    index: usize,
    line: &str,
    scan_config: &HistoryScanConfig,
    current_session_id: &mut Option<String>,
    saw_rollout_record: &mut bool,
    groups: &mut Vec<CodexRolloutGroup>,
) {
    let trimmed = line.trim();
    if trimmed.is_empty() {
        return;
    }
    let Ok(value) = serde_json::from_str::<Value>(trimmed) else {
        return;
    };
    let Some(event_type) = value.get("type").and_then(Value::as_str) else {
        return;
    };
    let Some(payload) = value.get("payload") else {
        return;
    };
    if !matches!(
        event_type,
        "session_meta" | "turn_context" | "response_item" | "event_msg"
    ) {
        return;
    }
    *saw_rollout_record = true;

    if event_type == "session_meta" {
        if let Some(session_id) = find_string(payload, &["id", "sessionId", "session_id"])
            .filter(|value| !value.trim().is_empty())
        {
            *current_session_id = Some(session_id);
        }
    }

    let session_id = current_session_id
        .clone()
        .unwrap_or_else(|| "file".to_string());
    let cwd = find_string(payload, &["cwd", "workingDirectory", "projectPath"])
        .filter(|value| !value.trim().is_empty());

    if event_type == "session_meta" {
        update_codex_rollout_group_lineage(groups, &session_id, payload);
    }
    if !groups.iter().any(|group| group.session_id == session_id) {
        update_codex_rollout_group_cwd(groups, session_id.clone(), cwd.clone());
    }
    let group = groups
        .iter_mut()
        .find(|group| group.session_id == session_id)
        .expect("session group exists");
    let lifecycle = payload.get("type").and_then(Value::as_str);
    if event_type == "session_meta"
        || event_type == "turn_context"
        || (event_type == "event_msg" && lifecycle == Some("task_started"))
    {
        group.native_turn_id = if event_type == "session_meta" {
            None
        } else {
            find_string(payload, &["turn_id", "turnId"]).filter(|id| !id.is_empty())
        };
    }
    let native_turn_id =
        find_string(payload, &["turn_id", "turnId"]).or_else(|| group.native_turn_id.clone());
    if let Some(mut message) = codex_rollout_message(path, index, event_type, payload, &value) {
        if matches!(
            message.get("role").and_then(Value::as_str),
            Some("agent" | "assistant")
        ) {
            if let Some(id) = native_turn_id {
                message["sourceTurnId"] = json!(id);
            }
            if event_type == "response_item"
                && payload.get("type").and_then(Value::as_str) == Some("message")
            {
                if let Some(id) = payload
                    .get("id")
                    .and_then(Value::as_str)
                    .filter(|id| !id.is_empty())
                {
                    message["sourceMessageId"] = json!(id);
                }
            }
        }
        push_codex_rollout_message(groups, session_id.clone(), message, cwd, scan_config);
    } else if cwd.is_some() {
        update_codex_rollout_group_cwd(groups, session_id.clone(), cwd);
    }
    if event_type == "event_msg"
        && matches!(
            lifecycle,
            Some(
                "task_complete"
                    | "task_cancelled"
                    | "turn_aborted"
                    | "turn_cancelled"
                    | "turn_completed"
            )
        )
    {
        if let Some(group) = groups
            .iter_mut()
            .find(|group| group.session_id == session_id)
        {
            group.native_turn_id = None;
        }
    }
}

pub(super) fn update_codex_rollout_group_cwd(
    groups: &mut Vec<CodexRolloutGroup>,
    session_id: String,
    cwd: Option<String>,
) {
    if let Some(group) = groups
        .iter_mut()
        .find(|group| group.session_id == session_id)
    {
        if group.cwd.is_none() {
            group.cwd = cwd;
        }
        return;
    }
    groups.push(CodexRolloutGroup {
        session_id,
        native_turn_id: None,
        parent_session_id: None,
        subagent_title: None,
        is_subagent: false,
        messages: Vec::new(),
        cwd,
        matched_terms: BTreeSet::new(),
        message_count: 0,
        tool_call_count: 0,
        preview_count: 0,
        opening_user_title: None,
        opening_created_at: None,
    });
}

pub(super) fn update_codex_rollout_group_lineage(
    groups: &mut Vec<CodexRolloutGroup>,
    session_id: &str,
    payload: &Value,
) {
    if !groups.iter().any(|group| group.session_id == session_id) {
        update_codex_rollout_group_cwd(groups, session_id.to_string(), None);
    }
    let Some(group) = groups
        .iter_mut()
        .find(|group| group.session_id == session_id)
    else {
        return;
    };
    (
        group.parent_session_id,
        group.is_subagent,
        group.subagent_title,
    ) = codex_rollout_lineage(payload, session_id);
}

pub(super) fn codex_rollout_lineage(
    payload: &Value,
    session_id: &str,
) -> (Option<String>, bool, Option<String>) {
    let parent_session_id = find_nested_string(
        payload,
        &[
            "forked_from_id",
            "forkedFromId",
            "parent_session_id",
            "parentSessionId",
            "parent_thread_id",
            "parentThreadId",
        ],
        0,
    )
    .filter(|parent_id| parent_id != session_id);
    let is_subagent = contains_nested_key(payload, "subagent", 0)
        || contains_nested_key(payload, "thread_spawn", 0)
        || contains_nested_key(payload, "threadSpawn", 0);
    let subagent_title = find_nested_string(
        payload,
        &["agent_nickname", "agentNickname", "agent_role", "agentRole"],
        0,
    );
    (parent_session_id, is_subagent, subagent_title)
}

pub(super) fn find_nested_string(value: &Value, keys: &[&str], _depth: usize) -> Option<String> {
    let mut pending = vec![value];
    while let Some(candidate) = pending.pop() {
        match candidate {
            Value::Object(object) => {
                for key in keys {
                    if let Some(text) = object.get(*key).and_then(Value::as_str) {
                        if !text.trim().is_empty() {
                            return Some(text.to_string());
                        }
                    }
                }
                pending.extend(object.values().rev());
            }
            Value::Array(items) => pending.extend(items.iter().rev()),
            _ => {}
        }
    }
    None
}

pub(super) fn contains_nested_key(value: &Value, needle: &str, _depth: usize) -> bool {
    let mut pending = vec![value];
    while let Some(candidate) = pending.pop() {
        match candidate {
            Value::Object(object) => {
                if object.contains_key(needle) {
                    return true;
                }
                pending.extend(object.values());
            }
            Value::Array(items) => pending.extend(items),
            _ => {}
        }
    }
    false
}

pub(super) fn push_codex_rollout_message(
    groups: &mut Vec<CodexRolloutGroup>,
    session_id: String,
    message: Value,
    cwd: Option<String>,
    scan_config: &HistoryScanConfig,
) {
    let matched_terms = codex_rollout_message_matched_terms(&message, cwd.as_deref(), scan_config);
    if let Some(group) = groups
        .iter_mut()
        .find(|group| group.session_id == session_id)
    {
        if group.cwd.is_none() {
            group.cwd = cwd;
        }
        push_codex_rollout_message_into_group(group, message, matched_terms, scan_config);
        return;
    }
    let mut group = CodexRolloutGroup {
        session_id,
        native_turn_id: None,
        parent_session_id: None,
        subagent_title: None,
        is_subagent: false,
        messages: Vec::new(),
        cwd,
        matched_terms: BTreeSet::new(),
        message_count: 0,
        tool_call_count: 0,
        preview_count: 0,
        opening_user_title: None,
        opening_created_at: None,
    };
    push_codex_rollout_message_into_group(&mut group, message, matched_terms, scan_config);
    groups.push(group);
}

pub(super) fn push_codex_rollout_message_into_group(
    group: &mut CodexRolloutGroup,
    message: Value,
    matched_terms: Vec<String>,
    scan_config: &HistoryScanConfig,
) {
    let is_conversation = history_message_is_matchable(&message);
    group.message_count += 1;
    if group.opening_created_at.is_none() {
        group.opening_created_at = message
            .get("createdAt")
            .and_then(Value::as_str)
            .map(str::to_string);
    }
    if super::session_merge::subagent_card_child_message_is_tool_step(&message) {
        group.tool_call_count += 1;
    }
    if group.opening_user_title.is_none()
        && matches!(
            message.get("role").and_then(Value::as_str),
            Some("user" | "human")
        )
    {
        group.opening_user_title = message
            .get("text")
            .and_then(Value::as_str)
            .map(title_from_text);
    }
    for term in matched_terms {
        group.matched_terms.insert(term);
    }
    if !scan_config.has_match_filters() {
        group.messages.push(message);
        return;
    }
    if !is_conversation {
        return;
    }
    let is_match = !group.matched_terms.is_empty()
        && codex_rollout_message_matched_terms(&message, group.cwd.as_deref(), scan_config)
            .into_iter()
            .any(|term| group.matched_terms.contains(&term));
    if is_match || group.preview_count < ARCHIVE_DISCOVERY_PREVIEW_MESSAGES {
        if !is_match {
            group.preview_count += 1;
        }
        group
            .messages
            .push(truncate_codex_rollout_preview_message(message));
    }
}

pub(super) fn codex_rollout_message_matched_terms(
    message: &Value,
    cwd: Option<&str>,
    scan_config: &HistoryScanConfig,
) -> Vec<String> {
    if !scan_config.has_match_filters() || !history_message_is_matchable(message) {
        return Vec::new();
    }
    let text = message
        .get("text")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let path_text = cwd.unwrap_or_default();
    scan_config.matched_terms_in_text_and_path(text, path_text)
}

pub(super) fn truncate_codex_rollout_preview_message(mut message: Value) -> Value {
    if let Some(object) = message.as_object_mut() {
        if let Some(text) = object.get("text").and_then(Value::as_str) {
            if text.chars().count() > ARCHIVE_DISCOVERY_PREVIEW_TEXT_CHARS {
                object.insert(
                    "text".to_string(),
                    json!(format!(
                        "{}...",
                        text.chars()
                            .take(ARCHIVE_DISCOVERY_PREVIEW_TEXT_CHARS)
                            .collect::<String>()
                    )),
                );
            }
        }
    }
    message
}

pub(super) fn codex_rollout_message(
    path: &Path,
    index: usize,
    event_type: &str,
    payload: &Value,
    raw_value: &Value,
) -> Option<Value> {
    match event_type {
        "session_meta" | "turn_context" => None,
        "response_item" => codex_response_item_message(path, index, payload, raw_value),
        "event_msg" => codex_event_message(path, index, payload, raw_value),
        _ => None,
    }
}

pub(super) fn codex_event_message(
    path: &Path,
    index: usize,
    payload: &Value,
    raw_value: &Value,
) -> Option<Value> {
    let item_type = payload
        .get("type")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let normalized = normalize_history_message_semantic(item_type);
    if normalized.ends_with("-delta")
        || matches!(
            normalized.as_str(),
            "agent-message" | "user-message" | "task-started" | "task-complete"
        )
    {
        return None;
    }
    let kind = if matches!(
        normalized.as_str(),
        "token-count" | "usage" | "turn-metadata"
    ) {
        HistoryMessageKind::Metadata
    } else if matches!(normalized.as_str(), "turn-aborted" | "stream-error") {
        HistoryMessageKind::Error
    } else {
        history_message_kind_from_semantic(item_type)
    };
    if matches!(kind, HistoryMessageKind::Text | HistoryMessageKind::Event)
        && normalized != "warning"
    {
        return None;
    }
    Some(structured_history_message(
        HistoryAdapter::Codex,
        path,
        index,
        0,
        kind,
        item_type,
        payload,
        extract_timestamp(raw_value),
    ))
}

pub(super) fn codex_response_item_message(
    path: &Path,
    index: usize,
    payload: &Value,
    raw_value: &Value,
) -> Option<Value> {
    let item_type = payload
        .get("type")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let structured_kind = history_message_kind_from_semantic(item_type);
    if structured_kind != HistoryMessageKind::Text {
        return Some(structured_history_message(
            HistoryAdapter::Codex,
            path,
            index,
            0,
            structured_kind,
            item_type,
            payload,
            extract_timestamp(raw_value),
        ));
    }
    let text = match item_type {
        _ => payload
            .get("content")
            .or_else(|| payload.get("text"))
            .or_else(|| payload.get("summary"))
            .and_then(extract_text),
    }?;
    let role = extract_role(payload);
    let images = if matches!(role.as_str(), "user" | "human") {
        extract_user_image_attachments(&text)
    } else {
        Vec::new()
    };
    let text = clean_native_message_text(HistoryAdapter::Codex, &role, &text);
    if text.is_none() && images.is_empty() {
        return None;
    }
    let text = text.unwrap_or_default();
    if metadata_like_text(&text) {
        return None;
    }
    let mut message = json!({
        "id": message_id(HistoryAdapter::Codex.id(), path, index),
        "role": role,
        "text": text,
        "createdAt": extract_timestamp(raw_value).unwrap_or_else(|| {
            OffsetDateTime::now_utc()
                .format(&Rfc3339)
                .unwrap_or_else(|_| "1970-01-01T00:00:00Z".to_string())
        }),
        "sourcePath": display_path(path),
        "sourceEventType": "response_item",
        "sourceItemType": item_type
    });
    if !images.is_empty()
        && let Some(object) = message.as_object_mut()
    {
        object.insert("images".to_string(), json!(images));
    }
    if let Some(usage) = extract_token_usage(payload).or_else(|| extract_token_usage(raw_value)) {
        if let Some(object) = message.as_object_mut() {
            object.insert("usage".to_string(), usage);
        }
    }
    Some(message)
}

pub(super) fn rollout_session_id_from_filename(path: &Path) -> Option<String> {
    let stem = path.file_stem()?.to_str()?;
    let parts = stem.split('-').collect::<Vec<_>>();
    if parts.len() < 5 {
        return None;
    }
    for window in parts.windows(5) {
        let candidate = window.join("-");
        if looks_like_uuid(&candidate) {
            return Some(candidate);
        }
    }
    None
}

pub(super) fn looks_like_uuid(value: &str) -> bool {
    let parts = value.split('-').collect::<Vec<_>>();
    let lengths = [8usize, 4, 4, 4, 12];
    parts.len() == lengths.len()
        && parts.iter().zip(lengths).all(|(part, length)| {
            part.len() == length && part.chars().all(|ch| ch.is_ascii_hexdigit())
        })
}
