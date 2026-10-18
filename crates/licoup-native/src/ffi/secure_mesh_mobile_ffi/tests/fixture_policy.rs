use super::test_support::*;

#[test]
fn mobile_ffi_exposes_shared_file_route_and_receive_destination_policy() {
    let approved_root = std::env::temp_dir()
        .join("mobile-ffi-approved-root-canary")
        .join(uuid::Uuid::new_v4().to_string());
    let manifest = json!({
        "fileId": "mobile-ffi-file-id-canary",
        "fileName": "mobile-ffi-private-file-canary.pdf",
        "mimeType": "application/x-mobile-ffi-canary",
        "relativePath": "phone/mobile-ffi-private-relative-canary",
        "totalSize": 16,
        "chunkSize": 8,
        "chunkCount": 2
    });
    let route = dispatch_json(
        &json!({
            "action": "secure_mesh.file.route",
            "params": {"manifest": manifest}
        }),
        "ios_secure_mesh_native_json_action_unsupported",
    )
    .unwrap();
    assert_eq!(
        route["route"]["uploadOperation"],
        "secure_mesh.file_chunk.upload"
    );

    let receive_destination = dispatch_json(
        &json!({
            "action": "secure_mesh.file.receiveDestination",
            "params": {
                "manifest": manifest,
                "approvedRoot": approved_root.to_string_lossy()
            }
        }),
        "ios_secure_mesh_native_json_action_unsupported",
    )
    .unwrap();
    assert_eq!(
        receive_destination["receivePolicy"]["destinationApproved"],
        true
    );
    assert_eq!(
        receive_destination["receivePolicy"]["destinationPathRedacted"],
        true
    );

    let receive_confirmation = dispatch_json(
        &json!({
            "action": "secure_mesh.file.receiveConfirmation",
            "params": {
                "manifest": manifest,
                "approvedRoot": approved_root.to_string_lossy()
            }
        }),
        "ios_secure_mesh_native_json_action_unsupported",
    )
    .unwrap();
    assert_eq!(
        receive_confirmation["receiveConfirmation"]["required"],
        true
    );
    assert_eq!(
        receive_confirmation["receiveConfirmation"]["writeAllowed"],
        false
    );
    assert_eq!(
        receive_confirmation["receiveConfirmation"]["autoPreviewEnabled"],
        false
    );
    assert_eq!(
        receive_confirmation["receiveConfirmation"]["autoIngestionEnabled"],
        false
    );
    let serialized = serde_json::to_string(&receive_destination).unwrap();
    for forbidden in [
        "mobile-ffi-file-id-canary",
        "mobile-ffi-private-file-canary.pdf",
        "application/x-mobile-ffi-canary",
        "mobile-ffi-private-relative-canary",
        "mobile-ffi-approved-root-canary",
        approved_root.to_string_lossy().as_ref(),
    ] {
        assert!(
            !serialized.contains(forbidden),
            "mobile FFI file receive destination leaked {forbidden}"
        );
    }
    let serialized_confirmation = serde_json::to_string(&receive_confirmation).unwrap();
    for forbidden in [
        "mobile-ffi-file-id-canary",
        "mobile-ffi-private-file-canary.pdf",
        "application/x-mobile-ffi-canary",
        "mobile-ffi-private-relative-canary",
        "mobile-ffi-approved-root-canary",
        approved_root.to_string_lossy().as_ref(),
    ] {
        assert!(
            !serialized_confirmation.contains(forbidden),
            "mobile FFI file receive confirmation leaked {forbidden}"
        );
    }
}

#[test]
fn mobile_ffi_exposes_shared_file_handoff_reseal_proof_without_plaintext() {
    let proof = dispatch_json(
        &json!({
            "action": "secure_mesh.file.handoffProof",
            "params": {}
        }),
        "ios_secure_mesh_native_json_action_unsupported",
    )
    .unwrap();
    assert_eq!(proof["ok"], true);
    assert_eq!(proof["sourceOpenedByDesktop"], true);
    assert_eq!(proof["recipientOpenedResealed"], true);
    assert_eq!(proof["wrongRecipientRejected"], true);
    assert_eq!(proof["endpointSpecificResealReady"], true);
    assert_eq!(proof["multiRecipientIndependentResealReady"], true);
    assert_eq!(proof["serverVisibleNoPlaintext"], true);
    assert_eq!(proof["receiveConfirmationPolicyReady"], true);
    assert_eq!(proof["transfer"]["allRecipientTransfersAckPurged"], true);
    assert_eq!(proof["boundedTransferQueueReady"], true);
    assert_eq!(proof["transfer"]["boundedTransferQueueReady"], true);
    assert_eq!(proof["transfer"]["queue"]["activeTransferCount"], 0);
    assert_eq!(proof["transfer"]["queue"]["queuedCiphertextBytes"], 0);
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
            "mobile FFI file handoff proof leaked {forbidden}"
        );
    }
}

#[test]
fn mobile_ffi_exposes_shared_device_trust_actions_without_raw_keys() {
    let local = native_device_identity_fixture("desktop-native-trust", 11);
    let peer = native_device_identity_fixture("phone-native-trust", 22);
    let preview = dispatch_json(
        &json!({
            "action": "secure_mesh.deviceTrust.verifySas",
            "params": {
                "localIdentity": local,
                "peerIdentity": peer,
                "rosterEpoch": 3
            }
        }),
        "ios_secure_mesh_native_json_action_unsupported",
    )
    .unwrap();
    assert_eq!(preview["ok"], true);
    assert_eq!(preview["observationMatched"], false);
    assert_eq!(preview["sas"].as_array().map(Vec::len), Some(12));
    let sas = preview["sas"].clone();
    let verified = dispatch_json(
        &json!({
            "action": "secure_mesh.deviceTrust.verifySas",
            "params": {
                "localIdentity": native_device_identity_fixture("desktop-native-trust", 11),
                "peerIdentity": native_device_identity_fixture("phone-native-trust", 22),
                "rosterEpoch": 3,
                "sas": sas
            }
        }),
        "ios_secure_mesh_native_json_action_unsupported",
    )
    .unwrap();
    assert_eq!(verified["observationMatched"], true);
    assert_eq!(verified["decision"]["allowedForHighRiskCommand"], false);
    assert_eq!(verified["decision"]["requiresPersistedTrustRecord"], true);
    assert_eq!(
        verified["decision"]["code"],
        "verification_observation_requires_persisted_trust_record"
    );

    let revoked = dispatch_json(
        &json!({
            "action": "secure_mesh.deviceTrust.revoke",
            "params": {
                "identity": native_device_identity_fixture("phone-native-trust", 22)
            }
        }),
        "ios_secure_mesh_native_json_action_unsupported",
    )
    .unwrap();
    assert_eq!(revoked["trustState"], "revoked");
    assert_eq!(revoked["decision"]["allowedForHighRiskCommand"], false);

    let serialized = serde_json::to_string(&json!([preview, verified, revoked])).unwrap();
    for forbidden in [hex_bytes(11), hex_bytes(12), hex_bytes(22), hex_bytes(23)] {
        assert!(
            !serialized.contains(&forbidden),
            "mobile FFI trust response leaked raw public key material"
        );
    }
}

#[test]
fn mobile_ffi_exposes_shared_lifecycle_service_actions_without_plaintext() {
    let outputs = [
        json!({
            "action": "secure_mesh.lifecycle.serviceAction",
            "params": {
                "actionKind": "resend_request",
                "endpointId": "mobile-ffi-private-endpoint-canary",
                "conversationId": "mobile-ffi-private-conversation-canary",
                "missingMessageIds": ["mobile-ffi-private-missing-message-canary"],
                "body": "mobile-ffi-private-plaintext-canary"
            }
        }),
        json!({
            "action": "secure_mesh.lifecycle.serviceAction",
            "params": {
                "actionKind": "typing_state",
                "endpointId": "mobile-ffi-private-endpoint-canary",
                "conversationId": "mobile-ffi-private-conversation-canary",
                "typingState": "started",
                "body": "mobile-ffi-private-plaintext-canary"
            }
        }),
        json!({
            "action": "secure_mesh.lifecycle.serviceAction",
            "params": {
                "actionKind": "read_receipt",
                "endpointId": "mobile-ffi-private-endpoint-canary",
                "conversationId": "mobile-ffi-private-conversation-canary",
                "readUpToMessageId": "mobile-ffi-private-read-message-canary",
                "body": "mobile-ffi-private-plaintext-canary"
            }
        }),
    ]
    .into_iter()
    .map(|request| {
        dispatch_json(&request, "ios_secure_mesh_native_json_action_unsupported").unwrap()
    })
    .collect::<Vec<_>>();
    let output = &outputs[0];
    assert_eq!(output["ok"], true);
    assert_eq!(output["requiresPairwiseOrMlsEnvelope"], true);
    assert_eq!(output["serverVisiblePlaintextAllowed"], false);
    assert_eq!(output["servicePolicy"]["missingMessageIdsRedacted"], true);
    assert_eq!(outputs[1]["servicePolicy"]["typingStateEncrypted"], true);
    assert_eq!(outputs[1]["servicePolicy"]["typingContentIncluded"], false);
    assert_eq!(outputs[2]["servicePolicy"]["readMessageIdsRedacted"], true);
    let serialized = serde_json::to_string(&outputs).unwrap();
    for forbidden in [
        "mobile-ffi-private-endpoint-canary",
        "mobile-ffi-private-conversation-canary",
        "mobile-ffi-private-missing-message-canary",
        "mobile-ffi-private-read-message-canary",
        "mobile-ffi-private-plaintext-canary",
    ] {
        assert!(
            !serialized.contains(forbidden),
            "mobile FFI lifecycle service action leaked {forbidden}"
        );
    }
}
