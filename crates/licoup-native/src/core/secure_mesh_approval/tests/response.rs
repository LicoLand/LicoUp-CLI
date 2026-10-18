use super::super::{
    evaluate_approval_request_json, list_approval_inbox_json, resolve_approval_response_json,
};
use serde_json::json;

#[test]
fn first_valid_response_wins_and_duplicate_is_rejected() {
    let request = json!({
        "pendingOperationId": "op-cas-1",
        "requesterAgentId": "hermes",
        "targetClientId": "desktop-a",
        "originEndpointId": "endpoint-origin",
        "riskLevel": "high_risk",
        "displaySummary": "Approve terminal command",
        "adapterCallbackTokenRef": "cb-ref-2",
        "adapterStyle": "callback",
        "expiresAt": "2099-01-01T00:00:00Z",
        "responseNonce": "nonce-cas",
        "trustedEndpointIds": ["endpoint-origin", "endpoint-phone"],
    });
    let _ = evaluate_approval_request_json(&request).unwrap();
    let first = resolve_approval_response_json(&json!({
        "pendingOperationId": "op-cas-1",
        "decision": "allow",
        "respondingEndpointId": "endpoint-phone",
        "responseNonce": "nonce-cas",
    }))
    .unwrap();
    assert_eq!(first["ok"], true);
    assert_eq!(first["decision"], "allow");

    let second = resolve_approval_response_json(&json!({
        "pendingOperationId": "op-cas-1",
        "decision": "deny",
        "respondingEndpointId": "endpoint-origin",
        "responseNonce": "nonce-cas",
    }))
    .unwrap();
    assert_eq!(second["ok"], false);
    assert_eq!(second["code"], "secure_mesh_approval_already_resolved");
    assert_eq!(second["decision"], "allow");
    assert_eq!(second["duplicateRejected"], true);
}

#[test]
fn expired_and_untrusted_endpoint_fail_closed() {
    let request = json!({
        "pendingOperationId": "op-exp-1",
        "requesterAgentId": "openclaw",
        "targetClientId": "desktop-a",
        "originEndpointId": "endpoint-origin",
        "riskLevel": "safe_write",
        "displaySummary": "Approve edit",
        "adapterCallbackTokenRef": "cb-ref-3",
        "adapterStyle": "callback",
        "expiresAt": "2000-01-01T00:00:00Z",
        "responseNonce": "nonce-exp",
        "trustedEndpointIds": ["endpoint-origin"],
    });
    assert!(evaluate_approval_request_json(&request).is_err());

    let live = json!({
        "pendingOperationId": "op-trust-1",
        "requesterAgentId": "openclaw",
        "targetClientId": "desktop-a",
        "originEndpointId": "endpoint-origin",
        "riskLevel": "safe_write",
        "displaySummary": "Approve edit",
        "adapterCallbackTokenRef": "cb-ref-4",
        "adapterStyle": "callback",
        "expiresAt": "2099-01-01T00:00:00Z",
        "responseNonce": "nonce-trust",
        "trustedEndpointIds": ["endpoint-origin"],
    });
    let _ = evaluate_approval_request_json(&live).unwrap();
    assert!(
        resolve_approval_response_json(&json!({
            "pendingOperationId": "op-trust-1",
            "decision": "allow",
            "respondingEndpointId": "endpoint-evil",
            "responseNonce": "nonce-trust",
        }))
        .is_err()
    );
}

#[test]
fn multi_client_resolve_cas_converges_first_valid_response() {
    let request = json!({
        "pendingOperationId": "op-cas-multi-1",
        "requesterAgentId": "hermes",
        "targetClientId": "desktop-a",
        "originEndpointId": "endpoint-origin",
        "riskLevel": "high_risk",
        "displaySummary": "Approve remote effect",
        "adapterCallbackTokenRef": "cb-cas-multi",
        "adapterStyle": "callback",
        "expiresAt": "2099-01-01T00:00:00Z",
        "responseNonce": "nonce-cas-multi",
        "trustedEndpointIds": [
            "endpoint-origin",
            "endpoint-phone",
            "endpoint-tablet"
        ],
    });
    let _ = evaluate_approval_request_json(&request).unwrap();

    let phone_allow = resolve_approval_response_json(&json!({
        "pendingOperationId": "op-cas-multi-1",
        "decision": "allow",
        "respondingEndpointId": "endpoint-phone",
        "responseNonce": "nonce-cas-multi",
    }))
    .unwrap();
    assert_eq!(phone_allow["ok"], true);
    assert_eq!(phone_allow["decision"], "allow");
    assert_eq!(
        phone_allow["fanoutConvergence"]["firstValidResponseWins"],
        true
    );

    let tablet_deny = resolve_approval_response_json(&json!({
        "pendingOperationId": "op-cas-multi-1",
        "decision": "deny",
        "respondingEndpointId": "endpoint-tablet",
        "responseNonce": "nonce-cas-multi",
    }))
    .unwrap();
    assert_eq!(tablet_deny["ok"], false);
    assert_eq!(tablet_deny["duplicateRejected"], true);
    assert_eq!(tablet_deny["decision"], "allow");
    assert_eq!(tablet_deny["plaintextRelayBlocked"], true);

    let origin_retry = resolve_approval_response_json(&json!({
        "pendingOperationId": "op-cas-multi-1",
        "decision": "allow",
        "respondingEndpointId": "endpoint-origin",
        "responseNonce": "nonce-cas-multi",
    }))
    .unwrap();
    assert_eq!(origin_retry["ok"], false);
    assert_eq!(origin_retry["duplicateRejected"], true);
    assert_eq!(origin_retry["decision"], "allow");

    let inbox = list_approval_inbox_json(&json!({ "includeResolved": true })).unwrap();
    let serialized = serde_json::to_string(&inbox).unwrap();
    assert!(!serialized.contains("toolArguments"));
    assert!(!serialized.contains("plaintextDetail"));
    assert!(serialized.contains("plaintextRelayBlocked"));
}
