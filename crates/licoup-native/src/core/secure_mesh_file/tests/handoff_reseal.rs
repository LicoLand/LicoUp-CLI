use super::support::*;

#[test]
fn secure_mesh_file_handoff_proof_reseals_endpoint_specific_ciphertext() {
    let proof = evaluate_file_handoff_proof_json(&json!({})).unwrap();
    assert_eq!(proof["ok"], true);
    assert_eq!(proof["sourceOpenedByDesktop"], true);
    assert_eq!(proof["recipientOpenedResealed"], true);
    assert_eq!(proof["wrongRecipientRejected"], true);
    assert_eq!(proof["endpointSpecificResealReady"], true);
    assert_eq!(proof["recipientCount"], 2);
    assert_eq!(proof["allRecipientsOpenedResealed"], true);
    assert_eq!(proof["allRecipientsWrongRecipientRejected"], true);
    assert_eq!(proof["allRecipientsEndpointSpecificResealReady"], true);
    assert_eq!(proof["multiRecipientIndependentResealReady"], true);
    assert_eq!(proof["allRecipientTransfersAckPurged"], true);
    assert_eq!(proof["deliveryJsonRedacted"], true);
    assert_eq!(proof["serverVisibleNoPlaintext"], true);
    assert_eq!(proof["routePolicyReady"], true);
    assert_eq!(proof["receiveDestinationPolicyReady"], true);
    assert_eq!(proof["receiveConfirmationPolicyReady"], true);
    assert_eq!(proof["transfer"]["recipientCount"], 2);
    assert_eq!(proof["transfer"]["allRecipientTransfersAckPurged"], true);
    assert_eq!(proof["recipientDeliveries"].as_array().unwrap().len(), 2);

    let serialized = serde_json::to_string(&proof).unwrap();
    for forbidden in [
        "handoff-proof-file-id-private-file-canary",
        "handoff-proof-private-file-canary.pdf",
        "application/x-handoff-private-file-canary",
        "private-relative-canary",
        "file-body-plaintext-secret-canary-content",
    ] {
        assert!(
            !serialized.contains(forbidden),
            "handoff proof leaked {forbidden}"
        );
    }
}

#[test]
fn secure_mesh_file_handoff_proof_reseals_distinct_ciphertext_for_multiple_recipients() {
    let proof = evaluate_file_handoff_proof_json(&json!({
        "recipientEndpoints": [
            "iphone-recipient-endpoint",
            "android-recipient-endpoint"
        ]
    }))
    .unwrap();
    assert_eq!(proof["ok"], true);
    assert_eq!(proof["recipientCount"], 2);
    assert_eq!(proof["allRecipientsOpenedResealed"], true);
    assert_eq!(proof["allRecipientsWrongRecipientRejected"], true);
    assert_eq!(proof["allRecipientsEndpointSpecificResealReady"], true);
    assert_eq!(proof["multiRecipientIndependentResealReady"], true);
    assert_eq!(proof["allRecipientTransfersAckPurged"], true);
    assert_eq!(proof["serverVisibleNoPlaintext"], true);

    let deliveries = proof["recipientDeliveries"].as_array().unwrap();
    assert_eq!(deliveries.len(), 2);
    assert_ne!(
        deliveries[0]["resealedManifestCiphertextHash"],
        deliveries[1]["resealedManifestCiphertextHash"]
    );
    assert_ne!(
        deliveries[0]["resealedChunkCiphertextHash"],
        deliveries[1]["resealedChunkCiphertextHash"]
    );
    for delivery in deliveries {
        assert_eq!(delivery["recipientOpenedResealed"], true);
        assert_eq!(delivery["wrongRecipientRejected"], true);
        assert_eq!(delivery["endpointSpecificResealReady"], true);
        assert_eq!(delivery["transferAckPurged"], true);
    }

    let serialized = serde_json::to_string(&proof).unwrap();
    for forbidden in [
        "handoff-proof-file-id-private-file-canary",
        "handoff-proof-private-file-canary.pdf",
        "application/x-handoff-private-file-canary",
        "private-relative-canary",
        "file-body-plaintext-secret-canary-content",
    ] {
        assert!(
            !serialized.contains(forbidden),
            "multi-recipient handoff proof leaked {forbidden}"
        );
    }
}
