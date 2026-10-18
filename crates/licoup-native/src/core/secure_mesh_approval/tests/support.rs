use serde_json::{Value, json};

pub(super) fn base_request() -> Value {
    json!({
        "pendingOperationId": "op-1",
        "requesterAgentId": "openclaw",
        "targetClientId": "desktop-a",
        "originEndpointId": "endpoint-origin",
        "riskLevel": "local_effect",
        "displaySummary": "Allow file read in project workspace",
        "policyReason": "ACP session/request_permission",
        "adapterCallbackTokenRef": "cb-ref-1",
        "adapterStyle": "callback",
        "expiresAt": "2099-01-01T00:00:00Z",
        "responseNonce": "nonce-1",
        "requestedTools": ["fs.read"],
        "trustedEndpointIds": ["endpoint-origin", "endpoint-phone"],
    })
}
