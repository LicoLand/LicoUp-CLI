use super::*;
use crate::core::mcp::transfer::{TransferApproval, encode_bounded_transfer_body, message_digest};
use serde_json::json;
use std::time::{Duration, Instant};

#[test]
fn response_forwarding_requires_matching_one_shot_user_approval() {
    let gate = McpExternalTransferGate::default();
    let response = McpMessage::success("request-1", object(json!({"content": []})));

    assert!(
        gate.forward_response_once("action-1", "paired-device", "return tool result", &response)
            .is_err()
    );
    gate.record_direct_user_approval(
        "action-1",
        McpTransferDirection::Response,
        "paired-device",
        "return tool result",
        &response,
    )
    .unwrap();
    assert!(
        gate.forward_response_once(
            "action-1",
            "different-device",
            "return tool result",
            &response,
        )
        .is_err()
    );
    assert_eq!(gate.pending_approval_count(), 0);
    assert!(
        gate.forward_response_once("action-1", "paired-device", "return tool result", &response)
            .is_err()
    );

    gate.record_direct_user_approval(
        "action-2",
        McpTransferDirection::Response,
        "paired-device",
        "return tool result",
        &response,
    )
    .unwrap();
    let packet = gate
        .forward_response_once("action-2", "paired-device", "return tool result", &response)
        .unwrap();
    assert_eq!(packet.destination(), "paired-device");
    assert_eq!(packet.purpose(), "return tool result");
    assert!(!packet.body().is_empty());
    assert_eq!(gate.pending_approval_count(), 0);
}

#[test]
fn cancelled_approval_cannot_forward() {
    let gate = McpExternalTransferGate::default();
    let response = McpMessage::success(3, Map::new());
    gate.record_direct_user_approval(
        "action",
        McpTransferDirection::Response,
        "destination",
        "return result",
        &response,
    )
    .unwrap();
    assert!(gate.cancel("action").unwrap());
    assert!(
        gate.forward_response_once("action", "destination", "return result", &response)
            .is_err()
    );
}

#[test]
fn outbound_request_approval_binds_direction_destination_and_exact_body() {
    let gate = McpExternalTransferGate::default();
    let request = McpMessage::request(
        "request-7",
        "resources/read",
        Some(object(json!({"uri": "local://approved-item"}))),
    )
    .unwrap();
    gate.record_direct_user_approval(
        "send-7",
        McpTransferDirection::Request,
        "explicit-endpoint",
        "read approved resource",
        &request,
    )
    .unwrap();

    let changed = McpMessage::request(
        "request-7",
        "resources/read",
        Some(object(json!({"uri": "local://different-item"}))),
    )
    .unwrap();
    assert!(
        gate.send_request_once(
            "send-7",
            "explicit-endpoint",
            "read approved resource",
            &changed,
        )
        .is_err()
    );
    assert_eq!(gate.pending_approval_count(), 0);
    assert!(
        gate.send_request_once(
            "send-7",
            "explicit-endpoint",
            "read approved resource",
            &request,
        )
        .is_err()
    );

    gate.record_direct_user_approval(
        "send-purpose",
        McpTransferDirection::Request,
        "explicit-endpoint",
        "read approved resource",
        &request,
    )
    .unwrap();
    assert!(
        gate.send_request_once(
            "send-purpose",
            "explicit-endpoint",
            "different purpose",
            &request,
        )
        .is_err()
    );
    assert_eq!(gate.pending_approval_count(), 0);

    gate.record_direct_user_approval(
        "send-direction",
        McpTransferDirection::Request,
        "explicit-endpoint",
        "read approved resource",
        &request,
    )
    .unwrap();
    assert!(
        gate.forward_response_once(
            "send-direction",
            "explicit-endpoint",
            "read approved resource",
            &McpMessage::success("request-7", Map::new()),
        )
        .is_err()
    );
    assert_eq!(gate.pending_approval_count(), 0);

    gate.record_direct_user_approval(
        "send-correct",
        McpTransferDirection::Request,
        "explicit-endpoint",
        "read approved resource",
        &request,
    )
    .unwrap();
    let packet = gate
        .send_request_once(
            "send-correct",
            "explicit-endpoint",
            "read approved resource",
            &request,
        )
        .unwrap();
    assert_eq!(packet.direction(), McpTransferDirection::Request);
    assert_eq!(packet.destination(), "explicit-endpoint");
    assert_eq!(gate.pending_approval_count(), 0);
}

#[test]
fn expired_or_invalid_ttl_approval_fails_closed() {
    let gate = McpExternalTransferGate::default();
    let request = McpMessage::request(1, "ping", None).unwrap();
    assert!(
        gate.record_direct_user_approval_with_ttl(
            "invalid",
            McpTransferDirection::Request,
            "destination",
            "health check",
            &request,
            Duration::ZERO,
        )
        .is_err()
    );
    assert!(
        gate.record_direct_user_approval_until(
            "expired".to_owned(),
            McpTransferDirection::Request,
            "destination".to_owned(),
            "health check".to_owned(),
            &request,
            Instant::now(),
        )
        .is_err()
    );
    gate.approvals.lock().unwrap().insert(
        "expired-stored".to_owned(),
        TransferApproval {
            direction: McpTransferDirection::Request,
            destination: "destination".to_owned(),
            purpose: "health check".to_owned(),
            body_sha256: message_digest(&encode_bounded_transfer_body(&request).unwrap()),
            expires_at: Instant::now(),
        },
    );
    assert!(
        gate.send_request_once("expired-stored", "destination", "health check", &request)
            .is_err()
    );
    assert_eq!(gate.pending_approval_count(), 0);
}
