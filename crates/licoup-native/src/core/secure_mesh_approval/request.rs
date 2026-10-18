use super::input::{
    json_string_list, json_text, looks_like_plaintext_secret, optional_text, parse_adapter_style,
    parse_risk_level, require_text,
};
use super::ledger::ledger;
use super::model::PendingApproval;
use super::projection::{is_expired, now_rfc3339, projection, redact_hash};
use super::response::prune_expired;
use super::{
    MAX_PENDING, MAX_SUMMARY_BYTES, MAX_TEXT_BYTES, SECURE_MESH_APPROVAL_CONTENT_TYPE,
    SECURE_MESH_APPROVAL_REQUEST_PROTOCOL, SECURE_MESH_APPROVAL_RESPONSE_PROTOCOL,
    SECURE_MESH_APPROVAL_STATUS,
};
use anyhow::{Result, anyhow, bail, ensure};
use serde_json::{Value, json};
use time::{OffsetDateTime, format_description::well_known::Rfc3339};

/// Evaluate and register a remote approval request (fail-closed validation).
pub fn evaluate_approval_request_json(params: &Value) -> Result<Value> {
    let pending_operation_id = require_text(
        "pendingOperationId",
        &json_text(
            params,
            &["pendingOperationId", "pending_operation_id", "operationId"],
        )
        .ok_or_else(|| anyhow!("secure mesh approval pending operation id is required"))?,
        MAX_TEXT_BYTES,
    )?;
    let requester_agent_id = require_text(
        "requesterAgentId",
        &json_text(
            params,
            &["requesterAgentId", "requester_agent_id", "agentId"],
        )
        .ok_or_else(|| anyhow!("secure mesh approval requester agent id is required"))?,
        MAX_TEXT_BYTES,
    )?;
    let target_client_id = require_text(
        "targetClientId",
        &json_text(params, &["targetClientId", "target_client_id", "clientId"])
            .ok_or_else(|| anyhow!("secure mesh approval target client id is required"))?,
        MAX_TEXT_BYTES,
    )?;
    let origin_endpoint_id = require_text(
        "originEndpointId",
        &json_text(
            params,
            &["originEndpointId", "origin_endpoint_id", "sourceEndpointId"],
        )
        .ok_or_else(|| anyhow!("secure mesh approval origin endpoint id is required"))?,
        MAX_TEXT_BYTES,
    )?;
    let risk_level = parse_risk_level(
        &json_text(params, &["riskLevel", "risk_level", "minimumRiskClass"])
            .unwrap_or_else(|| "local_effect".to_string()),
    )?;
    let display_summary = require_text(
        "displaySummary",
        &json_text(params, &["displaySummary", "display_summary", "summary"])
            .ok_or_else(|| anyhow!("secure mesh approval display summary is required"))?,
        MAX_SUMMARY_BYTES,
    )?;
    ensure!(
        !looks_like_plaintext_secret(&display_summary),
        "secure mesh approval display summary must not contain plaintext secrets"
    );
    let policy_reason = optional_text(
        json_text(params, &["policyReason", "policy_reason", "reason"]).as_deref(),
        MAX_SUMMARY_BYTES,
    )?;
    let adapter_callback_token_ref = require_text(
        "adapterCallbackTokenRef",
        &json_text(
            params,
            &[
                "adapterCallbackTokenRef",
                "adapter_callback_token_ref",
                "callbackTokenRef",
            ],
        )
        .ok_or_else(|| anyhow!("secure mesh approval adapter callback token ref is required"))?,
        MAX_TEXT_BYTES,
    )?;
    let adapter_style = parse_adapter_style(
        &json_text(
            params,
            &["adapterStyle", "adapter_style", "permissionSelection"],
        )
        .unwrap_or_else(|| "callback".to_string()),
    )?;
    ensure!(
        adapter_style != "unavailable",
        "secure mesh approval adapter style is unavailable"
    );
    let expires_at = require_text(
        "expiresAt",
        &json_text(params, &["expiresAt", "expires_at"])
            .ok_or_else(|| anyhow!("secure mesh approval expiry is required"))?,
        MAX_TEXT_BYTES,
    )?;
    OffsetDateTime::parse(&expires_at, &Rfc3339)
        .map_err(|_| anyhow!("secure mesh approval expiry must be RFC3339"))?;
    let now = now_rfc3339();
    ensure!(
        !is_expired(&expires_at, &now),
        "secure mesh approval request is already expired"
    );
    let response_nonce = require_text(
        "responseNonce",
        &json_text(params, &["responseNonce", "response_nonce", "nonce"])
            .ok_or_else(|| anyhow!("secure mesh approval response nonce is required"))?,
        MAX_TEXT_BYTES,
    )?;
    let requested_tools =
        json_string_list(params, &["requestedTools", "requested_tools", "tools"])?;
    let trusted_endpoint_ids = json_string_list(
        params,
        &["trustedEndpointIds", "trusted_endpoint_ids", "endpoints"],
    )?;
    ensure!(
        !trusted_endpoint_ids.is_empty(),
        "secure mesh approval fanout requires at least one trusted endpoint"
    );
    ensure!(
        params.get("operationDetail").is_none()
            && params.get("toolArguments").is_none()
            && params.get("prompt").is_none()
            && params.get("plaintextDetail").is_none(),
        "secure mesh approval request must not carry plaintext operation detail"
    );

    let entry = PendingApproval {
        pending_operation_id: pending_operation_id.clone(),
        requester_agent_id,
        target_client_id,
        origin_endpoint_id,
        risk_level,
        display_summary,
        policy_reason,
        adapter_callback_token_ref,
        adapter_style,
        expires_at,
        response_nonce,
        requested_tools,
        trusted_endpoint_ids: trusted_endpoint_ids.clone(),
        created_at: now,
        resolved: None,
    };

    let mut guard = ledger()
        .lock()
        .map_err(|_| anyhow!("secure mesh approval ledger is poisoned"))?;
    if let Some(existing) = guard.pending.get(&pending_operation_id) {
        if existing.resolved.is_some() {
            bail!("secure mesh approval pending operation is already resolved");
        }
        bail!("secure mesh approval pending operation id already exists");
    }
    if guard.pending.len() >= MAX_PENDING {
        prune_expired(&mut guard, &now_rfc3339());
        ensure!(
            guard.pending.len() < MAX_PENDING,
            "secure mesh approval ledger is full"
        );
    }
    guard
        .pending
        .insert(pending_operation_id.clone(), entry.clone());

    Ok(json!({
        "ok": true,
        "approvalProtocolVersion": SECURE_MESH_APPROVAL_REQUEST_PROTOCOL,
        "approvalContentType": SECURE_MESH_APPROVAL_CONTENT_TYPE,
        "approvalStatus": SECURE_MESH_APPROVAL_STATUS,
        "pendingOperationId": pending_operation_id,
        "fanout": {
            "transport": "SecureEnvelopeDeliveryMailbox",
            "sealPerTrustedEndpoint": true,
            "trustedEndpointCount": trusted_endpoint_ids.len(),
            "trustedEndpointIdHashes": trusted_endpoint_ids
                .iter()
                .map(|id| Value::String(redact_hash(id)))
                .collect::<Vec<_>>(),
            "plaintextRelayBlocked": true,
        },
        "request": projection(&entry),
    }))
}

/// Plan encrypted fanout of a pending approval to trusted endpoints.
pub fn evaluate_approval_fanout_json(params: &Value) -> Result<Value> {
    let pending_operation_id = require_text(
        "pendingOperationId",
        &json_text(
            params,
            &["pendingOperationId", "pending_operation_id", "operationId"],
        )
        .ok_or_else(|| anyhow!("secure mesh approval pending operation id is required"))?,
        MAX_TEXT_BYTES,
    )?;
    let guard = ledger()
        .lock()
        .map_err(|_| anyhow!("secure mesh approval ledger is poisoned"))?;
    let entry = guard
        .pending
        .get(&pending_operation_id)
        .ok_or_else(|| anyhow!("secure mesh approval pending operation was not found"))?;
    if let Some(resolved) = &entry.resolved {
        return Ok(json!({
            "ok": true,
            "pendingOperationId": pending_operation_id,
            "fanoutRequired": false,
            "state": "resolved",
            "decision": resolved.decision.as_str(),
            "plaintextRelayBlocked": true,
        }));
    }
    let now = now_rfc3339();
    ensure!(
        !is_expired(&entry.expires_at, &now),
        "secure mesh approval request is expired"
    );
    Ok(json!({
        "ok": true,
        "pendingOperationId": pending_operation_id,
        "fanoutRequired": true,
        "sealPerTrustedEndpoint": true,
        "trustedEndpointCount": entry.trusted_endpoint_ids.len(),
        "trustedEndpointIdHashes": entry
            .trusted_endpoint_ids
            .iter()
            .map(|id| Value::String(redact_hash(id)))
            .collect::<Vec<_>>(),
        "requestProtocolVersion": SECURE_MESH_APPROVAL_REQUEST_PROTOCOL,
        "responseProtocolVersion": SECURE_MESH_APPROVAL_RESPONSE_PROTOCOL,
        "plaintextRelayBlocked": true,
        "payloadClass": "permission_payload",
    }))
}
