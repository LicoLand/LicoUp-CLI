//! Bridge from LicoUp's history read path to ACOP codecs.
//!
//! For agents with a published a4aa store card, Agent Context Operators
//! codecs own the persisted-session format: file acceptance, container
//! enumeration, native decoding and record projection. This module maps
//! `HistoryAdapter` to the codec and folds the canonical conversation
//! back into the session shape the product layers (delegated collapse,
//! semantic assembly, privacy projection, pagination) already consume.
//! Adapters without a card — Lico Agent, WorkBuddy, CodeBuddy, Trae and
//! VS Code — keep their existing parsers.

use super::{
    HistoryMessageKind, HistoryScanConfig, backfill_transcript_message_times, extract_role,
    extract_text, find_string, history_message_kind_from_semantic, native_history_message_id,
    normalize_history_message_semantic, plain_history_message, session_from_messages_with_title,
    structured_history_message,
};
use crate::domain::conversation::source_catalog::HistoryAdapter;
use acop::codec::{Codec, Decoded, SessionRef};
use acop::model::Body;
use serde_json::{Value, json};
use std::fs;
use std::path::Path;

/// The codec that owns this adapter's persisted-session store, when one
/// exists. Codec ids come from the a4aa cards, which differ from the
/// adapter ids for `claude-code`/`deepseek-harness`/`kilo-code`/
/// `kimi-code`.
pub(crate) fn codec_for(adapter: HistoryAdapter) -> Option<&'static dyn Codec> {
    Some(match adapter {
        HistoryAdapter::Antigravity => acop_antigravity::codec(),
        HistoryAdapter::ClaudeCode => acop_claude::codec(),
        HistoryAdapter::Codex => acop_codex::codec(),
        HistoryAdapter::Copilot => acop_copilot::codec(),
        HistoryAdapter::Cursor => acop_cursor::codec(),
        HistoryAdapter::DeepSeekHarness => acop_dsh::codec(),
        HistoryAdapter::Hermes => acop_hermes::codec(),
        HistoryAdapter::KiloCode => acop_kilo_code::codec(),
        HistoryAdapter::KimiCode => acop_kimi_code::codec(),
        HistoryAdapter::OpenClaw => acop_openclaw::codec(),
        HistoryAdapter::OpenCode => acop_opencode::codec(),
        HistoryAdapter::Pi => acop_pi::codec(),
        _ => return None,
    })
}

/// One history-file read through the codec: the file expands to session
/// units (a container file can hold many), each decodes to a canonical
/// conversation and folds into the session projection the catalog and
/// query layers consume.
pub(crate) fn sessions_from_store(
    codec: &dyn Codec,
    adapter: HistoryAdapter,
    path: &Path,
    source_kind: &str,
    metadata: &fs::Metadata,
    scan_config: &HistoryScanConfig,
) -> Vec<Value> {
    let Ok(refs) = codec.sessions_in_file(path) else {
        return Vec::new();
    };
    let scope = scan_config.shared_store_session_scope();
    // `retain_requested_session_lineage`: the page scope names roots;
    // descendants of a scoped session join the fold through their
    // declared parent edge, transitively. Only container-addressed refs
    // (`key = Some`) filter here — their ref id is the declared session
    // id. A file-addressed ref's id is only the filename locator guess;
    // the declared id is known after decode, so the file still opens and
    // the session-level `matches_session` filter decides.
    let wanted = scope.as_ref().map(|scope| {
        let mut wanted: std::collections::HashSet<String> = refs
            .iter()
            .filter(|session| session.key.is_some() && scope.contains(&session.id))
            .map(|session| session.id.clone())
            .collect();
        loop {
            let before = wanted.len();
            for session in &refs {
                if session.key.is_none() || wanted.contains(&session.id) {
                    continue;
                }
                if session
                    .parent
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
        wanted
    });
    refs.into_iter()
        .filter(|session| {
            session.key.is_none()
                || wanted
                    .as_ref()
                    .is_none_or(|wanted| wanted.contains(&session.id))
        })
        .filter_map(|session| {
            codec
                .decode_session(&session)
                .ok()
                .map(|decoded| (session, decoded))
        })
        .map(|(session, decoded)| {
            project_session(adapter, path, source_kind, metadata, &session, decoded)
        })
        .collect()
}

fn project_session(
    adapter: HistoryAdapter,
    path: &Path,
    source_kind: &str,
    metadata: &fs::Metadata,
    session: &SessionRef,
    decoded: Decoded,
) -> Value {
    let is_rollout = adapter == HistoryAdapter::Codex
        && decoded.conversation.records.iter().any(|record| {
            matches!(&record.body, Body::Meta { subtype, .. } if subtype == "session_meta")
        });
    let mut messages = Vec::<Value>::new();
    let mut last_message = None::<usize>;
    let mut session_usage = None::<Value>;
    let mut turn_context_cwd = None::<String>;
    // The current rollout turn (`turn_context` opens one, lifecycle events
    // close it): agent records raised inside a turn carry `sourceTurnId`
    // so execution provenance can bind them to provider turn keys.
    let mut native_turn_id = None::<String>;
    let mut block_index = 0usize;
    let mut previous_line = usize::MAX;
    for (ordinal, record) in decoded.conversation.records.iter().enumerate() {
        let line = decoded
            .record_lines
            .get(ordinal)
            .copied()
            .unwrap_or(ordinal);
        block_index = if line == previous_line {
            block_index + 1
        } else {
            0
        };
        previous_line = line;
        match &record.body {
            Body::User { text, model, .. } => {
                if let Some(mut message) = plain_history_message(
                    adapter,
                    path,
                    line,
                    block_index,
                    "user",
                    text,
                    record.ts.clone(),
                ) {
                    if let Some(model) = model {
                        message["model"] = json!(model);
                    }
                    last_message = Some(messages.len());
                    messages.push(message);
                }
            }
            Body::Assistant { text, model } => {
                if let Some(mut message) = plain_history_message(
                    adapter,
                    path,
                    line,
                    block_index,
                    "agent",
                    text,
                    record.ts.clone(),
                ) {
                    if is_rollout {
                        if let Some(turn) = &native_turn_id {
                            message["sourceTurnId"] = json!(turn);
                        }
                        // `response_item`/`message` payloads name their own
                        // record id — the canonical record keeps it.
                        if let Some(id) = record.id.as_deref().filter(|id| !id.is_empty()) {
                            message["sourceMessageId"] = json!(id);
                        }
                    }
                    if let Some(model) = model {
                        message["model"] = json!(model);
                    }
                    last_message = Some(messages.len());
                    messages.push(message);
                }
            }
            Body::Reasoning { text } => {
                last_message = Some(messages.len());
                messages.push(structured_history_message(
                    adapter,
                    path,
                    line,
                    block_index,
                    HistoryMessageKind::Reasoning,
                    "reasoning",
                    &json!({ "thinking": text }),
                    record.ts.clone(),
                ));
            }
            Body::ToolCall {
                call_id,
                name,
                args,
            } => {
                last_message = Some(messages.len());
                messages.push(structured_history_message(
                    adapter,
                    path,
                    line,
                    block_index,
                    HistoryMessageKind::ToolCall,
                    name,
                    &json!({ "id": call_id, "name": name, "arguments": args }),
                    record.ts.clone(),
                ));
            }
            Body::ToolResult {
                call_id,
                name,
                content,
                is_error,
            } => {
                last_message = Some(messages.len());
                messages.push(structured_history_message(
                    adapter,
                    path,
                    line,
                    block_index,
                    HistoryMessageKind::ToolResult,
                    "toolResult",
                    &json!({
                        "id": call_id,
                        "name": name,
                        "content": content,
                        "isError": is_error,
                    }),
                    record.ts.clone(),
                ));
            }
            Body::System { subtype, text } => {
                // The subtype is the record's raw role — the same semantic
                // dispatch the source readers ran: text-carriers through the
                // plain pipeline (per-adapter visibility decides), every
                // declared event kind through the structured projector.
                let kind = history_message_kind_from_semantic(subtype);
                if kind == HistoryMessageKind::Text {
                    if let Some(message) = plain_history_message(
                        adapter,
                        path,
                        line,
                        block_index,
                        subtype,
                        text,
                        record.ts.clone(),
                    ) {
                        last_message = Some(messages.len());
                        messages.push(message);
                    }
                } else {
                    last_message = Some(messages.len());
                    messages.push(structured_history_message(
                        adapter,
                        path,
                        line,
                        block_index,
                        kind,
                        subtype,
                        &json!({ "text": text }),
                        record.ts.clone(),
                    ));
                }
            }
            Body::Meta { subtype, data } => {
                // Rollout turn tracking: `turn_context` and `task_started`
                // open the current turn, the closing lifecycle events end
                // it — agent records inside carry `sourceTurnId`.
                if is_rollout {
                    match subtype.as_str() {
                        "session_meta" => native_turn_id = None,
                        "turn_context" => {
                            native_turn_id = find_string(data, &["turn_id", "turnId"])
                                .filter(|id| !id.is_empty());
                        }
                        _ if subtype.starts_with("event:") => {
                            let lifecycle = &subtype["event:".len()..];
                            if lifecycle == "task_started" {
                                native_turn_id = find_string(data, &["turn_id", "turnId"])
                                    .filter(|id| !id.is_empty());
                            } else if matches!(
                                lifecycle,
                                "task_complete"
                                    | "task_cancelled"
                                    | "turn_aborted"
                                    | "turn_cancelled"
                                    | "turn_completed"
                            ) {
                                native_turn_id = None;
                            }
                        }
                        _ => {}
                    }
                }
                if matches!(
                    subtype.as_str(),
                    "subagent-lineage" | "session-agent" | "session_meta"
                ) {
                    // Session-fact records stamp fields on the session
                    // itself; they are never transcript messages.
                    let _ = data;
                    continue;
                }
                if subtype == "row" {
                    // A store row with no decodable JSON is the source's
                    // "record" card fallback: raw `name: value` lines under
                    // the `record` role, carrying the table/key the
                    // locator named (`ItemTable/chat.first`).
                    let text = data
                        .as_object()
                        .map(|fields| {
                            fields
                                .iter()
                                .filter_map(|(name, value)| {
                                    value.as_str().map(|value| format!("{name}: {value}"))
                                })
                                .collect::<Vec<_>>()
                                .join("\n")
                        })
                        .unwrap_or_default();
                    let mut message = json!({
                        "id": native_history_message_id(adapter, path, line, block_index),
                        "role": "record",
                        "text": text,
                        "createdAt": record.ts.clone().unwrap_or_default(),
                        "sourcePath": super::display_path(path),
                        "sourceFields": data,
                    });
                    if let Some((table, row_key)) =
                        session.key.as_deref().and_then(|key| key.split_once('/'))
                    {
                        message["sourceTable"] = json!(table);
                        message["sourceKey"] = json!(row_key);
                    }
                    last_message = Some(messages.len());
                    messages.push(message);
                    continue;
                }
                if subtype == "usage" {
                    // Usage rides the record that produced it: folded onto
                    // the last visible message like the source parsers did
                    // (`usage` + `usageScope` on the same message object),
                    // or onto the session itself when it leads the store.
                    let usage = extract_usage_value(data);
                    match (usage, last_message) {
                        (Some(usage), Some(index)) => {
                            messages[index]["usage"] = usage;
                            messages[index]["usageScope"] = json!("request-response");
                        }
                        (usage @ Some(_), None) => session_usage = usage,
                        _ => {}
                    }
                    continue;
                }
                // Codex rollout bookkeeping subtypes only ever come out of
                // the native rollout decoder — dispatch them through the
                // same projection contract the rollout reader had instead
                // of surfacing every one as a metadata card.
                if codex_rollout_meta(subtype, data, &mut turn_context_cwd) {
                    continue;
                }
                if let Some(sub) = subtype.strip_prefix("event:") {
                    // `event_msg` lines that mirror the visible records or
                    // bare lifecycle noise carry no card; everything else
                    // goes through the structured projector under the
                    // payload's own item semantic.
                    let normalized = normalize_history_message_semantic(sub);
                    if normalized.ends_with("-delta")
                        || matches!(
                            normalized.as_str(),
                            "agent-message" | "user-message" | "task-started" | "task-complete"
                        )
                    {
                        continue;
                    }
                    let kind = if matches!(
                        normalized.as_str(),
                        "token-count" | "usage" | "turn-metadata"
                    ) {
                        HistoryMessageKind::Metadata
                    } else if matches!(normalized.as_str(), "turn-aborted" | "stream-error") {
                        HistoryMessageKind::Error
                    } else {
                        history_message_kind_from_semantic(sub)
                    };
                    if matches!(kind, HistoryMessageKind::Text | HistoryMessageKind::Event)
                        && normalized != "warning"
                    {
                        continue;
                    }
                    last_message = Some(messages.len());
                    messages.push(structured_history_message(
                        adapter,
                        path,
                        line,
                        block_index,
                        kind,
                        sub,
                        data,
                        record.ts.clone(),
                    ));
                    continue;
                }
                if let Some(item) = subtype.strip_prefix("response_item:") {
                    // A `response_item` payload the codec had no dedicated
                    // body for still follows `codex_response_item_message`:
                    // known structured kinds project, text-like payloads
                    // need a text carrier or they carry nothing.
                    let kind = history_message_kind_from_semantic(item);
                    if kind != HistoryMessageKind::Text {
                        last_message = Some(messages.len());
                        messages.push(structured_history_message(
                            adapter,
                            path,
                            line,
                            block_index,
                            kind,
                            item,
                            data,
                            record.ts.clone(),
                        ));
                    } else if let Some(text) = extract_text(data)
                        .or_else(|| data.get("text").and_then(Value::as_str).map(String::from))
                    {
                        let role = extract_role(data);
                        if let Some(message) = plain_history_message(
                            adapter,
                            path,
                            line,
                            block_index,
                            &role,
                            &text,
                            record.ts.clone(),
                        ) {
                            last_message = Some(messages.len());
                            messages.push(message);
                        }
                    }
                    continue;
                }
                last_message = Some(messages.len());
                messages.push(structured_history_message(
                    adapter,
                    path,
                    line,
                    block_index,
                    HistoryMessageKind::Metadata,
                    subtype,
                    data,
                    record.ts.clone(),
                ));
            }
        }
    }
    // Rollout-native conversations (a `session_meta` head is present)
    // source every visible record from `response_item` lines; the
    // legacy parser stamped that envelope type on each message and the
    // semantic evidence layer reads it back.
    if is_rollout {
        for message in &mut messages {
            if let Some(object) = message.as_object_mut() {
                object
                    .entry("sourceEventType".to_string())
                    .or_insert_with(|| json!("response_item"));
            }
        }
    }
    backfill_transcript_message_times(&mut messages, path);
    let mut value = session_from_messages_with_title(
        adapter,
        path,
        metadata,
        source_kind,
        decoded.conversation.session_id.clone(),
        messages,
        decoded.conversation.title.clone(),
    );
    if let Some(object) = value.as_object_mut() {
        let cwd = decoded
            .conversation
            .cwd
            .as_deref()
            .or(turn_context_cwd.as_deref())
            .and_then(super::project_workspace::bounded_project_workspace);
        if let Some(cwd) = cwd {
            object
                .entry("workingDirectory".to_string())
                .or_insert_with(|| Value::String(cwd));
        }
        // Canonical timestamps come from the store itself — a container
        // session's `createdAt`/`updatedAt` outrank the file's stat times.
        if let Some(created_at) = &decoded.conversation.created_at {
            object.insert("createdAt".to_string(), json!(created_at));
        }
        if let Some(key) = &session.key {
            object.insert("sourceSessionKey".to_string(), json!(key));
        }
        if let Some(updated_at) = &decoded.conversation.updated_at {
            object.insert("updatedAt".to_string(), json!(updated_at));
        }
        if let Some(usage) = session_usage {
            object.insert("usage".to_string(), usage);
        }
    }
    stamp_session_lineage(adapter, path, &decoded, &mut value);
    value
}

/// Codex rollout bookkeeping records — only ever produced by the native
/// rollout decoder — that the source reader consumed as group facts
/// (`turn_context` feeds the session cwd) or dropped outright
/// (`world_state`, `token_usage_record`, unknown envelope types). Returns
/// true when the record must not surface as a transcript message.
fn codex_rollout_meta(subtype: &str, data: &Value, turn_context_cwd: &mut Option<String>) -> bool {
    match subtype {
        "turn_context" => {
            if turn_context_cwd.is_none() {
                *turn_context_cwd = find_string(data, &["cwd", "workingDirectory", "projectPath"])
                    .filter(|value| !value.trim().is_empty());
            }
            true
        }
        "world_state" | "token_usage_record" => true,
        subtype => subtype.starts_with("codex:"),
    }
}

/// Session-level lineage the store declares: delegated transcripts are
/// identified by their place in the tree (Cursor CLI transcripts and Claude
/// Code `subagents/` files), and Codex rollouts carry `thread_spawn` inside
/// the `session_meta` payload, which travels verbatim as `meta{session_meta}`
/// evidence on the canonical conversation.
fn stamp_session_lineage(
    adapter: HistoryAdapter,
    path: &Path,
    decoded: &Decoded,
    value: &mut Value,
) {
    // Store-declared session facts travel as leading `meta` records on
    // the canonical conversation (`subagent-lineage`, `session-agent`):
    // they lift back onto the session fields the merge and browse
    // layers read — `parentSessionId` folds children, `model` and
    // `projectPath` label the row.
    for record in &decoded.conversation.records {
        let Body::Meta { subtype, data } = &record.body else {
            continue;
        };
        if !matches!(subtype.as_str(), "subagent-lineage" | "session-agent") {
            continue;
        }
        let Some(object) = value.as_object_mut() else {
            break;
        };
        if let Some(parent) = data.get("parentSessionId").and_then(Value::as_str) {
            object.insert("parentSessionId".to_string(), json!(parent));
            if subtype == "subagent-lineage" {
                object.insert("delegatedSubagent".to_string(), json!(true));
            }
        }
        if let Some(kind) = data.get("subagentTypeName").and_then(Value::as_str) {
            object.insert("subagentType".to_string(), json!(kind));
        }
        if let Some(title) = data
            .get("subagentTitle")
            .or_else(|| data.get("title"))
            .and_then(Value::as_str)
        {
            object.insert("subagentTitle".to_string(), json!(title));
        }
        if let Some(model) = data.get("model").and_then(Value::as_str) {
            object
                .entry("model".to_string())
                .or_insert_with(|| json!(model));
        }
        if let Some(agent_name) = data.get("agent").and_then(Value::as_str) {
            object
                .entry("nativeAgent".to_string())
                .or_insert_with(|| json!(agent_name));
        }
        // `directory` (bounded) drives `workingDirectory`; the recorded
        // `path` is a separate display fact under its own key.
        if let Some(project) = data
            .get("projectPath")
            .and_then(Value::as_str)
            .filter(|value| !value.trim().is_empty())
        {
            object
                .entry("projectPath".to_string())
                .or_insert_with(|| json!(project));
        }
        break;
    }
    if matches!(adapter, HistoryAdapter::Cursor | HistoryAdapter::ClaudeCode) {
        if adapter == HistoryAdapter::Cursor
            && let (Some(workspace), Some(object)) = (
                super::delegated_transcripts::cursor_transcript_project_workspace(path),
                value.as_object_mut(),
            )
        {
            object
                .entry("workingDirectory".to_string())
                .or_insert_with(|| Value::String(workspace));
        }
        super::delegated_transcripts::apply_transcript_identity(value, path);
        super::delegated_transcripts::mark_delegated_transcript_session(value, path);
    }
    if adapter == HistoryAdapter::Codex {
        let session_meta =
            decoded
                .conversation
                .records
                .iter()
                .find_map(|record| match &record.body {
                    Body::Meta { subtype, data } if subtype == "session_meta" => Some(data),
                    _ => None,
                });
        let Some(payload) = session_meta else {
            return;
        };
        let (parent, is_subagent, title) =
            super::codex::codex_rollout_lineage(payload, &decoded.conversation.session_id);
        let Some(object) = value.as_object_mut() else {
            return;
        };
        if let Some(parent) = parent {
            object.insert("parentSessionId".to_string(), json!(parent));
        }
        if is_subagent {
            object.insert("delegatedSubagent".to_string(), json!(true));
        }
        if let Some(title) = title {
            object.insert("subagentTitle".to_string(), json!(title));
        }
    }
}

/// The token counters a `meta{usage}` record carries: already-normalized
/// store columns (`promptTokens`…) pass through verbatim, raw envelopes
/// (`{"tokens": …}`, `{"usage": …}`) go through the usage normalizer.
fn extract_usage_value(data: &Value) -> Option<Value> {
    let mut usage = crate::domain::conversation::usage::extract_token_usage(data)
        .or_else(|| data.get("promptTokens").map(|_| data.clone()))?;
    if let Some(model) = agent_statistics::variant::model_label(data) {
        usage["model"] = json!(model);
    }
    Some(usage)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn usage_projection_keeps_the_canonical_model_without_inventing_one() {
        let counters =
            json!({"promptTokens":10,"cachedInputTokens":0,"completionTokens":2,"totalTokens":12});
        let projected =
            extract_usage_value(&json!({"model":"recorded-model","usage":counters})).unwrap();
        assert_eq!(projected["model"], "recorded-model");
        assert_eq!(projected["totalTokens"], 12);
        assert!(
            extract_usage_value(&json!({"usage":counters}))
                .unwrap()
                .get("model")
                .is_none()
        );
    }
}
