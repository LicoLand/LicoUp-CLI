use super::{
    agent_id, append_activity, bool_param, collection_items_mut, get_approved_pairing,
    is_explicitly_revealed, is_hidden, pairing_required, skill_id, timestamp, upsert_policy_item,
};
use crate::platform::client_state::ClientStateStore;
use anyhow::Result;
use serde_json::{Value, json};

/// Read-only catalog queries stay strict: they never create pairing state as
/// a side effect, unlike the mutating management operations.
fn has_approved_pairing(store: &ClientStateStore, agent_id: &str) -> Result<bool> {
    Ok(get_approved_pairing(store, agent_id)?.is_some())
}

pub(super) fn list(store: &ClientStateStore, params: &Value) -> Result<Value> {
    let agent_id = agent_id(params)?;
    if !has_approved_pairing(store, &agent_id)? {
        return Ok(pairing_required(&agent_id));
    }
    let mut skills = Vec::<Value>::new();
    for discovered in super::discovery::discover(&agent_id, params).unwrap_or_default() {
        let discovered_id = discovered
            .get("skillId")
            .and_then(Value::as_str)
            .unwrap_or_default();
        if is_hidden(store, &agent_id, discovered_id)?
            || !is_visible_by_policy(store, &agent_id, discovered_id)?
            || skills.iter().any(|skill| {
                skill.get("skillId").and_then(Value::as_str) == Some(discovered_id)
                    && skill.get("agentId").and_then(Value::as_str) == Some(agent_id.as_str())
            })
        {
            continue;
        }
        skills.push(discovered);
    }
    skills.sort_by(|left, right| {
        left.get("skillId")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .cmp(
                right
                    .get("skillId")
                    .and_then(Value::as_str)
                    .unwrap_or_default(),
            )
    });
    Ok(json!({
        "ok": true,
        "agentId": agent_id,
        "source": "local-agent-skill-roots",
        "skills": skills
    }))
}

pub(super) fn get(store: &ClientStateStore, params: &Value) -> Result<Value> {
    let agent_id = agent_id(params)?;
    if !has_approved_pairing(store, &agent_id)? {
        return Ok(pairing_required(&agent_id));
    }
    let skill_id = skill_id(params)?;
    let skill = super::discovery::discover(&agent_id, params)
        .ok()
        .and_then(|skills| {
            skills.into_iter().find(|skill| {
                skill.get("skillId").and_then(Value::as_str) == Some(skill_id.as_str())
            })
        });
    let Some(skill) = skill else {
        return Ok(json!({
            "ok": false,
            "error": "not_found",
            "agentId": agent_id,
            "skillId": skill_id,
            "source": "local-agent-skill-roots"
        }));
    };
    if is_hidden(store, &agent_id, &skill_id)? {
        return Ok(json!({
            "ok": false,
            "error": "hidden",
            "agentId": agent_id,
            "skillId": skill_id
        }));
    }
    if !is_visible_by_policy(store, &agent_id, &skill_id)? {
        return Ok(json!({
            "ok": false,
            "error": "visibility_denied",
            "agentId": agent_id,
            "skillId": skill_id,
            "message": "Skill visibility denied by pairing defaultVisibilityPolicy"
        }));
    }
    Ok(json!({
        "ok": true,
        "agentId": agent_id,
        "skill": skill,
        "source": "local-agent-skill-roots"
    }))
}

pub(super) fn visibility(store: &ClientStateStore, params: &Value) -> Result<Value> {
    let agent_id = agent_id(params)?;
    let skill_id = skill_id(params)?;
    let hidden = bool_param(params, "hidden").unwrap_or_else(|| {
        params
            .get("visibility")
            .and_then(Value::as_str)
            .map(|value| value == "hidden" || value == "hide")
            .unwrap_or(false)
    });
    let mut document = store.read_collection("skills")?;
    let items = collection_items_mut(&mut document)?;
    upsert_policy_item(
        items,
        &agent_id,
        &skill_id,
        json!({
            "agentId": agent_id,
            "skillId": skill_id,
            "hidden": hidden,
            "visibility": if hidden { "hidden" } else { "allowed" },
            "updatedAt": timestamp()
        }),
    );
    store.write_collection("skills", document)?;
    append_activity(
        store,
        if hidden {
            "skill.hidden"
        } else {
            "skill.revealed"
        },
        json!({"target": agent_id, "agentId": agent_id, "skillId": skill_id}),
    )?;
    Ok(json!({
        "ok": true,
        "agentId": agent_id,
        "skillId": skill_id,
        "hidden": hidden
    }))
}

fn is_visible_by_policy(store: &ClientStateStore, agent_id: &str, skill_id: &str) -> Result<bool> {
    let pairing = get_approved_pairing(store, agent_id)?;
    let policy = pairing
        .as_ref()
        .and_then(|pairing| {
            pairing
                .get("defaultVisibilityPolicy")
                .and_then(Value::as_str)
        })
        .unwrap_or("allow-all");
    match policy {
        "deny-by-default" => Ok(is_explicitly_revealed(store, agent_id, skill_id)),
        _ => Ok(true),
    }
}
