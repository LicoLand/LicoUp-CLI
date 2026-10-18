use super::SECURE_MESH_APPROVAL_REQUEST_PROTOCOL;
use super::model::PendingApproval;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use time::{OffsetDateTime, format_description::well_known::Rfc3339};

pub(super) fn now_rfc3339() -> String {
    OffsetDateTime::now_utc()
        .format(&Rfc3339)
        .unwrap_or_else(|_| "1970-01-01T00:00:00Z".to_string())
}

pub(super) fn is_expired(expires_at: &str, now: &str) -> bool {
    match (
        OffsetDateTime::parse(expires_at, &Rfc3339),
        OffsetDateTime::parse(now, &Rfc3339),
    ) {
        (Ok(expiry), Ok(current)) => current >= expiry,
        _ => true,
    }
}

pub(super) fn redact_hash(value: &str) -> String {
    let digest = Sha256::digest(value.as_bytes());
    format!(
        "sha256:{}",
        digest
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>()
    )
}

pub(super) fn projection(entry: &PendingApproval) -> Value {
    let status = if let Some(resolved) = &entry.resolved {
        json!({
            "state": "resolved",
            "decision": resolved.decision.as_str(),
            "respondingEndpointIdHash": redact_hash(&resolved.responding_endpoint_id),
            "resolvedAt": resolved.resolved_at,
        })
    } else if is_expired(&entry.expires_at, &now_rfc3339()) {
        json!({
            "state": "expired",
            "decision": Value::Null,
        })
    } else {
        json!({
            "state": "pending",
            "decision": Value::Null,
        })
    };
    json!({
        "protocolVersion": SECURE_MESH_APPROVAL_REQUEST_PROTOCOL,
        "pendingOperationId": entry.pending_operation_id,
        "requesterAgentId": entry.requester_agent_id,
        "targetClientId": entry.target_client_id,
        "originEndpointIdHash": redact_hash(&entry.origin_endpoint_id),
        "riskLevel": entry.risk_level,
        "displaySummary": entry.display_summary,
        "policyReason": entry.policy_reason,
        "adapterCallbackTokenRef": entry.adapter_callback_token_ref,
        "adapterStyle": entry.adapter_style,
        "expiresAt": entry.expires_at,
        "requestedTools": entry.requested_tools,
        "trustedEndpointCount": entry.trusted_endpoint_ids.len(),
        "createdAt": entry.created_at,
        "status": status,
        "detailCiphertextPresent": false,
        "plaintextRelayBlocked": true,
    })
}
