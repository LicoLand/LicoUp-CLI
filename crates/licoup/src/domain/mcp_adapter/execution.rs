use super::approval::{parse_scope, require_direct_confirmation, require_direct_origin};
use super::plan::McpApprovalPlanStore;
use super::sse::decode_sse_messages;
use crate::core::mcp::{
    DEFAULT_MAX_MESSAGE_BYTES, McpExternalTransferGate, McpMessage, McpRequestId,
    McpTransferDirection, OUTBOUND_TRANSFER_PROTOCOL_REVISION, decode_http_body,
};
use anyhow::{Result, ensure};
use serde_json::{Value, json};

pub struct McpHttpTransportResponse {
    pub status: u16,
    pub content_type: Option<String>,
    pub session_id: Option<String>,
    pub body: Vec<u8>,
}

pub fn preview_http_transfer(params: &Value, plans: &impl McpApprovalPlanStore) -> Result<Value> {
    require_direct_origin(params)?;
    let scope = parse_scope(params)?;
    let plan_id = plans.stage(&scope.approval_digest)?;
    Ok(json!({
        "ok": true,
        "schemaVersion": "licoup.mcp-transfer-preview.v1",
        "direction": direction_name(scope.direction),
        "destination": scope.destination,
        "purpose": scope.purpose,
        "protocolVersion": OUTBOUND_TRANSFER_PROTOCOL_REVISION,
        "sessionBound": scope.session_id.is_some(),
        "messageBytes": scope.body.len(),
        "planId": plan_id,
        "approvalDigest": scope.approval_digest,
        "requiresDirectUserConfirmation": true,
        "oneShot": true
    }))
}

pub fn execute_http_transfer<F>(
    params: &Value,
    plans: &impl McpApprovalPlanStore,
    exchange: F,
) -> Result<Value>
where
    F: FnOnce(
        &crate::core::mcp::McpTransferPacket,
        Option<&str>,
    ) -> Result<McpHttpTransportResponse>,
{
    let scope = parse_scope(params)?;
    require_direct_confirmation(params, &scope)?;
    let plan_id = plan_id(params)?;
    let planned_digest = plans.claim(plan_id)?;
    ensure!(
        planned_digest == scope.approval_digest,
        "mcp_transfer_approval_scope_mismatch"
    );
    execute_approved_scope(scope, exchange)
}

fn execute_approved_scope<F>(
    scope: super::approval::ApprovedTransferScope,
    exchange: F,
) -> Result<Value>
where
    F: FnOnce(
        &crate::core::mcp::McpTransferPacket,
        Option<&str>,
    ) -> Result<McpHttpTransportResponse>,
{
    let gate = McpExternalTransferGate::default();
    gate.record_direct_user_approval(
        &scope.approval_digest,
        scope.direction,
        &scope.destination,
        &scope.purpose,
        &scope.message,
    )?;
    let packet = match scope.direction {
        McpTransferDirection::Request => gate.send_request_once(
            &scope.approval_digest,
            &scope.destination,
            &scope.purpose,
            &scope.message,
        )?,
        McpTransferDirection::Response => gate.forward_response_once(
            &scope.approval_digest,
            &scope.destination,
            &scope.purpose,
            &scope.message,
        )?,
    };
    let response = exchange(&packet, scope.session_id.as_deref())?;
    project_response(&scope.message, response)
}

fn plan_id(params: &Value) -> Result<&str> {
    params
        .get("planId")
        .and_then(Value::as_str)
        .ok_or_else(|| anyhow::anyhow!("mcp_transfer_plan_id_invalid"))
}

fn project_response(sent: &McpMessage, response: McpHttpTransportResponse) -> Result<Value> {
    validate_session_id(response.session_id.as_deref())?;
    match sent {
        McpMessage::Notification { .. } | McpMessage::Response { .. } => {
            ensure!(
                response.status == 202 && response.body.is_empty(),
                "mcp_http_acceptance_invalid"
            );
            Ok(json!({
                "ok": true,
                "schemaVersion": "licoup.mcp-transfer-result.v1",
                "accepted": true,
                "sessionId": response.session_id
            }))
        }
        McpMessage::Request { id, .. } => {
            ensure!(response.status == 200, "mcp_http_response_status_invalid");
            let content_type = normalized_content_type(response.content_type.as_deref())?;
            let messages = match content_type {
                "application/json" => {
                    vec![decode_http_body(&response.body, DEFAULT_MAX_MESSAGE_BYTES)?]
                }
                "text/event-stream" => decode_sse_messages(&response.body)?,
                _ => unreachable!(),
            };
            let final_response = messages
                .iter()
                .rev()
                .find(|message| matches_response_id(message, id))
                .ok_or_else(|| anyhow::anyhow!("mcp_http_response_id_missing"))?;
            Ok(json!({
                "ok": true,
                "schemaVersion": "licoup.mcp-transfer-result.v1",
                "accepted": true,
                "sessionId": response.session_id,
                "messageCount": messages.len(),
                "messages": messages.iter().map(McpMessage::to_value).collect::<Vec<_>>(),
                "response": final_response.to_value()
            }))
        }
    }
}

fn normalized_content_type(content_type: Option<&str>) -> Result<&str> {
    let content_type = content_type
        .and_then(|value| value.split(';').next())
        .map(str::trim)
        .ok_or_else(|| anyhow::anyhow!("mcp_http_content_type_invalid"))?;
    ensure!(
        matches!(content_type, "application/json" | "text/event-stream"),
        "mcp_http_content_type_invalid"
    );
    Ok(content_type)
}

fn matches_response_id(message: &McpMessage, request_id: &McpRequestId) -> bool {
    matches!(
        message,
        McpMessage::Response {
            id: Some(response_id),
            ..
        } if response_id == request_id
    )
}

fn validate_session_id(session_id: Option<&str>) -> Result<()> {
    let Some(session_id) = session_id else {
        return Ok(());
    };
    ensure!(
        !session_id.is_empty()
            && session_id.len() <= 1024
            && session_id.bytes().all(|byte| (0x21..=0x7e).contains(&byte)),
        "mcp_session_id_invalid"
    );
    Ok(())
}

fn direction_name(direction: McpTransferDirection) -> &'static str {
    match direction {
        McpTransferDirection::Request => "request",
        McpTransferDirection::Response => "response",
    }
}
