use super::super::station::{deletion_transport_hint, lease_transport_hint, station_context};
use super::poll::{StationPoll, receive_station_envelopes_with_config};
use crate::core::secure_mesh_crypto::SecureMeshPayloadKind;
use crate::core::secure_mesh_pairwise::SecureMeshPairwiseReceivedPayload;
use crate::domain::mobile_relay::endpoint_trust::{ensure_peer_verified, now_iso};
use crate::domain::mobile_relay::pairing::refresh_pairwise_acceptance_if_pending;
use crate::domain::mobile_relay::pairwise_session::{
    is_pairwise_replay_rejection_error,
    mobile_relay_pairwise_operation_with_runtime_secret_context,
    open_mobile_relay_payload_deferred, result_envelope_replay_proof_with_pairwise_operation,
};
use crate::domain::mobile_relay::secret_custody::{
    RuntimeSecretMaterial, load_config_with_runtime_secret_context,
    load_config_with_runtime_secret_context_for_operation,
    mobile_relay_e2ee_secret_store_authorization_batch_operation_count,
};
use crate::domain::mobile_relay::support::{CONFIG_SCHEMA_VERSION, text_param};
use anyhow::{Result, anyhow, ensure};
use base64::{Engine as _, engine::general_purpose};
use licoup_protocol_bindings::licoarc_relay::LicoArcRelayEnvelope;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

const MAX_SECURE_RESULT_SCAN_LIMIT: u64 = 16;

struct ReceivedResult {
    mailbox_id: String,
    envelope: LicoArcRelayEnvelope,
    lease_transport_hint: Value,
}

pub fn command_result(params: &Value) -> Result<Value> {
    let (config, secret_context) = load_config_with_runtime_secret_context(params)?;
    let received = receive_result_with_config(params, &config, &secret_context.material)?;
    Ok(received_result_projection(&received)?)
}

fn receive_result_with_config(
    params: &Value,
    config: &Value,
    secret_material: &RuntimeSecretMaterial,
) -> Result<ReceivedResult> {
    let poll = receive_station_envelopes_with_config(params, config, secret_material)?;
    select_result(params, poll)
}

fn select_result(params: &Value, mut poll: StationPoll) -> Result<ReceivedResult> {
    let requested_envelope_id = text_param(params, &["envelopeId"]);
    let index = poll
        .envelopes
        .iter()
        .position(|envelope| {
            requested_envelope_id
                .as_deref()
                .is_none_or(|expected| envelope.envelope_id() == expected)
        })
        .ok_or_else(|| anyhow!("Lico Arc result envelope is not available"))?;
    let envelope = poll.envelopes.swap_remove(index);
    Ok(ReceivedResult {
        mailbox_id: envelope.mailbox_id().to_string(),
        envelope,
        lease_transport_hint: lease_transport_hint(poll.lease_hint),
    })
}

fn received_result_projection(received: &ReceivedResult) -> Result<Value> {
    Ok(json!({
        "ok": true,
        "schemaVersion": CONFIG_SCHEMA_VERSION,
        "command": {
            "resultEnvelope": serde_json::from_str::<Value>(&received.envelope.to_json()?)?,
            "envelopeId": received.envelope.envelope_id()
        },
        "transportHint": {
            "lease": received.lease_transport_hint
        }
    }))
}

pub fn command_result_secure(params: &Value) -> Result<Value> {
    let receive_limit = params
        .get("limit")
        .and_then(Value::as_u64)
        .unwrap_or(MAX_SECURE_RESULT_SCAN_LIMIT);
    let (mut config, mut secret_context) = load_config_with_runtime_secret_context_for_operation(
        params,
        "Mobile Relay secure result operation authorization batch",
        mobile_relay_e2ee_secret_store_authorization_batch_operation_count()
            .saturating_add((receive_limit as usize).saturating_mul(5))
            .saturating_add(8),
    )?;
    refresh_pairwise_acceptance_if_pending(params, &mut config, &mut secret_context)?;
    ensure_peer_verified(&config)?;
    let mut operation = mobile_relay_pairwise_operation_with_runtime_secret_context(
        &config,
        "Mobile Relay secure result operation authorization batch",
        5,
        &mut secret_context,
    )?;
    if let Some(receipt_id) = text_param(params, &["acknowledgeReceiptId"]) {
        let _ = operation.delete_received_payload(&receipt_id)?;
        return Ok(json!({
            "ok": true,
            "schemaVersion": CONFIG_SCHEMA_VERSION,
            "acknowledged": true,
            "bodyRedacted": true
        }));
    }
    let (expected_command_id, expected_idempotency_key) = requested_result_binding(params)?;
    let expected_binding_digest =
        result_binding_digest(&expected_command_id, &expected_idempotency_key);
    if let Some(received) = operation.received_payload(&expected_binding_digest)? {
        return secure_received_result_projection(&received, None);
    }
    drop(operation);

    let mut receive_params = params.clone();
    if receive_params.get("limit").is_none() {
        receive_params
            .as_object_mut()
            .ok_or_else(|| anyhow!("mobile relay secure result parameters must be an object"))?
            .insert(
                "limit".to_string(),
                Value::from(MAX_SECURE_RESULT_SCAN_LIMIT),
            );
    }
    let poll =
        receive_station_envelopes_with_config(&receive_params, &config, &secret_context.material)?;
    let station = station_context(params, &config)?;
    let lease_hint = lease_transport_hint(poll.lease_hint);
    let requested_envelope_id = text_param(params, &["envelopeId"]);
    for candidate in poll.envelopes {
        if requested_envelope_id
            .as_deref()
            .is_some_and(|expected| candidate.envelope_id() != expected)
        {
            continue;
        }
        let envelope = serde_json::from_str::<Value>(&candidate.to_json()?)?;
        let mut operation = mobile_relay_pairwise_operation_with_runtime_secret_context(
            &config,
            "Mobile Relay secure result operation authorization batch",
            3,
            &mut secret_context,
        )?;
        let opened = match open_mobile_relay_payload_deferred(
            &config,
            &secret_context.material,
            &envelope,
            SecureMeshPayloadKind::ResultPayload,
            &mut operation,
        ) {
            Ok(opened) => opened,
            Err(error) if is_pairwise_replay_rejection_error(&error) => {
                let _ = station
                    .transport
                    .delete_envelope(candidate.mailbox_id(), candidate.envelope_id());
                continue;
            }
            Err(_) => continue,
        };
        let result_payload = match serde_json::from_slice::<Value>(&opened) {
            Ok(payload) => payload,
            Err(_) => {
                operation.commit()?;
                let _ = station
                    .transport
                    .delete_envelope(candidate.mailbox_id(), candidate.envelope_id());
                continue;
            }
        };
        let Some((candidate_command_id, candidate_idempotency_key)) =
            result_payload_binding(&result_payload)
        else {
            operation.commit()?;
            let _ = station
                .transport
                .delete_envelope(candidate.mailbox_id(), candidate.envelope_id());
            continue;
        };
        let candidate_binding_digest =
            result_binding_digest(&candidate_command_id, &candidate_idempotency_key);
        if let Some(received) = operation.received_payload(&candidate_binding_digest)? {
            operation.commit()?;
            let deletion_hint = station
                .transport
                .delete_envelope(candidate.mailbox_id(), candidate.envelope_id())
                .map(deletion_transport_hint)
                .unwrap_or_else(|_| deletion_not_reported_hint());
            if candidate_binding_digest == expected_binding_digest {
                return secure_received_result_projection(
                    &received,
                    Some((&lease_hint, &deletion_hint)),
                );
            }
            continue;
        }
        let received = SecureMeshPairwiseReceivedPayload {
            receipt_id: candidate.envelope_id().to_string(),
            binding_digest: candidate_binding_digest.clone(),
            mailbox_id: candidate.mailbox_id().to_string(),
            payload_json: serde_json::to_string(&result_payload)?,
            received_at: now_iso(),
        };
        operation.commit_with_received_payload(&received)?;
        let deletion_hint = station
            .transport
            .delete_envelope(candidate.mailbox_id(), candidate.envelope_id())
            .map(deletion_transport_hint)
            .unwrap_or_else(|_| deletion_not_reported_hint());
        if candidate_binding_digest == expected_binding_digest {
            return secure_received_result_projection(
                &received,
                Some((&lease_hint, &deletion_hint)),
            );
        }
    }
    Ok(json!({
        "ok": true,
        "schemaVersion": CONFIG_SCHEMA_VERSION,
        "openedResult": Value::Null,
        "pending": true,
        "bodyRedacted": true,
        "transportHint": {
            "lease": lease_hint
        }
    }))
}

fn requested_result_binding(params: &Value) -> Result<(String, String)> {
    let command_id = text_param(params, &["commandId"])
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| anyhow!("mobile relay secure result command binding is missing"))?;
    let idempotency_key = text_param(params, &["idempotencyKey"])
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| anyhow!("mobile relay secure result idempotency binding is missing"))?;
    Ok((command_id, idempotency_key))
}

fn result_payload_binding(result: &Value) -> Option<(String, String)> {
    let evaluation_command_id = result
        .get("evaluation")
        .and_then(|value| value.get("commandId"))
        .and_then(Value::as_str)?
        .trim();
    let execution = result.get("execution");
    let execution_command_id = execution
        .and_then(|value| value.get("commandId"))
        .and_then(Value::as_str)?
        .trim();
    let execution_idempotency_key = execution
        .and_then(|value| value.get("idempotencyKey"))
        .and_then(Value::as_str)?
        .trim();
    if evaluation_command_id.is_empty()
        || execution_command_id.is_empty()
        || execution_idempotency_key.is_empty()
        || evaluation_command_id != execution_command_id
    {
        return None;
    }
    Some((
        execution_command_id.to_string(),
        execution_idempotency_key.to_string(),
    ))
}

fn result_binding_digest(command_id: &str, idempotency_key: &str) -> String {
    let mut digest = Sha256::new();
    digest.update(b"LICOUP-SECURE-RESULT-BINDING-v1");
    for value in [command_id, idempotency_key] {
        digest.update((value.len() as u64).to_be_bytes());
        digest.update(value.as_bytes());
    }
    general_purpose::URL_SAFE_NO_PAD.encode(digest.finalize())
}

fn secure_received_result_projection(
    received: &SecureMeshPairwiseReceivedPayload,
    transport: Option<(&Value, &Value)>,
) -> Result<Value> {
    let opened_result: Value = serde_json::from_str(&received.payload_json)?;
    let mut projection = json!({
        "ok": true,
        "schemaVersion": CONFIG_SCHEMA_VERSION,
        "openedResult": opened_result,
        "pending": false,
        "bodyRedacted": true,
        "resultReceiptId": received.receipt_id
    });
    if let Some((lease, delete)) = transport {
        projection["transportHint"] = json!({
            "lease": lease,
            "delete": delete
        });
    }
    Ok(projection)
}

fn deletion_not_reported_hint() -> Value {
    json!({
        "stationReportedAcknowledged": false
    })
}

pub fn command_result_replay_proof(params: &Value) -> Result<Value> {
    let (mut config, mut secret_context) = load_config_with_runtime_secret_context_for_operation(
        params,
        "Mobile Relay secure result replay proof authorization batch",
        mobile_relay_e2ee_secret_store_authorization_batch_operation_count().saturating_add(5),
    )?;
    refresh_pairwise_acceptance_if_pending(params, &mut config, &mut secret_context)?;
    let received = receive_result_with_config(params, &config, &secret_context.material)?;
    let envelope = serde_json::from_str::<Value>(&received.envelope.to_json()?)?;
    let mut pairwise_operation = mobile_relay_pairwise_operation_with_runtime_secret_context(
        &config,
        "Mobile Relay secure result replay proof authorization batch",
        5,
        &mut secret_context,
    )?;
    let mut proof = result_envelope_replay_proof_with_pairwise_operation(
        &config,
        &secret_context.material,
        &envelope,
        json!({
            "ok": true,
            "command": {
                "resultEnvelopePresent": true
            }
        }),
        &mut pairwise_operation,
    )?;
    ensure!(
        proof.get("ok").and_then(Value::as_bool) == Some(true),
        "mobile relay endpoint replay proof is incomplete"
    );
    let station = station_context(params, &config)?;
    let deletion_hint = station
        .transport
        .delete_envelope(&received.mailbox_id, received.envelope.envelope_id())?;
    let object = proof
        .as_object_mut()
        .ok_or_else(|| anyhow!("mobile relay endpoint replay proof is invalid"))?;
    object.remove("ackPurgeReady");
    object.insert(
        "transportHint".to_string(),
        json!({
            "lease": received.lease_transport_hint,
            "delete": deletion_transport_hint(deletion_hint)
        }),
    );
    Ok(proof)
}
