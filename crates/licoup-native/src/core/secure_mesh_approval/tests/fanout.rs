use super::super::{evaluate_approval_fanout_json, evaluate_approval_request_json};
use serde_json::json;

#[test]
fn approval_fanout_plan_never_exposes_plaintext_operation_detail() {
    let request = json!({
        "pendingOperationId": "op-fanout-plain-1",
        "requesterAgentId": "hermes",
        "targetClientId": "desktop-a",
        "originEndpointId": "endpoint-origin",
        "riskLevel": "local_effect",
        "displaySummary": "Allow hermes tool",
        "adapterCallbackTokenRef": "cb-fanout-1",
        "adapterStyle": "callback",
        "expiresAt": "2099-01-01T00:00:00Z",
        "responseNonce": "nonce-fanout",
        "requestedTools": ["fs.read"],
        "trustedEndpointIds": ["endpoint-origin", "endpoint-phone", "endpoint-tablet"],
    });
    let registered = evaluate_approval_request_json(&request).unwrap();
    assert_eq!(registered["fanout"]["plaintextRelayBlocked"], true);
    let fanout = evaluate_approval_fanout_json(&json!({
        "pendingOperationId": "op-fanout-plain-1",
    }))
    .unwrap();
    assert_eq!(fanout["ok"], true);
    assert_eq!(fanout["fanoutRequired"], true);
    assert_eq!(fanout["plaintextRelayBlocked"], true);
    assert_eq!(fanout["payloadClass"], "permission_payload");
    assert_eq!(fanout["sealPerTrustedEndpoint"], true);
    assert_eq!(fanout["trustedEndpointCount"], 3);
    let wire = serde_json::to_string(&fanout).unwrap();
    for canary in [
        "toolArguments",
        "plaintextDetail",
        "operationDetail",
        "prompt",
        "/secret",
        "Authorization:",
    ] {
        assert!(
            !wire.contains(canary),
            "fanout plan must not contain canary {canary}"
        );
    }
    // Hashes only — never raw endpoint identifiers on the fanout projection.
    assert!(wire.contains("trustedEndpointIdHashes"));
    assert!(!wire.contains("endpoint-phone"));
    assert!(!wire.contains("endpoint-tablet"));
}
