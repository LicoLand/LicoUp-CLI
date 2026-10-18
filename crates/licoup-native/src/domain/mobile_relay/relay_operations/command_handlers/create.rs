use super::super::envelope::{relay_envelope_from_value, secure_envelope_param};
use super::super::station::{delivery_transport_hint, station_binding_digest, station_context};
use crate::core::secure_mesh_crypto::SecureMeshPayloadKind;
use crate::core::secure_mesh_pairwise::SecureMeshPairwisePendingDelivery;
use crate::domain::mobile_relay::endpoint_collaboration_gate::ensure_outbound_permitted;
use crate::domain::mobile_relay::endpoint_trust::ensure_peer_verified;
use crate::domain::mobile_relay::pairwise_session::{
    mobile_relay_pairwise_operation_with_runtime_secret_context,
    seal_mobile_relay_payload_deferred, secure_command_payload,
};
use crate::domain::mobile_relay::secret_custody::{
    MobileRelayE2eeSecretField, RuntimeSecretMaterial,
    ensure_secure_mesh_protected_operation_allowed, load_config,
    load_config_with_runtime_secret_context_for_operation,
    mobile_relay_e2ee_secret_store_authorization_batch_operation_count,
};
use crate::domain::mobile_relay::support::{CONFIG_SCHEMA_VERSION, json_param, text_param};
use anyhow::{Result, anyhow, ensure};
use base64::{Engine as _, engine::general_purpose};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use time::{OffsetDateTime, format_description::well_known::Rfc3339};

const PENDING_COMMAND_BINDING_SCHEMA: &str = "licoup.mobile-relay.pending-command.v1";
const PENDING_COMMAND_DELIVERY_KIND: &str = "command";

#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct PendingSecureCommandBinding {
    schema: String,
    command_id: String,
    idempotency_key: String,
    command_kind: String,
    station_binding_digest: String,
    intent_digest: String,
}

fn require_relay_private_key(material: &RuntimeSecretMaterial) -> Result<()> {
    let private_key = material
        .e2ee_secret(MobileRelayE2eeSecretField::PrivateKey)
        .ok_or_else(|| anyhow!("mobile relay private key material is missing"))?;
    ensure!(
        !private_key.expose_bytes().is_empty(),
        "mobile relay private key material is empty"
    );
    Ok(())
}

pub fn command_create(params: &Value) -> Result<Value> {
    ensure_outbound_permitted()?;
    ensure_secure_mesh_protected_operation_allowed()?;
    let config = load_config()?;
    let secure_envelope = secure_envelope_param(params)
        .ok_or_else(|| anyhow!("mobile relay command create requires --secure-envelope"))?;
    let envelope = relay_envelope_from_value(&secure_envelope)?;
    let station = station_context(params, &config)?;
    let hint = station.transport.send_envelope(&envelope)?;
    Ok(json!({
        "ok": true,
        "schemaVersion": CONFIG_SCHEMA_VERSION,
        "transportHint": delivery_transport_hint(hint)
    }))
}

pub fn command_create_secure(params: &Value) -> Result<Value> {
    ensure_outbound_permitted()?;
    let (config, mut secret_context) = load_config_with_runtime_secret_context_for_operation(
        params,
        "Mobile Relay secure command create authorization batch",
        mobile_relay_e2ee_secret_store_authorization_batch_operation_count().saturating_add(3),
    )?;
    require_relay_private_key(&secret_context.material)?;
    ensure_peer_verified(&config)?;
    let body = json_param(params, "body")
        .or_else(|| json_param(params, "payload"))
        .unwrap_or_else(|| json!({}));
    let command_kind = text_param(params, &["commandKind", "type", "command"])
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| "agent.message.send".to_string());
    let target_agent_id = text_param(params, &["targetAgentId", "agentId", "agent", "target"]);
    let workspace_id = text_param(params, &["workspaceId", "workspace"])
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| "default".to_string());
    let client_intent_id =
        text_param(params, &["clientIntentId"]).filter(|value| !value.trim().is_empty());
    if let Some(client_intent_id) = &client_intent_id {
        ensure!(
            client_intent_id.len() <= 128
                && client_intent_id
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_' || byte == b'-'),
            "mobile relay client intent ID is invalid"
        );
    }
    let intent_digest = secure_command_intent_digest(
        &command_kind,
        target_agent_id.as_deref(),
        &workspace_id,
        &body,
        client_intent_id.as_deref(),
    )?;
    let mut pairwise_operation = mobile_relay_pairwise_operation_with_runtime_secret_context(
        &config,
        "Mobile Relay secure command create authorization batch",
        3,
        &mut secret_context,
    )?;
    if let Some(recovered) =
        recover_pending_secure_command(params, &config, &intent_digest, &mut pairwise_operation)?
    {
        return Ok(recovered);
    }
    let payload = secure_command_payload(
        &config,
        &secret_context.material,
        &command_kind,
        target_agent_id.as_deref(),
        &workspace_id,
        body,
    )?;
    let payload_command_id = payload
        .get("commandId")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    let payload_idempotency_key = payload
        .get("idempotencyKey")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    let envelope = seal_mobile_relay_payload_deferred(
        &config,
        &secret_context.material,
        SecureMeshPayloadKind::Command,
        &payload,
        &mut pairwise_operation,
    )?;
    let relay_envelope = relay_envelope_from_value(&envelope)?;
    let binding = PendingSecureCommandBinding {
        schema: PENDING_COMMAND_BINDING_SCHEMA.to_string(),
        command_id: payload_command_id,
        idempotency_key: payload_idempotency_key,
        command_kind,
        station_binding_digest: station_binding_digest(params, &config)?,
        intent_digest,
    };
    let pending = SecureMeshPairwisePendingDelivery {
        delivery_kind: PENDING_COMMAND_DELIVERY_KIND.to_string(),
        envelope_id: relay_envelope.envelope_id().to_string(),
        expires_at: relay_envelope.expires_at().to_string(),
        envelope_json: relay_envelope.to_json()?,
        binding_json: serde_json::to_string(&binding)?,
        created_at: crate::domain::mobile_relay::endpoint_trust::now_iso(),
    };
    pairwise_operation.commit_with_pending_delivery(&pending)?;
    let station = station_context(params, &config)?;
    let hint = station.transport.send_envelope(&relay_envelope)?;
    ensure!(
        pairwise_operation
            .delete_pending_delivery(PENDING_COMMAND_DELIVERY_KIND, relay_envelope.envelope_id(),)?,
        "mobile relay pending command delivery disappeared"
    );
    Ok(secure_command_delivery_projection(&binding, hint, false))
}

fn recover_pending_secure_command(
    params: &Value,
    config: &Value,
    requested_intent_digest: &str,
    pairwise_operation: &mut crate::domain::mobile_relay::pairwise_session::MobileRelayPairwiseOperation,
) -> Result<Option<Value>> {
    let Some(pending) = pairwise_operation.pending_delivery(PENDING_COMMAND_DELIVERY_KIND)? else {
        return Ok(None);
    };
    let expires_at = OffsetDateTime::parse(&pending.expires_at, &Rfc3339)
        .map_err(|_| anyhow!("mobile relay pending command expiry is invalid"))?;
    if expires_at <= OffsetDateTime::now_utc() {
        return Err(anyhow!(
            "mobile relay pending command expired after ratchet commit; re-pairing is required"
        ));
    }
    let binding: PendingSecureCommandBinding = serde_json::from_str(&pending.binding_json)
        .map_err(|_| anyhow!("mobile relay pending command binding is invalid"))?;
    ensure!(
        binding.schema == PENDING_COMMAND_BINDING_SCHEMA
            && !binding.intent_digest.is_empty()
            && binding.station_binding_digest == station_binding_digest(params, config)?,
        "mobile relay pending command station binding changed"
    );
    ensure!(
        binding.intent_digest == requested_intent_digest,
        "a different secure command delivery is pending"
    );
    let envelope = licoup_protocol_bindings::licoarc_relay::LicoArcRelayEnvelope::from_json(
        &pending.envelope_json,
    )?;
    ensure!(
        envelope.envelope_id() == pending.envelope_id
            && envelope.expires_at() == pending.expires_at,
        "mobile relay pending command envelope binding is invalid"
    );
    let station = station_context(params, config)?;
    let hint = station.transport.send_envelope(&envelope)?;
    ensure!(
        pairwise_operation
            .delete_pending_delivery(PENDING_COMMAND_DELIVERY_KIND, envelope.envelope_id())?,
        "mobile relay pending command delivery disappeared"
    );
    Ok(Some(secure_command_delivery_projection(
        &binding, hint, true,
    )))
}

fn secure_command_intent_digest(
    command_kind: &str,
    target_agent_id: Option<&str>,
    workspace_id: &str,
    body: &Value,
    client_intent_id: Option<&str>,
) -> Result<String> {
    let canonical = canonical_json_value(&json!({
        "schema": "licoup.mobile-relay.command-intent.v1",
        "commandKind": command_kind,
        "targetAgentId": target_agent_id,
        "workspaceId": workspace_id,
        "body": body,
        "clientIntentId": client_intent_id
    }));
    let encoded = serde_json::to_vec(&canonical)?;
    Ok(general_purpose::URL_SAFE_NO_PAD.encode(Sha256::digest(encoded)))
}

fn canonical_json_value(value: &Value) -> Value {
    match value {
        Value::Array(values) => {
            Value::Array(values.iter().map(canonical_json_value).collect::<Vec<_>>())
        }
        Value::Object(values) => {
            let ordered = values
                .iter()
                .map(|(key, value)| (key.clone(), canonical_json_value(value)))
                .collect::<BTreeMap<_, _>>();
            Value::Object(ordered.into_iter().collect())
        }
        _ => value.clone(),
    }
}

fn secure_command_delivery_projection(
    binding: &PendingSecureCommandBinding,
    hint: crate::platform::badtower_station::BadTowerDeliveryTransportHint,
    recovered_pending_delivery: bool,
) -> Value {
    json!({
        "ok": true,
        "schemaVersion": CONFIG_SCHEMA_VERSION,
        "transportHint": delivery_transport_hint(hint),
        "secureCommandBinding": {
            "payloadCommandId": binding.command_id,
            "idempotencyKey": binding.idempotency_key,
            "commandKind": binding.command_kind,
            "recoveredPendingDelivery": recovered_pending_delivery,
        }
    })
}
