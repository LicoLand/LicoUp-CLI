use super::*;
use crate::core::mcp::OUTBOUND_TRANSFER_PROTOCOL_REVISION;
use serde_json::json;
use std::collections::HashMap;
use std::sync::Mutex;

#[derive(Default)]
struct MemoryPlanStore {
    plans: Mutex<HashMap<String, String>>,
}

impl McpApprovalPlanStore for MemoryPlanStore {
    fn stage(&self, approval_digest: &str) -> anyhow::Result<String> {
        let mut plans = self.plans.lock().unwrap();
        let plan_id = format!("plan-{}", plans.len() + 1);
        plans.insert(plan_id.clone(), approval_digest.to_owned());
        Ok(plan_id)
    }

    fn claim(&self, plan_id: &str) -> anyhow::Result<String> {
        self.plans
            .lock()
            .unwrap()
            .remove(plan_id)
            .ok_or_else(|| anyhow::anyhow!("mcp_transfer_plan_missing_or_used"))
    }
}

fn request_params() -> serde_json::Value {
    json!({
        "direction": "request",
        "destination": "https://example.invalid/mcp",
        "purpose": "invoke an explicitly selected tool",
        "protocolVersion": OUTBOUND_TRANSFER_PROTOCOL_REVISION,
        "messageJson": r#"{"jsonrpc":"2.0","id":"req-1","method":"tools/call","params":{"name":"selected","arguments":{}}}"#,
        "requestOrigin": "direct-user"
    })
}

#[test]
fn preview_binds_exact_scope_without_transferring() {
    let preview = preview_http_transfer(&request_params(), &MemoryPlanStore::default()).unwrap();
    assert_eq!(preview["requiresDirectUserConfirmation"], true);
    assert_eq!(preview["oneShot"], true);
    assert_eq!(preview["approvalDigest"].as_str().unwrap().len(), 64);
}

#[test]
fn execute_requires_direct_confirmation_and_exact_preview_digest() {
    let plans = MemoryPlanStore::default();
    let params = request_params();
    let preview = preview_http_transfer(&params, &plans).unwrap();
    let mut confirmed = params;
    confirmed["planId"] = preview["planId"].clone();
    let error = execute_http_transfer(&confirmed, &plans, |_, _| unreachable!()).unwrap_err();
    assert!(error.to_string().contains("confirmation_required"));

    confirmed["confirmed"] = json!(true);
    confirmed["approvalDigest"] = preview["approvalDigest"].clone();
    confirmed["purpose"] = json!("changed after approval");
    assert!(execute_http_transfer(&confirmed, &plans, |_, _| unreachable!()).is_err());
}

#[test]
fn approved_request_accepts_bounded_json_or_sse_response() {
    for (content_type, body) in [
        (
            "application/json",
            r#"{"jsonrpc":"2.0","id":"req-1","result":{"content":[]}}"#,
        ),
        (
            "text/event-stream",
            "event: message\ndata: {\"jsonrpc\":\"2.0\",\"method\":\"notifications/progress\"}\n\nevent: message\ndata: {\"jsonrpc\":\"2.0\",\"id\":\"req-1\",\"result\":{\"content\":[]}}\n\n",
        ),
    ] {
        let plans = MemoryPlanStore::default();
        let mut params = request_params();
        let preview = preview_http_transfer(&params, &plans).unwrap();
        params["planId"] = preview["planId"].clone();
        params["confirmed"] = json!(true);
        params["approvalDigest"] = preview["approvalDigest"].clone();
        let result = execute_http_transfer(&params, &plans, |packet, session_id| {
            assert!(!packet.body().is_empty());
            assert!(session_id.is_none());
            Ok(McpHttpTransportResponse {
                status: 200,
                content_type: Some(content_type.to_owned()),
                session_id: None,
                body: body.as_bytes().to_vec(),
            })
        })
        .unwrap();
        assert_eq!(result["ok"], true);
        assert!(result["messageCount"].as_u64().unwrap() >= 1);
    }
}

#[test]
fn forwarded_response_requires_empty_202_acceptance() {
    let plans = MemoryPlanStore::default();
    let mut params = json!({
        "direction": "response",
        "destination": "https://example.invalid/mcp",
        "purpose": "return the approved MCP result",
        "protocolVersion": OUTBOUND_TRANSFER_PROTOCOL_REVISION,
        "messageJson": r#"{"jsonrpc":"2.0","id":"req-1","result":{"content":[]}}"#,
        "requestOrigin": "direct-user"
    });
    let preview = preview_http_transfer(&params, &plans).unwrap();
    params["planId"] = preview["planId"].clone();
    params["confirmed"] = json!(true);
    params["approvalDigest"] = preview["approvalDigest"].clone();
    let result = execute_http_transfer(&params, &plans, |_, _| {
        Ok(McpHttpTransportResponse {
            status: 202,
            content_type: None,
            session_id: None,
            body: Vec::new(),
        })
    })
    .unwrap();
    assert_eq!(result["accepted"], true);
}
