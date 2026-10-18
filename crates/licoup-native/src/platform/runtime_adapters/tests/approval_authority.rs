//! The approval authority this host answers the ACP session transport's port
//! with.
//!
//! The transport parks a `session/request_permission` interaction and asks the
//! port whether it may be raised and fanned out; the authority behind the
//! answer is this host's Secure Mesh approval ledger. These suites prove the
//! composed host answers that port with its own authority, and that a park
//! registers and resolves through it — an uncomposed host refuses instead,
//! which is the failure this composition exists to prevent.

use serde_json::json;

/// One valid approval payload, with a fresh pending operation id so repeated
/// runs never collide in the process-wide ledger.
fn approval_payload(pending_operation_id: &str) -> serde_json::Value {
    json!({
        "pendingOperationId": pending_operation_id,
        "requesterAgentId": "claude-code",
        "targetClientId": "local-desktop",
        "originEndpointId": "local-desktop",
        "displaySummary": "Approve one bounded local effect",
        "policyReason": "native session/request_permission",
        "adapterCallbackTokenRef": "callback-approval-authority",
        "adapterStyle": "callback",
        "expiresAt": "2099-01-01T00:00:00Z",
        "responseNonce": "nonce-approval-authority",
        "trustedEndpointIds": ["local-desktop"],
        "requestedTools": ["fs.read"],
        "riskLevel": "local_effect",
    })
}

#[test]
fn the_composed_host_answers_the_approval_port_with_its_own_authority() {
    super::compose();
    let pending_operation_id = format!("approval-port-{}", uuid::Uuid::new_v4());
    let payload = approval_payload(&pending_operation_id);

    let raised =
        licoup_agent_drivers::acp_session_transport::approval_port::evaluate_request(&payload)
            .expect("the composed host answers the approval port");
    assert_eq!(raised["ok"], true);
    assert_eq!(raised["pendingOperationId"], json!(pending_operation_id));

    let fanout =
        licoup_agent_drivers::acp_session_transport::approval_port::evaluate_fanout(&json!({
            "pendingOperationId": pending_operation_id,
        }))
        .expect("the composed host answers the fanout the park registered");
    assert_eq!(fanout["ok"], true);
}

#[test]
fn an_invalid_approval_is_refused_by_the_authority_rather_than_by_a_missing_port() {
    super::compose();
    let error =
        licoup_agent_drivers::acp_session_transport::approval_port::evaluate_request(&json!({}))
            .expect_err("an approval with no pending operation is refused");

    assert_ne!(
        error, "approval_authority_unavailable",
        "the authority must answer the port instead of leaving it uninstalled"
    );
}

#[test]
fn a_park_registers_and_resolves_through_the_composed_authority() {
    super::compose();
    let token = format!("approval-port-park-{}", uuid::Uuid::new_v4());
    let (decision_tx, decision_rx) = std::sync::mpsc::sync_channel(1);

    licoup_agent_drivers::acp_session_transport::register_park_and_inbox(
        &token,
        "session-approval-authority",
        "turn-approval-authority",
        "claude-code",
        &json!({ "request_id": "request-approval-authority" }),
        "Approve one bounded local effect",
        None,
        &["fs.read".to_owned()],
        decision_tx,
    )
    .expect("a composed host registers the park its authority raised");

    let resolved =
        licoup_agent_drivers::acp_session_transport::resolve_interaction_approval(&token, true)
            .expect("the registered park answers once");
    assert_eq!(resolved["signal"], "in-process-one-shot");
    assert!(
        decision_rx
            .recv_timeout(std::time::Duration::from_secs(5))
            .expect("the parked decision reaches the waiting transport"),
        "the decision the transport parked on carries the approval it was given"
    );
}
