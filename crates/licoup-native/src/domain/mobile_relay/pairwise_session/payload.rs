use crate::domain::mobile_relay::endpoint_trust::{
    local_endpoint_state, now_iso, peer_endpoint_state, timestamp_after_seconds,
};
use crate::domain::mobile_relay::relay_operations::allowed_agent_ids;
use crate::domain::mobile_relay::secret_custody::RuntimeSecretMaterial;
use crate::domain::mobile_relay::support::{MOBILE_RELAY_COMMAND_TTL_SECONDS, json_param};
use anyhow::Result;
use serde_json::{Value, json};
use uuid::Uuid;

pub(in crate::domain::mobile_relay) fn secure_command_payload(
    config: &Value,
    secret_material: &RuntimeSecretMaterial,
    command_kind: &str,
    target_agent_id: Option<&str>,
    workspace_id: &str,
    body: Value,
) -> Result<Value> {
    let endpoint = local_endpoint_state(config, secret_material)?;
    let peer = peer_endpoint_state(config)?;
    let created_at = now_iso();
    let expires_at = timestamp_after_seconds(MOBILE_RELAY_COMMAND_TTL_SECONDS)?;
    Ok(json!({
        "schema": crate::core::secure_mesh::SECURE_MESH_COMMAND_PROTOCOL_VERSION,
        "commandId": format!("cmd_{}", Uuid::new_v4()),
        "commandKind": command_kind,
        "senderIdentity": {
            "endpointId": endpoint.endpoint_id,
            "identityFingerprint": endpoint.fingerprint,
            "trustState": "verified",
            "endpointKind": endpoint.endpoint_kind
        },
        "targetBinding": {
            "targetEndpointId": peer.endpoint_id,
            "targetAgentId": target_agent_id.map(Value::from).unwrap_or(Value::Null),
            "workspaceId": workspace_id
        },
        "riskClass": if matches!(
            command_kind,
            "agent.sessions.list" | "agent.sessions.describe"
        ) {
            "read_only"
        } else {
            "safe_write"
        },
        "requiresUserConfirmation": false,
        "idempotencyKey": format!("idem_{}", Uuid::new_v4()),
        "createdAt": created_at,
        "expiresAt": expires_at,
        "body": body
    }))
}

pub(in crate::domain::mobile_relay) fn secure_command_context(
    config: &Value,
    secret_material: &RuntimeSecretMaterial,
    params: &Value,
    payload: &Value,
) -> Result<Value> {
    let endpoint = local_endpoint_state(config, secret_material)?;
    let peer = peer_endpoint_state(config)?;
    let has_agent_binding = payload
        .get("targetBinding")
        .and_then(|binding| binding.get("targetAgentId"))
        .and_then(Value::as_str)
        .map(|value| !value.trim().is_empty())
        .unwrap_or(false);
    let command_kind = payload
        .get("commandKind")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let allowed_agents = if has_agent_binding {
        allowed_agent_ids(params, command_kind)?
    } else {
        json!([])
    };
    let allowed_workspaces = json_param(params, "allowedWorkspaceIds")
        .filter(Value::is_array)
        .unwrap_or_else(|| json!(["default"]));
    Ok(json!({
        "localEndpointId": endpoint.endpoint_id,
        "senderEndpointId": peer.endpoint_id,
        "senderIdentityFingerprint": peer.fingerprint,
        "senderTrustState": "verified",
        "senderEndpointKind": peer.endpoint_kind,
        "senderRosterActive": true,
        "targetRosterActive": true,
        "sessionOrEpochValid": true,
        "userConfirmed": false,
        "allowedWorkspaceIds": allowed_workspaces,
        "allowedAgentIds": allowed_agents,
        "now": now_iso()
    }))
}
