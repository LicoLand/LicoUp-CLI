use anyhow::{Result, anyhow};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use super::endpoint_collaboration_gate::ensure_outbound_permitted;
use super::endpoint_trust::{ensure_peer_verified, now_iso};
#[cfg(test)]
use super::pairwise_session::mobile_relay_pairwise_operation;
use super::pairwise_session::{
    MobileRelayPairwiseOperation, is_pairwise_replay_rejection_error,
    mobile_relay_pairwise_operation_with_runtime_secret_context,
    open_mobile_relay_payload_deferred, seal_mobile_relay_payload_deferred, secure_command_context,
};
use super::relay_operations::{
    deletion_transport_hint, delivery_transport_hint, lease_transport_hint,
    local_command_from_relay_delivery, pc_check_in_with_context,
    receive_station_envelopes_with_config, relay_envelope_from_value, station_binding_digest,
    station_context, station_lease_seconds, validate_secure_envelope,
};
#[cfg(test)]
use super::secret_custody::load_config_with_runtime_secret_context;
use super::secret_custody::{
    CONFIG_SCHEMA_VERSION, RuntimeSecretMaterial,
    load_config_with_runtime_secret_context_for_operation,
    mobile_relay_e2ee_secret_store_authorization_batch_operation_count,
};
use super::support::{
    SECURE_MESH_ENDPOINT_CRYPTO_RUNTIME_FAILED_CODE,
    SECURE_MESH_ENDPOINT_CRYPTO_RUNTIME_FAILED_DETAIL,
};
use crate::core::secure_mesh_pairwise::SecureMeshPairwisePendingDelivery;

const PENDING_RESULT_BINDING_SCHEMA: &str = "licoup.mobile-relay.pending-result.v1";
const PENDING_RESULT_DELIVERY_KIND: &str = "result";

#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct PendingSecureResultBinding {
    schema: String,
    station_binding_digest: String,
    received_mailbox_id: String,
    received_envelope_id: String,
}

/// Synchronize relay deliveries using one bounded secret-store authorization context.
pub fn commands_sync(params: &Value) -> Result<Value> {
    ensure_outbound_permitted()?;
    let command_limit = params.get("limit").and_then(Value::as_u64).unwrap_or(10) as usize;
    let (config, mut secret_context) = load_config_with_runtime_secret_context_for_operation(
        params,
        "Mobile Relay commands sync operation authorization batch",
        mobile_relay_e2ee_secret_store_authorization_batch_operation_count()
            .saturating_add(command_limit.saturating_mul(4))
            .saturating_add(4),
    )?;
    let check_in = pc_check_in_with_context(params, &config, &secret_context)?;
    let mut completed = Vec::<Value>::new();
    if let Some(recovered) = recover_pending_result_delivery(params, &config, &mut secret_context)?
    {
        completed.push(recovered);
    }
    let polled = receive_station_envelopes_with_config(params, &config, &secret_context.material)?;
    let delivery_values = polled
        .envelopes
        .iter()
        .map(|envelope| {
            envelope
                .to_json()
                .and_then(|wire| serde_json::from_str::<Value>(&wire).map_err(Into::into))
        })
        .collect::<Result<Vec<_>>>()?;
    let commands = delivery_values
        .iter()
        .map(local_command_from_relay_delivery)
        .collect::<Result<Vec<_>>>()?;
    let mut visible_commands = Vec::<Value>::new();
    for (command, delivery) in commands.iter().zip(delivery_values.iter()) {
        let redacted_command = redacted_relay_command(command);
        visible_commands.push(redacted_command.clone());
        let mut operation = match mobile_relay_pairwise_operation_with_runtime_secret_context(
            &config,
            "Mobile Relay commands sync operation authorization batch",
            5,
            &mut secret_context,
        ) {
            Ok(operation) => operation,
            Err(_error) => {
                completed.push(failed_completion(&redacted_command));
                continue;
            }
        };
        match execute_secure_envelope_command_with_pairwise_operation(
            command,
            params,
            &config,
            &secret_context.material,
            &mut operation,
        ) {
            Ok(result_envelope) => {
                let pending = pending_result_delivery(params, &config, delivery, &result_envelope)?;
                if operation.commit_with_pending_delivery(&pending).is_err() {
                    completed.push(failed_completion(&redacted_command));
                    continue;
                }
                match complete_authenticated_station_command(
                    params,
                    &config,
                    delivery,
                    &result_envelope,
                    &mut operation,
                ) {
                    Ok(completion) => {
                        completed.push(json!({
                            "command": redacted_command,
                            "ok": true,
                            "bodyRedacted": true,
                            "resultEnvelope": result_envelope,
                            "completion": completion
                        }));
                    }
                    Err(_error) => {
                        completed.push(json!({
                            "command": redacted_command,
                            "ok": false,
                            "bodyRedacted": true,
                            "error": "secure mesh result delivery remains pending",
                            "completion": {
                                "ok": false,
                                "code": "mobile_relay_result_delivery_pending"
                            }
                        }));
                        break;
                    }
                }
            }
            Err(error) => {
                if is_pairwise_replay_rejection_error(&error) {
                    let received = relay_envelope_from_value(delivery)?;
                    let station = station_context(params, &config)?;
                    let deletion = station
                        .transport
                        .delete_envelope(received.mailbox_id(), received.envelope_id())
                        .map(deletion_transport_hint)
                        .unwrap_or_else(|_| deletion_not_reported_hint());
                    completed.push(json!({
                        "command": redacted_command,
                        "ok": true,
                        "bodyRedacted": true,
                        "completion": {
                            "ok": true,
                            "code": "mobile_relay_authenticated_replay_cleaned",
                            "transportHint": {
                                "delete": deletion
                            }
                        }
                    }));
                } else {
                    completed.push(failed_completion(&redacted_command));
                }
            }
        }
    }
    Ok(json!({
        "ok": true,
        "schemaVersion": CONFIG_SCHEMA_VERSION,
        "checkIn": check_in,
        "commands": visible_commands,
        "completed": completed,
        "transportHint": {
            "pollLease": lease_transport_hint(polled.lease_hint)
        }
    }))
}

fn complete_authenticated_station_command(
    params: &Value,
    config: &Value,
    received_envelope: &Value,
    result_envelope: &Value,
    pairwise_operation: &mut MobileRelayPairwiseOperation,
) -> Result<Value> {
    ensure_outbound_permitted()?;
    let received = relay_envelope_from_value(received_envelope)?;
    let result = relay_envelope_from_value(result_envelope)?;
    let station = station_context(params, config)?;
    let delivery_hint = station.transport.send_envelope(&result)?;
    anyhow::ensure!(
        pairwise_operation
            .delete_pending_delivery(PENDING_RESULT_DELIVERY_KIND, result.envelope_id())?,
        "mobile relay pending result delivery disappeared"
    );
    let deletion_hint = station
        .transport
        .delete_envelope(received.mailbox_id(), received.envelope_id())
        .map(deletion_transport_hint)
        .unwrap_or_else(|_| deletion_not_reported_hint());
    Ok(json!({
        "ok": true,
        "schemaVersion": CONFIG_SCHEMA_VERSION,
        "transportHint": {
            "result": delivery_transport_hint(delivery_hint),
            "delete": deletion_hint
        }
    }))
}

fn pending_result_delivery(
    params: &Value,
    config: &Value,
    received_envelope: &Value,
    result_envelope: &Value,
) -> Result<SecureMeshPairwisePendingDelivery> {
    let received = relay_envelope_from_value(received_envelope)?;
    let result = relay_envelope_from_value(result_envelope)?;
    let binding = PendingSecureResultBinding {
        schema: PENDING_RESULT_BINDING_SCHEMA.to_string(),
        station_binding_digest: station_binding_digest(params, config)?,
        received_mailbox_id: received.mailbox_id().to_string(),
        received_envelope_id: received.envelope_id().to_string(),
    };
    Ok(SecureMeshPairwisePendingDelivery {
        delivery_kind: PENDING_RESULT_DELIVERY_KIND.to_string(),
        envelope_id: result.envelope_id().to_string(),
        expires_at: result.expires_at().to_string(),
        envelope_json: result.to_json()?,
        binding_json: serde_json::to_string(&binding)?,
        created_at: now_iso(),
    })
}

fn recover_pending_result_delivery(
    params: &Value,
    config: &Value,
    secret_context: &mut super::secret_custody::RuntimeSecretContext,
) -> Result<Option<Value>> {
    let mut operation = match mobile_relay_pairwise_operation_with_runtime_secret_context(
        config,
        "Mobile Relay pending result recovery authorization batch",
        3,
        secret_context,
    ) {
        Ok(operation) => operation,
        Err(_) => return Ok(None),
    };
    let Some(pending) = operation.pending_delivery(PENDING_RESULT_DELIVERY_KIND)? else {
        return Ok(None);
    };
    let binding: PendingSecureResultBinding = serde_json::from_str(&pending.binding_json)
        .map_err(|_| anyhow!("mobile relay pending result binding is invalid"))?;
    anyhow::ensure!(
        binding.schema == PENDING_RESULT_BINDING_SCHEMA
            && binding.station_binding_digest == station_binding_digest(params, config)?,
        "mobile relay pending result station binding changed"
    );
    let result = licoup_protocol_bindings::licoarc_relay::LicoArcRelayEnvelope::from_json(
        &pending.envelope_json,
    )?;
    anyhow::ensure!(
        result.envelope_id() == pending.envelope_id && result.expires_at() == pending.expires_at,
        "mobile relay pending result envelope binding is invalid"
    );
    let station = station_context(params, config)?;
    let result_hint = delivery_transport_hint(station.transport.send_envelope(&result)?);
    anyhow::ensure!(
        operation.delete_pending_delivery(PENDING_RESULT_DELIVERY_KIND, result.envelope_id())?,
        "mobile relay pending result delivery disappeared"
    );
    let _ = station
        .transport
        .lease_mailbox(&binding.received_mailbox_id, station_lease_seconds(params));
    let deletion = station
        .transport
        .delete_envelope(&binding.received_mailbox_id, &binding.received_envelope_id)
        .map(deletion_transport_hint)
        .unwrap_or_else(|_| deletion_not_reported_hint());
    Ok(Some(json!({
        "command": {
            "commandId": binding.received_envelope_id,
            "type": super::support::SECURE_MESH_ENVELOPE_COMMAND,
            "bodyRedacted": true,
            "secureEnvelopePresent": true
        },
        "ok": true,
        "bodyRedacted": true,
        "completion": {
            "ok": true,
            "code": "mobile_relay_pending_result_recovered",
            "recoveredPendingDelivery": true,
            "transportHint": {
                "result": result_hint,
                "delete": deletion
            }
        }
    })))
}

fn failed_completion(redacted_command: &Value) -> Value {
    json!({
        "command": redacted_command,
        "ok": false,
        "bodyRedacted": true,
        "error": SECURE_MESH_ENDPOINT_CRYPTO_RUNTIME_FAILED_DETAIL,
        "completion": {
            "ok": false,
            "code": SECURE_MESH_ENDPOINT_CRYPTO_RUNTIME_FAILED_CODE
        }
    })
}

fn deletion_not_reported_hint() -> Value {
    json!({
        "stationReportedAcknowledged": false
    })
}

#[cfg(test)]
fn reject_plaintext_relay_command(command: &Value) -> Value {
    let command_type = command
        .get("type")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let command_label = if command_type.is_empty() {
        "<missing>"
    } else {
        command_type
    };
    json!({
        "ok": false,
        "bodyRedacted": true,
        "error": format!(
            "mobile relay plaintext command {} requires SecureEnvelope transport",
            command_label
        ),
        "completion": {
            "ok": false,
            "code": "mobile_relay_plaintext_command_rejected"
        }
    })
}

pub(super) fn redacted_relay_command(command: &Value) -> Value {
    json!({
        "commandId": command.get("commandId").and_then(Value::as_str).unwrap_or_default(),
        "type": command.get("type").and_then(Value::as_str).unwrap_or_default(),
        "bodyRedacted": true,
        "secureEnvelopePresent": command_has_secure_envelope(command)
    })
}

fn command_has_secure_envelope(command: &Value) -> bool {
    command.get("envelope").is_some_and(Value::is_object)
        || command
            .get("payload")
            .and_then(|payload| payload.get("envelope"))
            .is_some_and(Value::is_object)
}

#[cfg(test)]
pub(super) fn execute_secure_envelope_command(command: &Value, params: &Value) -> Result<Value> {
    let (config, secret_context) = load_config_with_runtime_secret_context(params)?;
    ensure_peer_verified(&config)?;
    let mut pairwise_operation = mobile_relay_pairwise_operation(
        &config,
        &secret_context.material,
        "Mobile Relay secure command operation authorization batch",
        5,
    )?;
    let history_home = licoup_foundation::platform::paths::portable_data_dir()?;
    let result = crate::domain::secure_mesh_command_runtime::with_secure_command_test_history_home(
        &history_home,
        || {
            execute_secure_envelope_command_with_pairwise_operation(
                command,
                params,
                &config,
                &secret_context.material,
                &mut pairwise_operation,
            )
        },
    )?;
    pairwise_operation.commit()?;
    Ok(result)
}

fn execute_secure_envelope_command_with_pairwise_operation(
    command: &Value,
    params: &Value,
    config: &Value,
    secret_material: &RuntimeSecretMaterial,
    pairwise_operation: &mut MobileRelayPairwiseOperation,
) -> Result<Value> {
    ensure_peer_verified(config)?;
    let envelope = command
        .get("envelope")
        .cloned()
        .ok_or_else(|| anyhow!("secure mesh relay command is missing envelope"))?;
    validate_secure_envelope(&envelope)?;
    let opened = open_mobile_relay_payload_deferred(
        config,
        secret_material,
        &envelope,
        crate::core::secure_mesh_crypto::SecureMeshPayloadKind::Command,
        pairwise_operation,
    )?;
    let payload: Value = serde_json::from_slice(&opened)
        .map_err(|error| anyhow!("secure mesh command payload is not JSON: {}", error))?;
    let context = secure_command_context(config, secret_material, params, &payload)?;
    let ledger_path =
        crate::domain::secure_mesh_command_runtime::default_secure_command_ledger_path()?;
    let mut ledger =
        crate::core::secure_mesh_command::SecureCommandSqliteReplayLedger::open(ledger_path)?;
    let mut executor = crate::domain::secure_mesh_command_runtime::SecureCommandRuntimeExecutor;
    let completed_at = now_iso();
    let execution = crate::core::secure_mesh_command::execute_secure_command_json(
        &payload,
        &context,
        &mut ledger,
        &mut executor,
        completed_at,
    )
    .unwrap_or_else(|_error| {
        json!({
            "ok": false,
            "protocolVersion": crate::core::secure_mesh::SECURE_MESH_COMMAND_PROTOCOL_VERSION,
            "code": SECURE_MESH_ENDPOINT_CRYPTO_RUNTIME_FAILED_CODE,
            "error": SECURE_MESH_ENDPOINT_CRYPTO_RUNTIME_FAILED_DETAIL,
            "bodyRedacted": true
        })
    });
    seal_mobile_relay_payload_deferred(
        config,
        secret_material,
        crate::core::secure_mesh_crypto::SecureMeshPayloadKind::ResultPayload,
        &execution,
        pairwise_operation,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn command_visibility_policy_redacts_plaintext_and_reports_envelope_presence() {
        let command = json!({
            "commandId": "cmd-test",
            "type": "plaintext.command",
            "payload": {
                "sensitive": "must-not-escape",
                "envelope": {}
            }
        });

        let visible = redacted_relay_command(&command);
        let rejection = reject_plaintext_relay_command(&command);

        assert_eq!(visible["commandId"], json!("cmd-test"));
        assert_eq!(visible["secureEnvelopePresent"], json!(true));
        assert!(visible.get("payload").is_none());
        assert_eq!(
            rejection["completion"]["code"],
            json!("mobile_relay_plaintext_command_rejected")
        );
        assert_eq!(rejection["bodyRedacted"], json!(true));
    }
}
