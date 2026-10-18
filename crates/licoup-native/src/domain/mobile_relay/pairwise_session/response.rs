use super::crypto_operation::open_mobile_relay_payload_with_pairwise_operation;
use super::transaction::MobileRelayPairwiseOperation;
#[cfg(test)]
use super::transaction::mobile_relay_pairwise_operation;
use crate::core::secure_mesh_crypto::SecureMeshPayloadKind;
use crate::domain::mobile_relay::endpoint_trust::ensure_peer_verified;
use crate::domain::mobile_relay::relay_operations::validate_secure_envelope;
use crate::domain::mobile_relay::secret_custody::RuntimeSecretMaterial;
use crate::domain::mobile_relay::support::CONFIG_SCHEMA_VERSION;
use anyhow::{Result, anyhow};
use serde_json::{Value, json};

#[cfg(test)]
pub(in crate::domain::mobile_relay) fn secure_result_response_summary(response: &Value) -> Value {
    let command = response.get("command").unwrap_or(&Value::Null);
    json!({
        "ok": response.get("ok").and_then(Value::as_bool).unwrap_or(false),
        "command": {
            "commandId": command.get("commandId").and_then(Value::as_str).unwrap_or_default(),
            "status": command.get("status").and_then(Value::as_str).unwrap_or_default(),
            "resultEnvelopePresent": command
                .get("resultEnvelope")
                .map(Value::is_object)
                .unwrap_or(false)
        },
        "ackPurge": {
            "purged": response
                .get("ackPurge")
                .and_then(|ack| ack.get("purged"))
                .and_then(Value::as_bool)
                .unwrap_or(false)
        },
        "bodyRedacted": true
    })
}

#[cfg(test)]
pub(in crate::domain::mobile_relay) fn result_envelope_replay_proof(
    config: &Value,
    secret_material: &RuntimeSecretMaterial,
    envelope: &Value,
    response_summary: Value,
) -> Result<Value> {
    ensure_peer_verified(config)?;
    let mut pairwise_operation = mobile_relay_pairwise_operation(
        config,
        secret_material,
        "Mobile Relay secure result replay proof authorization batch",
        5,
    )?;
    result_envelope_replay_proof_with_pairwise_operation(
        config,
        secret_material,
        envelope,
        response_summary,
        &mut pairwise_operation,
    )
}

pub(in crate::domain::mobile_relay) fn result_envelope_replay_proof_with_pairwise_operation(
    config: &Value,
    secret_material: &RuntimeSecretMaterial,
    envelope: &Value,
    response_summary: Value,
    pairwise_operation: &mut MobileRelayPairwiseOperation,
) -> Result<Value> {
    ensure_peer_verified(config)?;
    validate_secure_envelope(envelope)?;
    let opened = open_mobile_relay_payload_with_pairwise_operation(
        config,
        secret_material,
        envelope,
        SecureMeshPayloadKind::ResultPayload,
        pairwise_operation,
    )?;
    let first_payload = serde_json::from_slice::<Value>(&opened).map_err(|error| {
        anyhow!("mobile relay secure replay proof payload is not JSON: {error}")
    })?;
    let first_open_ok = first_payload.get("ok").and_then(Value::as_bool) == Some(true);
    let first_body_redacted =
        first_payload.get("bodyRedacted").and_then(Value::as_bool) == Some(true);
    let first_evaluation_code = first_payload
        .get("evaluation")
        .and_then(|evaluation| evaluation.get("code"))
        .and_then(Value::as_str)
        .unwrap_or_default();
    let first_execution_outcome = first_payload
        .get("execution")
        .and_then(|execution| execution.get("outcome"))
        .and_then(Value::as_str)
        .unwrap_or_default();
    let replay_rejected = match open_mobile_relay_payload_with_pairwise_operation(
        config,
        secret_material,
        envelope,
        SecureMeshPayloadKind::ResultPayload,
        pairwise_operation,
    ) {
        Ok(_) => false,
        Err(error) => is_pairwise_replay_rejection_error(&error),
    };
    let ack_purge_ready = response_summary
        .get("ackPurge")
        .and_then(|ack| ack.get("purged"))
        .and_then(Value::as_bool)
        == Some(true);
    let result_envelope_present = response_summary
        .get("command")
        .and_then(|command| command.get("resultEnvelopePresent"))
        .and_then(Value::as_bool)
        == Some(true);
    let proof_ready =
        first_open_ok && first_body_redacted && replay_rejected && result_envelope_present;
    Ok(json!({
        "ok": proof_ready,
        "schemaVersion": CONFIG_SCHEMA_VERSION,
        "resultEnvelopePresent": result_envelope_present,
        "ackPurgeReady": ack_purge_ready,
        "firstOpenOk": first_open_ok,
        "firstOpenBodyRedacted": first_body_redacted,
        "firstOpenEvaluationCode": first_evaluation_code,
        "firstOpenExecutionOutcome": first_execution_outcome,
        "replayRejected": replay_rejected,
        "replayErrorRedacted": true,
        "bodyRedacted": true
    }))
}

pub(in crate::domain::mobile_relay) fn is_pairwise_replay_rejection(error: &str) -> bool {
    let normalized = error.to_ascii_lowercase();
    normalized.contains("replay detected")
        || normalized.contains("stale ratchet epoch")
        || normalized.contains("stale chain index")
}

pub(in crate::domain::mobile_relay) fn is_pairwise_replay_rejection_error(
    error: &anyhow::Error,
) -> bool {
    is_pairwise_replay_rejection(&format!("{error:#}"))
}
