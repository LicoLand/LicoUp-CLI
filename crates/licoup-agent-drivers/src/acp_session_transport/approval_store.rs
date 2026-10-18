use super::errors::ProtocolFailure;
use serde_json::{Value, json};
use time::OffsetDateTime;
use uuid::Uuid;

/// Resolve one adapter-neutral, process-local approval route. The canonical
/// interaction registry owns identity, capacity and one-shot consumption.
pub fn resolve_interaction_approval(token: &str, allow: bool) -> Result<Value, &'static str> {
    licoup_foundation::platform::native_agent_interaction::resolve(token, json!({"allow": allow}))
}

pub fn register_park_and_inbox(
    token: &str,
    session_id: &str,
    turn_id: &str,
    requester_agent_id: &str,
    request_id: &Value,
    display_summary: &str,
    option_id: Option<&str>,
    requested_tools: &[String],
    decision_tx: std::sync::mpsc::SyncSender<bool>,
) -> Result<(), ProtocolFailure> {
    let session_id = session_id.to_string();
    if session_id.trim().is_empty() || turn_id.trim().is_empty() {
        return Err(ProtocolFailure::user_interaction(
            "session/request_permission",
            None,
            Some(turn_id),
        ));
    }
    let route = licoup_foundation::platform::native_agent_interaction::park_with_token(
        token.to_string(),
        licoup_foundation::platform::native_agent_interaction::InteractionRequest {
            adapter_id: requester_agent_id.to_string(),
            session_id: session_id.clone(),
            turn_id: turn_id.to_string(),
            request_id: request_id.clone(),
            method: "session/request_permission".to_string(),
            summary: display_summary.to_string(),
            options: requested_tools.to_vec(),
            response_shape:
                licoup_foundation::platform::native_agent_interaction::ResponseShape::Approval,
        },
    )
    .map_err(|_| {
        ProtocolFailure::new(
            "approval_park_unavailable",
            "Approval park state is unavailable.",
            "server/request",
        )
    })?;
    std::thread::spawn(move || {
        if let Ok(response) = route.response_rx.recv() {
            let _ = decision_tx.send(
                response
                    .get("allow")
                    .and_then(Value::as_bool)
                    .unwrap_or(false),
            );
        }
    });
    let _ = option_id;
    // The native interaction remains pending until the user responds or the
    // live transport settles it. This far-future inbox timestamp is only a
    // schema field; it is not a route expiry or a turn timeout.
    let expires_at = (OffsetDateTime::now_utc() + time::Duration::days(36500))
        .format(&time::format_description::well_known::Rfc3339)
        .unwrap_or_else(|_| "2099-01-01T00:00:00Z".to_string());
    let nonce = Uuid::new_v4().to_string();
    let pending_operation_id = format!("{requester_agent_id}-park-{token}");
    let register = super::approval_port::evaluate_request(&json!({
        "pendingOperationId": pending_operation_id,
        "requesterAgentId": requester_agent_id,
        "targetClientId": "local-desktop",
        "originEndpointId": "local-desktop",
        "displaySummary": display_summary,
        "policyReason": "native session/request_permission",
        "adapterCallbackTokenRef": token,
        "adapterStyle": "callback",
        "expiresAt": expires_at,
        "responseNonce": nonce,
        "trustedEndpointIds": ["local-desktop"],
        "requestedTools": requested_tools,
        "riskLevel": "local_effect",
    }));
    if register.is_err() {
        // Inbox registration failure must not leave an unresolvable park.
        licoup_foundation::platform::native_agent_interaction::abandon(token);
        return Err(ProtocolFailure::new(
            "approval_inbox_register_failed",
            "The agent could not register an in-process one-shot approval handle.",
            "server/request",
        ));
    }
    let _ = super::approval_port::evaluate_fanout(&json!({
        "pendingOperationId": pending_operation_id,
    }));
    licoup_foundation::platform::turn_event_emit::emit_turn_event(
        "agent.approval.needed",
        &session_id,
        turn_id,
        json!({
            "agentId": requester_agent_id,
            "adapterCallbackTokenRef": token,
            "pendingOperationId": pending_operation_id,
            "displaySummary": display_summary,
            "requestedTools": requested_tools,
            "adapterStyle": "callback",
            "responseNonce": nonce,
            "expiresAt": expires_at,
            "originEndpointId": "local-desktop",
            "trustedEndpointIds": ["local-desktop"],
        }),
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn insert_test_park(token: &str) -> std::sync::mpsc::Receiver<Value> {
        let route = licoup_foundation::platform::native_agent_interaction::park_with_token(
            token.to_string(),
            licoup_foundation::platform::native_agent_interaction::InteractionRequest {
                adapter_id: "hermes".to_string(),
                session_id: "session".to_string(),
                turn_id: "turn".to_string(),
                request_id: json!("request"),
                method: "session/request_permission".to_string(),
                summary: "summary".to_string(),
                options: vec!["allow-once".to_string()],
                response_shape:
                    licoup_foundation::platform::native_agent_interaction::ResponseShape::Approval,
            },
        )
        .unwrap();
        route.response_rx
    }

    #[test]
    fn approval_ticket_is_in_process_and_consumed_once() {
        let token = format!("test-one-shot-{}", Uuid::new_v4());
        let decision_rx = insert_test_park(&token);

        let resolved = resolve_interaction_approval(&token, true).unwrap();

        assert_eq!(resolved["signal"], "in-process-one-shot");
        assert_eq!(
            decision_rx.recv_timeout(Duration::from_secs(1)).unwrap()["allow"],
            true
        );
        assert_eq!(
            resolve_interaction_approval(&token, true),
            Err("native_interaction_route_consumed")
        );
    }

    #[test]
    fn arbitrary_cross_process_style_token_never_authorizes() {
        let token = format!("forged-file-token-{}", Uuid::new_v4());
        assert_eq!(
            resolve_interaction_approval(&token, true),
            Err("native_interaction_route_missing")
        );
    }
}
