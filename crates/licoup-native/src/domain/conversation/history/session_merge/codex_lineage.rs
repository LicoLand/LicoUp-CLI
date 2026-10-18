use std::collections::{BTreeMap, BTreeSet};

use serde_json::{Value, json};
use sha2::{Digest, Sha256};

use super::dedupe_paging::{
    history_session_adapter_id, history_session_message_count, history_session_native_id,
};
use super::stable_order::{message_order_key, message_role, session_updated_order_key};

/// Collapse Codex fork/continuation rollouts that share a `forked_from_id` /
/// `parentSessionId` chain into one list entry per lineage root.
pub(super) fn merge_codex_rollout_lineage_sessions(sessions: Vec<Value>) -> Vec<Value> {
    let mut codex = Vec::<Value>::new();
    let mut other = Vec::<Value>::new();
    for session in sessions {
        if history_session_adapter_id(&session) == "codex" {
            codex.push(session);
        } else {
            other.push(session);
        }
    }
    if codex.len() < 2 {
        other.extend(codex);
        return other;
    }

    let parents = codex_rollout_lineage_parents(&codex);
    if parents.is_empty() {
        other.extend(codex);
        return other;
    }

    let mut groups = BTreeMap::<String, Vec<Value>>::new();
    for session in codex {
        let native_id = history_session_native_id(&session);
        let root = codex_rollout_lineage_root(&native_id, &parents);
        groups.entry(root).or_default().push(session);
    }

    for (root, members) in groups {
        if members.len() == 1 {
            let mut session = members.into_iter().next().expect("one member");
            annotate_codex_lineage_root(&mut session, &root);
            other.push(session);
        } else {
            other.push(collapse_codex_rollout_lineage_group(root, members));
        }
    }
    other
}

pub(super) fn codex_rollout_lineage_parents(sessions: &[Value]) -> BTreeMap<String, String> {
    let mut candidates = BTreeMap::<String, BTreeSet<String>>::new();
    for session in sessions {
        let native_id = history_session_native_id(session);
        if native_id.is_empty() {
            continue;
        }
        // A delegated task is a separate conversation the parent spawned, not a
        // fork continuation of it. Collapsing it as a fork would splice its
        // messages into the parent's own transcript and drop the rest.
        if session
            .get("delegatedSubagent")
            .and_then(Value::as_bool)
            .unwrap_or(false)
        {
            continue;
        }
        let Some(parent_id) = session
            .get("parentSessionId")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_string)
        else {
            continue;
        };
        if parent_id != native_id {
            candidates.entry(native_id).or_default().insert(parent_id);
        }
    }
    candidates
        .into_iter()
        .filter_map(|(session_id, parents)| {
            if parents.len() != 1 {
                return None;
            }
            parents
                .into_iter()
                .next()
                .map(|parent_id| (session_id, parent_id))
        })
        .collect()
}

pub(super) fn codex_rollout_lineage_root(
    session_id: &str,
    parents: &BTreeMap<String, String>,
) -> String {
    if session_id.is_empty() {
        return String::new();
    }
    let mut current = session_id.to_string();
    let mut visited = BTreeSet::<String>::new();
    loop {
        if !visited.insert(current.clone()) {
            return visited
                .into_iter()
                .min()
                .unwrap_or_else(|| session_id.to_string());
        }
        let Some(parent) = parents.get(&current) else {
            return current;
        };
        current.clone_from(parent);
    }
}

pub(super) fn collapse_codex_rollout_lineage_group(root: String, mut members: Vec<Value>) -> Value {
    members.sort_by(|left, right| {
        session_updated_order_key(left)
            .cmp(&session_updated_order_key(right))
            .then_with(|| history_session_native_id(left).cmp(&history_session_native_id(right)))
    });
    let tip_index = members
        .iter()
        .enumerate()
        .max_by_key(|(index, session)| {
            (
                session_updated_order_key(session),
                history_session_message_count(session),
                *index,
            )
        })
        .map(|(index, _)| index)
        .unwrap_or(0);
    let tip = members[tip_index].clone();
    let messages = merge_codex_lineage_messages(&members);
    let created_at = members
        .iter()
        .filter_map(|session| session.get("createdAt").and_then(Value::as_str))
        .min()
        .unwrap_or_default()
        .to_string();
    let updated_at = members
        .iter()
        .filter_map(|session| session.get("updatedAt").and_then(Value::as_str))
        .max()
        .unwrap_or_default()
        .to_string();
    let member_ids = members
        .iter()
        .map(history_session_native_id)
        .filter(|value| !value.is_empty())
        .collect::<BTreeSet<_>>();
    let running = members
        .iter()
        .any(|session| session.get("running").and_then(Value::as_bool) == Some(true));

    let mut collapsed = tip;
    if let Some(object) = collapsed.as_object_mut() {
        object.insert("createdAt".to_string(), json!(created_at));
        if !updated_at.is_empty() {
            object.insert("updatedAt".to_string(), json!(updated_at));
        }
        object.insert("messages".to_string(), json!(messages.clone()));
        object.insert("messageCount".to_string(), json!(messages.len()));
        object.insert("lineageRootId".to_string(), json!(root));
        object.insert(
            "lineageSessionIds".to_string(),
            json!(member_ids.into_iter().collect::<Vec<_>>()),
        );
        if running {
            object.insert("running".to_string(), json!(true));
        } else {
            object.remove("running");
        }
        object.remove("parentSessionId");
        object.remove("delegatedSubagent");
        object.remove("subagentTitle");
    }
    collapsed
}

pub(super) fn merge_codex_lineage_messages(members: &[Value]) -> Vec<Value> {
    let tip = members.iter().max_by_key(|session| {
        (
            session_updated_order_key(session),
            history_session_message_count(session),
        )
    });
    let tip_count = tip.map(history_session_message_count).unwrap_or(0);
    let max_count = members
        .iter()
        .map(history_session_message_count)
        .max()
        .unwrap_or(0);
    if tip_count >= max_count {
        if let Some(messages) = tip
            .and_then(|session| session.get("messages"))
            .and_then(Value::as_array)
        {
            return messages.clone();
        }
    }

    let mut ordered_members = members
        .iter()
        .enumerate()
        .map(|(index, session)| (session_updated_order_key(session), index, session))
        .collect::<Vec<_>>();
    ordered_members.sort_by(|left, right| left.0.cmp(&right.0).then_with(|| left.1.cmp(&right.1)));

    let mut seen = BTreeSet::<String>::new();
    let mut messages = Vec::<Value>::new();
    for (_, _, session) in ordered_members {
        let Some(items) = session.get("messages").and_then(Value::as_array) else {
            continue;
        };
        for message in items {
            let fingerprint = codex_lineage_message_fingerprint(message);
            if seen.insert(fingerprint) {
                messages.push(message.clone());
            }
        }
    }
    messages.sort_by(|left, right| {
        message_order_key(left)
            .unwrap_or(0)
            .cmp(&message_order_key(right).unwrap_or(0))
            .then_with(|| {
                codex_lineage_message_fingerprint(left)
                    .cmp(&codex_lineage_message_fingerprint(right))
            })
    });
    messages
}

pub(super) fn codex_lineage_message_fingerprint(message: &Value) -> String {
    let role = message_role(message);
    let text = message
        .get("text")
        .and_then(Value::as_str)
        .unwrap_or_default();
    if matches!(
        role.as_str(),
        "user" | "human" | "assistant" | "agent" | "model"
    ) && !text.trim().is_empty()
    {
        let mut hasher = Sha256::new();
        hasher.update(role.as_bytes());
        hasher.update(b"\n");
        hasher.update(text.as_bytes());
        return format!("thread:{:x}", hasher.finalize());
    }
    let id = message
        .get("id")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or_default();
    if !id.is_empty() {
        return format!("id:{id}");
    }
    let created = message
        .get("createdAt")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let card_title = message
        .get("cardTitle")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let mut hasher = Sha256::new();
    hasher.update(role.as_bytes());
    hasher.update(b"\n");
    hasher.update(created.as_bytes());
    hasher.update(b"\n");
    hasher.update(card_title.as_bytes());
    hasher.update(b"\n");
    hasher.update(text.as_bytes());
    format!("body:{:x}", hasher.finalize())
}

fn annotate_codex_lineage_root(session: &mut Value, root: &str) {
    if root.is_empty() {
        return;
    }
    if let Some(object) = session.as_object_mut() {
        object.insert("lineageRootId".to_string(), json!(root));
    }
}
