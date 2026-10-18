use super::{
    STATUS_APPROVED, STATUS_REVOKED, agent_id, append_activity, collection_items_mut, target_id,
    timestamp, uuid_v4,
};
use crate::platform::client_state::ClientStateStore;
use anyhow::Result;
use serde_json::{Value, json};

pub(super) fn request(store: &ClientStateStore, params: &Value) -> Result<Value> {
    let agent_id = agent_id(params)?;
    let target = target_id(params).unwrap_or_else(|| "manual".to_string());
    let target_kind = params
        .get("targetKind")
        .and_then(Value::as_str)
        .unwrap_or("unknown");
    let label = params
        .get("label")
        .and_then(Value::as_str)
        .unwrap_or(&agent_id);
    let config_path = params
        .get("configPath")
        .and_then(Value::as_str)
        .unwrap_or("");
    let binary_path = params
        .get("binaryPath")
        .and_then(Value::as_str)
        .unwrap_or("");
    let pairing_id = format!("pair-{}", uuid_v4());
    let local_identity = format!("local-{}", uuid_v4());
    let visibility_policy = params
        .get("defaultVisibilityPolicy")
        .and_then(Value::as_str)
        .unwrap_or("deny-by-default");

    // Local skill management is itself the user's explicit action, so a
    // pairing request is approved immediately; the record remains only as an
    // audit trail of which agents the user chose to manage.
    let now = timestamp();
    let mut document = store.read_collection("pairings")?;
    let items = collection_items_mut(&mut document)?;
    items.retain(|item| item.get("agentId").and_then(Value::as_str) != Some(&agent_id));
    let record = json!({
        "pairingId": pairing_id,
        "agentId": agent_id,
        "target": target,
        "targetKind": target_kind,
        "label": label,
        "configPath": config_path,
        "binaryPath": binary_path,
        "localIdentity": local_identity,
        "status": STATUS_APPROVED,
        "requestedAt": now,
        "approvedAt": now,
        "defaultVisibilityPolicy": visibility_policy,
        "scopes": [],
    });
    items.push(record.clone());
    store.write_collection("pairings", document)?;
    append_activity(
        store,
        "pairing.requested",
        json!({"target": target, "agentId": agent_id, "pairingId": pairing_id}),
    )?;
    append_activity(
        store,
        "pairing.approved",
        json!({"target": target, "agentId": agent_id, "pairingId": pairing_id}),
    )?;
    Ok(json!({
        "ok": true,
        "status": STATUS_APPROVED,
        "pairing": record
    }))
}

pub(super) fn approve(store: &ClientStateStore, params: &Value) -> Result<Value> {
    update_status(store, params, STATUS_APPROVED, "pairing.approved")
}

pub(super) fn revoke(store: &ClientStateStore, params: &Value) -> Result<Value> {
    update_status(store, params, STATUS_REVOKED, "pairing.revoked")
}

pub(super) fn list(store: &ClientStateStore, params: &Value) -> Result<Value> {
    let document = store.read_collection("pairings")?;
    let mut pairings = document
        .get("items")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    if let Some(agent) = params.get("agent").and_then(Value::as_str) {
        pairings.retain(|item| item.get("agentId").and_then(Value::as_str) == Some(agent));
    }
    Ok(json!({
        "ok": true,
        "pairings": pairings
    }))
}

fn update_status(
    store: &ClientStateStore,
    params: &Value,
    status: &str,
    event_type: &str,
) -> Result<Value> {
    let agent_id = agent_id(params)?;
    let mut document = store.read_collection("pairings")?;
    let items = collection_items_mut(&mut document)?;
    let mut updated = None::<Value>;
    for item in items.iter_mut() {
        if item.get("agentId").and_then(Value::as_str) == Some(&agent_id) {
            item["status"] = json!(status);
            let status_time_key = match status {
                STATUS_APPROVED => "approvedAt",
                STATUS_REVOKED => "revokedAt",
                _ => "updatedAt",
            };
            item[status_time_key] = json!(timestamp());
            updated = Some(item.clone());
            break;
        }
    }
    let Some(record) = updated else {
        return Ok(json!({
            "ok": false,
            "error": "pairing_not_found",
            "agentId": agent_id
        }));
    };
    store.write_collection("pairings", document)?;
    append_activity(
        store,
        event_type,
        json!({
            "target": record.get("target").and_then(Value::as_str).unwrap_or(""),
            "agentId": agent_id
        }),
    )?;
    Ok(json!({
        "ok": true,
        "status": status,
        "pairing": record
    }))
}
