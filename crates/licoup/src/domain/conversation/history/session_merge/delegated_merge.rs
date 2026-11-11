use std::collections::{BTreeMap, HashMap, VecDeque};

use serde_json::{Value, json};

use super::stable_order::{message_order_key, message_role};

const MAX_SUBAGENT_PREVIEW_CHARS: usize = 180;

pub(super) fn merge_delegated_subagent_sessions(
    sessions: Vec<Value>,
    requested_session: Option<&str>,
) -> Vec<Value> {
    let mut indexed_sessions = sessions.into_iter().map(Some).collect::<Vec<_>>();
    merge_explicit_parent_child_lineages(&mut indexed_sessions, requested_session);
    // Only an explicit, present parent can consume a child. Missing parents and
    // cycles retain their own identities and lineage facts for exact readback.
    indexed_sessions.into_iter().flatten().collect()
}

/// Merge explicit child sessions from leaves toward their parents.
///
/// The child-count frontier is the reverse orientation of a topological sort:
/// each edge is processed once, nested descendants are already materialized
/// when their parent becomes ready, and cyclic components remain accessible under their original identities.
fn merge_explicit_parent_child_lineages(
    indexed_sessions: &mut [Option<Value>],
    requested_session: Option<&str>,
) {
    // One identity can appear more than once: an agent store may split records
    // that carry no session field into their own group. Attaching delegated tasks
    // to whichever copy happened to be last would hide them behind a copy that a
    // later dedupe discards, so the copy holding the most of the conversation wins.
    let mut native_ids = HashMap::<String, usize>::new();
    for (slot, session) in indexed_sessions.iter().enumerate() {
        let Some(session) = session.as_ref() else {
            continue;
        };
        let Some(id) = session.get("nativeSessionId").and_then(Value::as_str) else {
            continue;
        };
        let message_count = session
            .get("messages")
            .and_then(Value::as_array)
            .map(Vec::len)
            .unwrap_or(0);
        match native_ids.get(id).copied() {
            Some(existing)
                if session_message_count(indexed_sessions, existing) >= message_count => {}
            _ => {
                native_ids.insert(id.to_string(), slot);
            }
        }
    }
    let parent_by_child = indexed_sessions
        .iter()
        .enumerate()
        .map(|(child_index, session)| {
            let session = session.as_ref()?;
            if !session_is_delegated_subagent(session)
                || requested_session.is_some_and(|id| {
                    session.get("nativeSessionId").and_then(Value::as_str) == Some(id)
                })
            {
                return None;
            }
            let parent_id = session.get("parentSessionId")?.as_str()?;
            let parent_index = *native_ids.get(parent_id)?;
            (parent_index != child_index).then_some(parent_index)
        })
        .collect::<Vec<_>>();

    let mut remaining_children = vec![0usize; indexed_sessions.len()];
    for parent_index in parent_by_child.iter().flatten() {
        remaining_children[*parent_index] += 1;
    }
    let mut ready = parent_by_child
        .iter()
        .enumerate()
        .filter_map(|(child_index, parent)| {
            (parent.is_some() && remaining_children[child_index] == 0).then_some(child_index)
        })
        .collect::<VecDeque<_>>();

    while let Some(child_index) = ready.pop_front() {
        let Some(parent_index) = parent_by_child[child_index] else {
            continue;
        };
        let Some(child_session) = indexed_sessions[child_index].take() else {
            continue;
        };
        let child_running = child_session.get("running").and_then(Value::as_bool) == Some(true);
        if let Some(card) = subagent_card_from_session(&child_session)
            && let Some(parent_session) = indexed_sessions[parent_index].as_mut()
        {
            insert_subagent_card_into_session(parent_session, card);
            if child_running {
                mark_session_running(parent_session);
            }
        }
        remaining_children[parent_index] = remaining_children[parent_index].saturating_sub(1);
        if remaining_children[parent_index] == 0 {
            if let Some(parent_session) = indexed_sessions[parent_index].as_mut() {
                refresh_session_source_revision(parent_session);
            }
            if parent_by_child[parent_index].is_some() {
                ready.push_back(parent_index);
            }
        }
    }
}

fn mark_session_running(session: &mut Value) {
    if let Some(object) = session.as_object_mut() {
        object.insert("running".to_string(), Value::Bool(true));
    }
}

fn session_message_count(indexed_sessions: &[Option<Value>], slot: usize) -> usize {
    indexed_sessions
        .get(slot)
        .and_then(Option::as_ref)
        .and_then(|session| session.get("messages"))
        .and_then(Value::as_array)
        .map(Vec::len)
        .unwrap_or(0)
}

pub(super) fn insert_subagent_card_into_session(session: &mut Value, card: Value) {
    let Some(object) = session.as_object_mut() else {
        return;
    };
    let previous_total = object
        .get("messageCount")
        .and_then(Value::as_u64)
        .map(|value| value as usize);
    let Some(message_count) = object
        .get_mut("messages")
        .and_then(Value::as_array_mut)
        .map(|messages| {
            let insert_at = match message_order_key(&card) {
                Some(card_order_key) => messages
                    .iter()
                    .position(|message| {
                        message_order_key(message)
                            .map(|order_key| order_key > card_order_key)
                            .unwrap_or(false)
                    })
                    .unwrap_or(messages.len()),
                // The card carries no reliable timestamp (the source history
                // recorded none). Keep it right after the conversation's own
                // user/assistant flow instead of pushing it past trailing
                // runtime events, so it still reads as part of the dialogue.
                None => messages
                    .iter()
                    .rposition(|message| {
                        matches!(
                            message_role(message).as_str(),
                            "user" | "human" | "agent" | "assistant"
                        )
                    })
                    .map(|index| index + 1)
                    .unwrap_or(messages.len()),
            };
            messages.insert(insert_at, card);
            messages.len()
        })
    else {
        return;
    };
    let exact_count = previous_total
        .map(|count| count.saturating_add(1))
        .unwrap_or(message_count)
        .max(message_count);
    object.insert("messageCount".to_string(), json!(exact_count));
}

/// Rebuild once after all direct children have joined. Stable native identity
/// order makes browse and exact reads agree, while stripping an earlier
/// aggregate keeps repeated folding from appending the same revisions again.
fn refresh_session_source_revision(session: &mut Value) {
    let mut revision = session
        .get("sourceRevision")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .split('|')
        .next()
        .unwrap_or_default()
        .to_owned();
    let children = session
        .get("messages")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|card| {
            Some((
                card.get("childSessionId")?.as_str()?,
                card.get("childSourceRevision")?.as_str()?,
            ))
        })
        .collect::<BTreeMap<_, _>>();
    for child_revision in children.values() {
        revision.push('|');
        revision.push_str(child_revision);
    }
    session["sourceRevision"] = json!(revision);
}

pub(super) fn subagent_card_from_session(session: &Value) -> Option<Value> {
    let messages = session.get("messages").and_then(Value::as_array)?;
    let explicit = session_is_explicit_delegated_subagent(session);
    if !explicit {
        return None;
    }
    let prompt = messages
        .iter()
        .find(|message| matches!(message_role(message).as_str(), "user" | "human"));
    let title = session
        .get("subagentTitle")
        .and_then(Value::as_str)
        .or_else(|| prompt.and_then(|message| message.get("subagentTitle").and_then(Value::as_str)))
        .filter(|title| !title.trim().is_empty())
        .map(str::to_string)
        .unwrap_or_else(|| "Subagent task".to_string());
    let child_messages = messages
        .iter()
        .filter(|message| subagent_card_child_message_is_visible(message))
        .collect::<Vec<_>>();
    let tool_call_count = session
        .get("toolCallCount")
        .and_then(Value::as_u64)
        .unwrap_or_else(|| {
            messages
                .iter()
                .filter(|message| subagent_card_child_message_is_tool_step(message))
                .count() as u64
        });
    let child_message_count = session
        .get("messageCount")
        .and_then(Value::as_u64)
        .unwrap_or(messages.len() as u64);
    let nested_depth = child_messages
        .iter()
        .filter_map(|message| message.get("subagentDepth").and_then(Value::as_u64))
        .max()
        .unwrap_or(0);
    let preview = child_messages
        .iter()
        .rev()
        .filter_map(|message| message.get("text").and_then(Value::as_str))
        .find(|text| !text.trim().is_empty())
        .map(subagent_card_preview_text)
        .unwrap_or_else(|| title.clone());
    let created_at = session
        .get("createdAt")
        .and_then(Value::as_str)
        .or_else(|| prompt.and_then(|message| message.get("createdAt").and_then(Value::as_str)))
        .or_else(|| {
            child_messages
                .first()
                .and_then(|message| message.get("createdAt").and_then(Value::as_str))
        })
        .unwrap_or_default()
        .to_string();
    let source_path = session
        .get("sourcePath")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    let card_id = session
        .get("id")
        .and_then(Value::as_str)
        .map(|id| format!("{}::subagent-card", id))
        .unwrap_or_else(|| "subagent-card".to_string());
    Some(json!({
        "id": card_id,
        "role": "subagent",
        "text": preview,
        "createdAt": created_at,
        "sourcePath": source_path,
        "cardType": "subagent",
        "cardTitle": title,
        // The declared agent type is the only label the store owns; counts stay
        // numeric so the client renders them in the user's language.
        "cardSubtitle": session
            .get("subagentType")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .unwrap_or_default(),
        "collapsed": true,
        // Nesting level of this card inside the conversation. A card whose own
        // children contain cards sits one level above the deepest of them, so the
        // client can indent a delegated task that delegated further.
        "subagentDepth": nested_depth.saturating_add(1),
        "subagentChildCount": child_message_count,
        "subagentToolCallCount": tool_call_count,
        "childSessionId": session.get("nativeSessionId"),
        "childMessageCount": child_message_count,
        "childSourceRevision": session.get("sourceRevision"),
        // Expanding the card reads this exact native identity through the same
        // message cursor as any conversation. Parent pages never serialize a
        // recursively growing tree of child transcripts.
        "messages": []
    }))
}

/// Whether one message of a delegated task belongs in its card.
///
/// Only the conversation's own framing is dropped. Tool steps stay: an explore
/// or verification task is often nothing but tool work, and hiding it leaves an
/// empty card that says nothing about what the task did.
fn subagent_card_child_message_is_visible(message: &Value) -> bool {
    let role = message_role(message);
    if matches!(role.as_str(), "system" | "developer" | "metadata") {
        return false;
    }
    if message
        .get("text")
        .and_then(Value::as_str)
        .is_some_and(|text| !text.trim().is_empty())
    {
        return true;
    }
    // A structured step carries its meaning in its event fields, not in text.
    message_is_structured_step(message)
}

pub(crate) fn subagent_card_child_message_is_tool_step(message: &Value) -> bool {
    matches!(
        message_role(message).as_str(),
        "tool" | "function" | "tool_use" | "tool_result" | "tool_call"
    ) || message
        .get("eventKind")
        .and_then(Value::as_str)
        .is_some_and(|kind| kind.contains("tool"))
}

fn message_is_structured_step(message: &Value) -> bool {
    ["eventKind", "cardType", "toolName", "kind"]
        .iter()
        .any(|field| {
            message
                .get(*field)
                .and_then(Value::as_str)
                .is_some_and(|value| !value.trim().is_empty())
        })
}

pub(super) fn subagent_card_preview_text(text: &str) -> String {
    let preview = text
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .unwrap_or_default();
    let mut chars = preview.chars();
    let mut out = chars
        .by_ref()
        .take(MAX_SUBAGENT_PREVIEW_CHARS)
        .collect::<String>();
    if chars.next().is_some() {
        out.push_str("...");
    }
    out
}

pub(super) fn session_is_delegated_subagent(session: &Value) -> bool {
    session_is_explicit_delegated_subagent(session)
}

fn session_is_explicit_delegated_subagent(session: &Value) -> bool {
    session
        .get("delegatedSubagent")
        .and_then(Value::as_bool)
        .unwrap_or(false)
        && session
            .get("parentSessionId")
            .and_then(Value::as_str)
            .is_some_and(|id| !id.trim().is_empty())
}
