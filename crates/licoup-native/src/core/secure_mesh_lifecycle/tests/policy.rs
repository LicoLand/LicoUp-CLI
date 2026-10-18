use serde_json::json;

use super::super::{evaluate_service_action_json, schema::MAX_TTL_SECONDS};

#[test]
fn secure_mesh_lifecycle_service_actions_redact_private_ids_and_plaintext() {
    let fixtures = [
        json!({
            "actionKind": "message_ttl_set",
            "endpointId": "private-endpoint-canary",
            "conversationId": "private-conversation-canary",
            "messageId": "private-message-canary",
            "ttlSeconds": 60,
            "body": "plaintext-body-canary"
        }),
        json!({
            "actionKind": "message_delete",
            "endpointId": "private-endpoint-canary",
            "conversationId": "private-conversation-canary",
            "messageId": "private-message-canary",
            "userConfirmed": true,
            "body": "plaintext-body-canary"
        }),
        json!({
            "actionKind": "screenshot_detected",
            "endpointId": "private-endpoint-canary",
            "conversationId": "private-conversation-canary",
            "messageId": "private-message-canary",
            "body": "plaintext-body-canary"
        }),
        json!({
            "actionKind": "resend_request",
            "endpointId": "private-endpoint-canary",
            "conversationId": "private-conversation-canary",
            "missingMessageIds": ["private-missing-message-a", "private-missing-message-b"],
            "body": "plaintext-body-canary"
        }),
        json!({
            "actionKind": "typing_state",
            "endpointId": "private-endpoint-canary",
            "conversationId": "private-conversation-canary",
            "typingState": "started",
            "body": "plaintext-body-canary"
        }),
        json!({
            "actionKind": "read_receipt",
            "endpointId": "private-endpoint-canary",
            "conversationId": "private-conversation-canary",
            "readUpToMessageId": "private-read-message-canary",
            "body": "plaintext-body-canary"
        }),
        json!({
            "actionKind": "ack_purge",
            "endpointId": "private-endpoint-canary",
            "fileTransferId": "private-file-transfer-canary",
            "acknowledged": true,
            "transferComplete": true,
            "body": "plaintext-body-canary"
        }),
    ];

    for fixture in fixtures {
        let output = evaluate_service_action_json(&fixture).unwrap();
        assert_eq!(output["ok"], true);
        assert_eq!(output["requiresPairwiseOrMlsEnvelope"], true);
        assert_eq!(output["serverVisiblePlaintextAllowed"], false);
        assert_eq!(output["metadataRedacted"], true);
        assert_eq!(output["bodyRedacted"], true);
        let serialized = output.to_string();
        for forbidden in [
            "private-endpoint-canary",
            "private-conversation-canary",
            "private-message-canary",
            "private-missing-message-a",
            "private-missing-message-b",
            "private-read-message-canary",
            "private-file-transfer-canary",
            "plaintext-body-canary",
        ] {
            assert!(
                !serialized.contains(forbidden),
                "service action leaked {forbidden}"
            );
        }
    }
}

#[test]
fn secure_mesh_lifecycle_delete_requires_confirmation_and_ttl_bounds() {
    let delete = evaluate_service_action_json(&json!({
        "actionKind": "message_delete",
        "messageId": "msg-delete"
    }))
    .unwrap_err();
    assert!(
        delete
            .to_string()
            .contains("requires local user confirmation")
    );

    let ttl = evaluate_service_action_json(&json!({
        "actionKind": "message_ttl_set",
        "messageId": "msg-ttl",
        "ttlSeconds": MAX_TTL_SECONDS + 1
    }))
    .unwrap_err();
    assert!(ttl.to_string().contains("outside the supported range"));
}

#[test]
fn secure_mesh_lifecycle_typing_and_read_receipts_are_encrypted_service_actions() {
    let typing = evaluate_service_action_json(&json!({
        "actionKind": "typing_state",
        "endpointId": "typing-private-endpoint",
        "conversationId": "typing-private-conversation",
        "typingState": "started"
    }))
    .unwrap();
    assert_eq!(typing["requiresPairwiseOrMlsEnvelope"], true);
    assert_eq!(typing["serverVisiblePlaintextAllowed"], false);
    assert_eq!(typing["servicePolicy"]["typingNoticeRequired"], true);
    assert_eq!(typing["servicePolicy"]["typingStateEncrypted"], true);
    assert_eq!(typing["servicePolicy"]["typingContentIncluded"], false);

    let read_receipt = evaluate_service_action_json(&json!({
        "actionKind": "read_receipt",
        "endpointId": "receipt-private-endpoint",
        "conversationId": "receipt-private-conversation",
        "readUpToMessageId": "receipt-private-message"
    }))
    .unwrap();
    assert_eq!(read_receipt["requiresPairwiseOrMlsEnvelope"], true);
    assert_eq!(read_receipt["serverVisiblePlaintextAllowed"], false);
    assert_eq!(read_receipt["servicePolicy"]["readReceiptRequired"], true);
    assert_eq!(
        read_receipt["servicePolicy"]["readMessageIdsRedacted"],
        true
    );
    assert!(
        read_receipt["servicePolicy"]["readUpToMessageDigest"]
            .as_str()
            .unwrap_or_default()
            .starts_with("sha256:")
    );

    let serialized = json!([typing, read_receipt]).to_string();
    for forbidden in [
        "typing-private-endpoint",
        "typing-private-conversation",
        "receipt-private-endpoint",
        "receipt-private-conversation",
        "receipt-private-message",
    ] {
        assert!(
            !serialized.contains(forbidden),
            "typing/read receipt service action leaked {forbidden}"
        );
    }
}
