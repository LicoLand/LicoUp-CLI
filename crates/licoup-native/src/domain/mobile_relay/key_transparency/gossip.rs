use crate::core::secure_mesh_transparency::SecureMeshKtGossipPayload;
use crate::domain::mobile_relay::endpoint_trust::{
    current_secure_mesh_kt_gate_epoch_seconds, descriptor_text,
    open_mobile_relay_directory_authority,
};
use crate::domain::mobile_relay::key_transparency::config::{
    load_config_with_runtime_secret_context_for_operation,
    mobile_relay_e2ee_secret_store_authorization_batch_operation_count,
};
use crate::domain::mobile_relay::key_transparency::contract::{
    SECURE_MESH_KT_GOSSIP_CONTROL_TYPE, SecureMeshKtGossipControlMessage,
};
use crate::domain::mobile_relay::pairwise_session::{
    PairwiseDirectoryGate, mobile_relay_pairwise_operation_with_runtime_secret_context,
    open_mobile_relay_payload_with_pairwise_operation_and_gate,
    seal_mobile_relay_payload_with_pairwise_operation_and_gate,
};
use crate::domain::mobile_relay::relay_operations::secure_envelope_param;
use crate::domain::mobile_relay::support::ensure_only_known_params;
use anyhow::{Result, anyhow, ensure};
use serde_json::{Value, json};

pub(super) fn key_transparency_gossip(params: &Value) -> Result<Value> {
    ensure_only_known_params(
        params,
        &[
            "operation",
            "gossip",
            "envelope",
            "allowInteraction",
            "secretOverrideTransport",
            "secretOverrides",
        ],
        "secure mesh KT gossip",
    )?;
    let operation = params
        .get("operation")
        .and_then(Value::as_str)
        .ok_or_else(|| anyhow!("secure mesh KT gossip operation is required"))?;
    let (config, mut secret_context) = load_config_with_runtime_secret_context_for_operation(
        params,
        "Secure Mesh KT gossip authorization batch",
        mobile_relay_e2ee_secret_store_authorization_batch_operation_count().saturating_add(4),
    )?;
    let local_endpoint_id = descriptor_text(
        config
            .get("mobileRelayE2ee")
            .ok_or_else(|| anyhow!("secure mesh KT gossip local endpoint is missing"))?,
        "endpointId",
    )?;
    let now_epoch_seconds = current_secure_mesh_kt_gate_epoch_seconds()?;
    let mut pairwise_operation = mobile_relay_pairwise_operation_with_runtime_secret_context(
        &config,
        "Secure Mesh KT gossip authorization batch",
        4,
        &mut secret_context,
    )?;
    match operation {
        "seal" => {
            let gossip: SecureMeshKtGossipPayload = serde_json::from_value(
                params
                    .get("gossip")
                    .filter(|value| value.is_object())
                    .cloned()
                    .ok_or_else(|| anyhow!("secure mesh KT gossip payload is required"))?,
            )
            .map_err(|_| anyhow!("secure mesh KT gossip payload is invalid"))?;
            let mut authority = open_mobile_relay_directory_authority(&config, &local_endpoint_id)?;
            let checkpoint = authority.validate_outgoing_gossip(&gossip, now_epoch_seconds)?;
            let control = SecureMeshKtGossipControlMessage {
                message_type: SECURE_MESH_KT_GOSSIP_CONTROL_TYPE.to_string(),
                gossip,
            };
            let envelope = seal_mobile_relay_payload_with_pairwise_operation_and_gate(
                &config,
                &secret_context.material,
                crate::core::secure_mesh_crypto::SecureMeshPayloadKind::ServiceAction,
                &serde_json::to_value(control)?,
                &mut pairwise_operation,
                PairwiseDirectoryGate::KtGossipControl,
            )?;
            Ok(json!({
                "ok": true,
                "operation": "seal",
                "envelope": envelope,
                "treeSize": checkpoint.tree_size,
                "bodyRedacted": true
            }))
        }
        "open" => {
            let envelope = secure_envelope_param(params)
                .ok_or_else(|| anyhow!("secure mesh KT gossip encrypted envelope is required"))?;
            let opened = open_mobile_relay_payload_with_pairwise_operation_and_gate(
                &config,
                &secret_context.material,
                &envelope,
                crate::core::secure_mesh_crypto::SecureMeshPayloadKind::ServiceAction,
                &mut pairwise_operation,
                PairwiseDirectoryGate::KtGossipControl,
            )?;
            let control: SecureMeshKtGossipControlMessage = serde_json::from_slice(&opened)
                .map_err(|_| anyhow!("secure mesh KT gossip control payload is invalid"))?;
            ensure!(
                control.message_type == SECURE_MESH_KT_GOSSIP_CONTROL_TYPE,
                "secure mesh KT gossip control type is invalid"
            );
            let mut authority = open_mobile_relay_directory_authority(&config, &local_endpoint_id)?;
            let checkpoint = authority.observe_gossip(&control.gossip, now_epoch_seconds)?;
            Ok(json!({
                "ok": true,
                "operation": "open",
                "treeSize": checkpoint.tree_size,
                "bodyRedacted": true
            }))
        }
        _ => Err(anyhow!("secure mesh KT gossip operation is unsupported")),
    }
}
