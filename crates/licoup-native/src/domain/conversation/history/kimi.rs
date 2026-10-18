//! Kimi Code wire-history parser.

use super::*;

pub(crate) fn parse_kimi_code_wire_session(
    path: &Path,
    source_kind: &str,
    metadata: &fs::Metadata,
) -> Vec<Value> {
    let file = match fs::File::open(path) {
        Ok(file) => file,
        Err(_) => return Vec::new(),
    };
    let mut messages = Vec::<Value>::new();
    let mut fallback_user_messages = Vec::<Value>::new();
    let mut fallback_agent_messages = Vec::<Value>::new();
    let mut saw_user = false;
    let mut saw_agent = false;
    let mut assistant_text = String::new();
    let mut assistant_index = 0usize;
    let mut assistant_created_at = None::<String>;
    let mut assistant_group = None::<String>;
    let mut reasoning_text = String::new();
    let mut reasoning_summaries = Vec::<Value>::new();
    let mut reasoning_index = 0usize;
    let mut reasoning_created_at = None::<String>;
    let mut reasoning_group = None::<String>;
    for (index, line) in BufReader::new(file).lines().enumerate() {
        let Ok(line) = line else {
            continue;
        };
        let Ok(value) = serde_json::from_str::<Value>(line.trim()) else {
            continue;
        };
        match value.get("type").and_then(Value::as_str) {
            Some("turn.prompt") => {
                flush_kimi_code_assistant(
                    path,
                    &mut messages,
                    &mut assistant_text,
                    assistant_index,
                    assistant_created_at.take(),
                );
                assistant_group = None;
                flush_kimi_code_reasoning(
                    path,
                    &mut messages,
                    &mut reasoning_text,
                    &mut reasoning_summaries,
                    reasoning_index,
                    reasoning_created_at.take(),
                );
                reasoning_group = None;
                if let Some(text) = value.get("input").and_then(extract_text)
                    && let Some(message) = plain_history_message(
                        HistoryAdapter::KimiCode,
                        path,
                        index,
                        0,
                        "user",
                        &text,
                        extract_timestamp(&value),
                    )
                {
                    messages.push(message);
                    saw_user = true;
                }
            }
            Some("context.append_message") => {
                let mut message_value = value.get("message").cloned().unwrap_or(Value::Null);
                if let Some(object) = message_value.as_object_mut()
                    && !object.contains_key("time")
                    && let Some(time) = value.get("time")
                {
                    object.insert("time".to_string(), time.clone());
                }
                for parsed in
                    messages_from_json(HistoryAdapter::KimiCode, path, index, &message_value)
                {
                    match parsed.get("role").and_then(Value::as_str) {
                        Some("user" | "human") => fallback_user_messages.push(parsed),
                        Some("agent" | "assistant" | "model" | "ai") => {
                            fallback_agent_messages.push(parsed)
                        }
                        _ => {}
                    }
                }
            }
            Some("context.append_loop_event") => {
                let event = value.get("event").unwrap_or(&value);
                let semantic = event
                    .get("type")
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                let created_at = extract_timestamp(&value).or_else(|| extract_timestamp(event));
                match semantic {
                    "content.part" => {
                        let part = event.get("part").unwrap_or(event);
                        let group = kimi_code_content_group(&value, event);
                        match part.get("type").and_then(Value::as_str) {
                            Some("text") => {
                                flush_kimi_code_reasoning(
                                    path,
                                    &mut messages,
                                    &mut reasoning_text,
                                    &mut reasoning_summaries,
                                    reasoning_index,
                                    reasoning_created_at.take(),
                                );
                                reasoning_group = None;
                                if assistant_group
                                    .as_deref()
                                    .is_some_and(|active| active != group)
                                {
                                    flush_kimi_code_assistant(
                                        path,
                                        &mut messages,
                                        &mut assistant_text,
                                        assistant_index,
                                        assistant_created_at.take(),
                                    );
                                }
                                if let Some(text) = part.get("text").and_then(extract_text)
                                    && !text.is_empty()
                                {
                                    if assistant_text.is_empty() {
                                        assistant_index = index;
                                        assistant_created_at = created_at;
                                        assistant_group = Some(group);
                                    }
                                    assistant_text.push_str(&text);
                                    saw_agent = true;
                                }
                            }
                            Some("think") => {
                                flush_kimi_code_assistant(
                                    path,
                                    &mut messages,
                                    &mut assistant_text,
                                    assistant_index,
                                    assistant_created_at.take(),
                                );
                                assistant_group = None;
                                if reasoning_group
                                    .as_deref()
                                    .is_some_and(|active| active != group)
                                {
                                    flush_kimi_code_reasoning(
                                        path,
                                        &mut messages,
                                        &mut reasoning_text,
                                        &mut reasoning_summaries,
                                        reasoning_index,
                                        reasoning_created_at.take(),
                                    );
                                }
                                if reasoning_text.is_empty() {
                                    reasoning_index = index;
                                    reasoning_created_at = created_at;
                                    reasoning_group = Some(group);
                                }
                                if let Some(text) = part
                                    .get("think")
                                    .or_else(|| part.get("text"))
                                    .and_then(extract_text)
                                {
                                    reasoning_text.push_str(&text);
                                }
                                for key in ["summary", "reasoningSummary", "reasoning_summary"] {
                                    if let Some(summary) = part.get(key) {
                                        reasoning_summaries.push(summary.clone());
                                    }
                                }
                            }
                            _ => {}
                        }
                    }
                    "tool.call" | "tool.result" => {
                        flush_kimi_code_assistant(
                            path,
                            &mut messages,
                            &mut assistant_text,
                            assistant_index,
                            assistant_created_at.take(),
                        );
                        assistant_group = None;
                        flush_kimi_code_reasoning(
                            path,
                            &mut messages,
                            &mut reasoning_text,
                            &mut reasoning_summaries,
                            reasoning_index,
                            reasoning_created_at.take(),
                        );
                        reasoning_group = None;
                        messages.push(structured_history_message(
                            HistoryAdapter::KimiCode,
                            path,
                            index,
                            0,
                            if semantic == "tool.call" {
                                HistoryMessageKind::ToolCall
                            } else {
                                HistoryMessageKind::ToolResult
                            },
                            semantic,
                            event,
                            created_at,
                        ));
                    }
                    _ => {}
                }
            }
            Some("usage.record" | "StatusUpdate" | "status.update" | "status_update") => {
                if !saw_user {
                    messages.append(&mut fallback_user_messages);
                }
                if !saw_agent {
                    messages.append(&mut fallback_agent_messages);
                }
                flush_kimi_code_assistant(
                    path,
                    &mut messages,
                    &mut assistant_text,
                    assistant_index,
                    assistant_created_at.take(),
                );
                assistant_group = None;
                flush_kimi_code_reasoning(
                    path,
                    &mut messages,
                    &mut reasoning_text,
                    &mut reasoning_summaries,
                    reasoning_index,
                    reasoning_created_at.take(),
                );
                reasoning_group = None;
                if let Some(message) = kimi_code_usage_message(path, index, &value) {
                    messages.push(message);
                }
            }
            _ => {}
        }
    }
    flush_kimi_code_assistant(
        path,
        &mut messages,
        &mut assistant_text,
        assistant_index,
        assistant_created_at,
    );
    flush_kimi_code_reasoning(
        path,
        &mut messages,
        &mut reasoning_text,
        &mut reasoning_summaries,
        reasoning_index,
        reasoning_created_at,
    );
    if !saw_user {
        messages.extend(fallback_user_messages);
    }
    if !saw_agent {
        messages.extend(fallback_agent_messages);
    }
    if messages.is_empty() {
        return Vec::new();
    }
    super::generic::backfill_transcript_message_times(&mut messages, path);
    let session_root = path.ancestors().nth(3);
    let state = session_root
        .map(|root| root.join("state.json"))
        .and_then(|state_path| fs::read_to_string(state_path).ok())
        .and_then(|raw| serde_json::from_str::<Value>(&raw).ok());
    let explicit_title = state.as_ref().and_then(extract_conversation_title);
    let mut session = session_from_messages_with_title(
        HistoryAdapter::KimiCode,
        path,
        metadata,
        source_kind,
        kimi_code_native_session_id(path),
        messages,
        explicit_title,
    );
    mark_kimi_code_working_directory(state.as_ref(), &mut session);
    mark_kimi_code_delegated_subagent(path, session_root, state.as_ref(), &mut session);
    vec![session]
}

fn mark_kimi_code_working_directory(state: Option<&Value>, session: &mut Value) {
    let Some(directory) = state
        .and_then(|state| state.get("workDir"))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| {
            !value.is_empty()
                && value.len() <= 4096
                && !value.contains('\0')
                && Path::new(value).is_absolute()
        })
    else {
        return;
    };
    if let Some(object) = session.as_object_mut() {
        object.insert("workingDirectory".to_string(), json!(directory));
    }
}

/// Kimi Code keeps every agent of one conversation under
/// `<session>/agents/<id>/wire.jsonl`; non-`main` agents are delegated
/// subagents of the same thread, so mark them for the shared merge that
/// collapses them into the main session as subagent cards.
fn mark_kimi_code_delegated_subagent(
    path: &Path,
    session_root: Option<&Path>,
    state: Option<&Value>,
    session: &mut Value,
) {
    let agent_id = path
        .parent()
        .and_then(Path::file_name)
        .and_then(|value| value.to_str())
        .unwrap_or("main");
    if agent_id == "main" {
        return;
    }
    let agent_state = state
        .and_then(|state| state.get("agents"))
        .and_then(|agents| agents.get(agent_id));
    let parent_agent = agent_state
        .and_then(|agent| agent.get("parentAgentId"))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty() && *value != "None")
        .unwrap_or("main");
    // The store labels the agent slot, not the task, so the task instruction is
    // the only real label. `swarmItem` only exists on swarm runs.
    let task_title = agent_state
        .and_then(|agent| agent.get("swarmItem"))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
        .or_else(|| {
            super::delegated_transcripts::delegated_task_label(
                super::delegated_transcripts::delegated_task_prompt_text(session)?,
            )
        });
    let (Some(session_root), Some(object)) = (session_root, session.as_object_mut()) else {
        return;
    };
    // A missing parent wire means the delegated merge would drop this
    // session entirely; keep it standalone instead.
    if !session_root
        .join("agents")
        .join(parent_agent)
        .join("wire.jsonl")
        .is_file()
    {
        return;
    }
    let session_id = session_root
        .file_name()
        .and_then(|value| value.to_str())
        .unwrap_or("session");
    let parent_session_id = if parent_agent == "main" {
        session_id.to_string()
    } else {
        format!("{session_id}:{parent_agent}")
    };
    object.insert("parentSessionId".to_string(), json!(parent_session_id));
    object.insert("delegatedSubagent".to_string(), json!(true));
    if let Some(title) = task_title {
        object.insert("subagentTitle".to_string(), json!(title));
    }
    // The agent slot is still worth showing: it is how the conversation refers to
    // the delegated run in its own transcript.
    object.insert("subagentType".to_string(), json!(agent_id));
}

pub(super) fn flush_kimi_code_assistant(
    path: &Path,
    messages: &mut Vec<Value>,
    text: &mut String,
    index: usize,
    created_at: Option<String>,
) {
    if text.is_empty() {
        return;
    }
    if let Some(message) = plain_history_message(
        HistoryAdapter::KimiCode,
        path,
        index,
        0,
        "agent",
        text,
        created_at,
    ) {
        messages.push(message);
    }
    text.clear();
}

pub(super) fn flush_kimi_code_reasoning(
    path: &Path,
    messages: &mut Vec<Value>,
    text: &mut String,
    summaries: &mut Vec<Value>,
    index: usize,
    created_at: Option<String>,
) {
    if text.is_empty() && summaries.is_empty() {
        return;
    }
    let mut value = json!({"text": std::mem::take(text)});
    if !summaries.is_empty() {
        value["summary"] = Value::Array(std::mem::take(summaries));
    }
    messages.push(structured_history_message(
        HistoryAdapter::KimiCode,
        path,
        index,
        0,
        HistoryMessageKind::Reasoning,
        "thinking",
        &value,
        created_at,
    ));
}

pub(super) fn kimi_code_content_group(value: &Value, event: &Value) -> String {
    let turn = find_string(value, &["turnId", "turn_id"])
        .or_else(|| find_string(event, &["turnId", "turn_id"]))
        .unwrap_or_default();
    let step = find_string(event, &["step", "stepId", "step_id"]).unwrap_or_default();
    format!("{turn}\n{step}")
}

pub(super) fn kimi_code_usage_message(path: &Path, index: usize, value: &Value) -> Option<Value> {
    let usage_scope = find_string(value, &["usageScope", "usage_scope"]);
    if usage_scope
        .as_deref()
        .is_some_and(|scope| !scope.eq_ignore_ascii_case("turn"))
    {
        return None;
    }
    let usage = extract_token_usage(value)?;
    let total_tokens = usage
        .get("totalTokens")
        .and_then(token_count_value)
        .unwrap_or(0);
    if total_tokens == 0 {
        return None;
    }
    let model = extract_native_model(value);
    let created_at = find_string(value, &["time", "timestamp", "createdAt"])
        .unwrap_or_else(native_message_timestamp);
    let source_event_type = value
        .get("type")
        .and_then(Value::as_str)
        .unwrap_or("usage.record");
    let mut message = json!({
        "id": native_history_message_id(HistoryAdapter::KimiCode, path, index, 0),
        "role": "metadata",
        "text": "Kimi Code token usage",
        "createdAt": created_at,
        "sourcePath": display_path(path),
        "sourceEventType": source_event_type,
        "usageScope": usage_scope.unwrap_or_else(|| "turn".to_owned()),
        "usage": usage
    });
    if let Some(model) = model {
        message["model"] = json!(model);
    }
    Some(message)
}

/// Native delegated identities append the agent slot to the conversation id.
pub(super) fn kimi_code_conversation_id(native_session_id: &str) -> &str {
    native_session_id
        .split_once(':')
        .map_or(native_session_id, |(session, _)| session)
}

pub(super) fn kimi_code_native_session_id(path: &Path) -> String {
    let agent_id = path
        .parent()
        .and_then(Path::file_name)
        .and_then(|value| value.to_str())
        .unwrap_or("main");
    let session_id = path
        .ancestors()
        .nth(3)
        .and_then(Path::file_name)
        .and_then(|value| value.to_str())
        .unwrap_or("session");
    if agent_id == "main" {
        session_id.to_string()
    } else {
        format!("{session_id}:{agent_id}")
    }
}
