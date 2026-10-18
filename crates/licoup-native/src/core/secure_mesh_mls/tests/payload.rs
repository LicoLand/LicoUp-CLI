use super::support::*;

#[test]
fn secure_mesh_mls_active_payload_key_is_mlkem1024_epoch_hybrid() {
    let (alice, bob, alice_group, bob_group) =
        active_payload_group_pair(b"secure-mesh-group-mlkem1024-hybrid-test");
    let _alice_key = alice_group
        .derive_group_payload_content_key(&alice)
        .unwrap();
    let _bob_key = bob_group.derive_group_payload_content_key(&bob).unwrap();
    let extension = alice_group.mlkem1024_epoch_extension().unwrap();
    assert_eq!(extension.epoch, alice_group.epoch());
    assert_eq!(extension.recipients.len(), 2);
    assert_eq!(
        extension.recipients,
        bob_group.mlkem1024_epoch_extension().unwrap().recipients
    );
}

#[test]
fn secure_mesh_openmls_group_payload_wire_has_fixed_public_aad_and_private_full_context() {
    let (alice, bob, mut alice_group, mut bob_group) =
        active_payload_group_pair(b"secure-mesh-group-private-context-wire-test");
    let context = SecureMeshContentContext::new(
        "wire-envelope-private-canary",
        "wire-message-private-canary",
        "wire-mailbox-private-canary",
        "wire-sender-private-canary",
        "wire-recipient-private-canary",
        "wire-session-private-canary",
        "2032-05-06T07:08:09.000Z",
        "2032-05-06T07:18:09.000Z",
    );
    let plaintext = SecureMeshPlaintext::new(
        SecureMeshPayloadKind::ServiceAction,
        b"wire-body-private-canary".as_slice(),
    )
    .with_content_type("application/x-wire-private-canary");

    let message = alice_group
        .seal_payload_message(&alice, &context, &plaintext)
        .unwrap();
    deserialize_protocol_message(
        &message,
        "secure mesh MLS private-context wire parse failed",
    )
    .unwrap();
    assert!(
        message
            .windows(SECURE_MESH_MLS_APPLICATION_PUBLIC_AAD.len())
            .any(|window| window == SECURE_MESH_MLS_APPLICATION_PUBLIC_AAD),
        "fixed versioned MLS application AAD must be present on the actual wire"
    );
    let business_canaries: [&[u8]; 12] = [
        context.envelope_id.as_bytes(),
        context.message_id.as_bytes(),
        context.opaque_mailbox_id.as_bytes(),
        context.sender_endpoint_id.as_bytes(),
        context.recipient_endpoint_id.as_bytes(),
        context.session_id.as_bytes(),
        context.created_at.as_bytes(),
        context.expires_at.as_bytes(),
        plaintext.kind.as_str().as_bytes(),
        plaintext.content_type.as_deref().unwrap().as_bytes(),
        plaintext.body.as_slice(),
        MLS_PRIVATE_CONTEXT_PAYLOAD_MAGIC,
    ];
    for canary in business_canaries {
        assert!(
            !message.windows(canary.len()).any(|window| window == canary),
            "MLS application wire exposed an encrypted inner-frame canary"
        );
    }

    let protocol_message = deserialize_protocol_message(
        &message,
        "secure mesh MLS private-context wire parse failed",
    )
    .unwrap();
    let processed = bob_group
        .group
        .process_message(&bob.provider, protocol_message)
        .unwrap();
    assert_eq!(
        processed.aad(),
        SECURE_MESH_MLS_APPLICATION_PUBLIC_AAD,
        "OpenMLS authenticated_data must be fixed and business-free"
    );
    let encoded = match processed.into_content() {
        ProcessedMessageContent::ApplicationMessage(application_message) => {
            application_message.into_bytes()
        }
        _ => panic!("secure mesh MLS private-context wire was not application data"),
    };
    let sealed = decode_mls_private_context_payload(&encoded).unwrap();
    let content_key = bob_group.derive_group_payload_content_key(&bob).unwrap();
    let opened = open_private_context_payload(&content_key, &sealed).unwrap();
    let (opened_context, opened_payload) = opened.into_parts();
    assert_eq!(opened_context, context);
    assert_eq!(opened_payload.kind, plaintext.kind);
    assert_eq!(opened_payload.body, plaintext.body);
    assert_eq!(opened_payload.content_type, plaintext.content_type);
    assert_eq!(opened_payload.created_at, context.created_at);
    assert_eq!(opened_payload.expires_at, context.expires_at);
}

#[test]
fn secure_mesh_openmls_group_payload_rejects_context_tamper() {
    let alice = SecureMeshMlsParticipant::new(b"desktop_gui:alice".to_vec()).unwrap();
    let bob = SecureMeshMlsParticipant::new(b"mobile:bob".to_vec()).unwrap();
    let bob_key_package = bob.generate_key_package().unwrap();
    let mut alice_group =
        SecureMeshMlsGroup::create(&alice, b"secure-mesh-group-payload-aad-test").unwrap();
    let welcome = alice_group.add_member(&alice, &bob_key_package).unwrap();
    let mut bob_group =
        SecureMeshMlsGroup::join_from_welcome(&bob, &welcome.welcome_message).unwrap();
    let capability_commit =
        activate_test_payload_capabilities(&mut alice_group, &alice, &[&alice, &bob]);
    process_test_payload_capability_commit(&mut bob_group, &bob, &capability_commit);
    let context = content_context_fixture(
        "msg_group_payload_aad",
        "desktop_gui:alice",
        "mobile:bob",
        format!(
            "mls:{}:{}",
            alice_group.epoch(),
            "secure-mesh-group-payload-aad-test"
        ),
    );
    let plaintext =
        SecureMeshPlaintext::new(SecureMeshPayloadKind::ResultPayload, br#"{"ok":true}"#)
            .with_content_type("application/json");
    let message = alice_group
        .seal_payload_message(&alice, &context, &plaintext)
        .unwrap();
    let mut tampered = context.clone();
    tampered.message_id = "msg_group_payload_tampered".to_string();

    let error = bob_group
        .open_payload_message(
            &bob,
            &tampered,
            &message,
            SecureMeshPayloadKind::ResultPayload,
        )
        .unwrap_err();
    assert!(
        error
            .to_string()
            .contains("encrypted inner context mismatch")
    );
}

#[test]
fn secure_mesh_openmls_group_payload_rejects_expected_kind_mismatch() {
    let (alice, bob, mut alice_group, mut bob_group) =
        active_payload_group_pair(b"secure-mesh-group-payload-kind-test");
    let context = content_context_fixture(
        "msg_group_payload_kind",
        "desktop_gui:alice",
        "mobile:bob",
        format!(
            "mls:{}:{}",
            alice_group.epoch(),
            "secure-mesh-group-payload-kind-test"
        ),
    );
    let plaintext = SecureMeshPlaintext::new(SecureMeshPayloadKind::Error, b"kind-private");
    let message = alice_group
        .seal_payload_message(&alice, &context, &plaintext)
        .unwrap();

    let error = bob_group
        .open_payload_message(&bob, &context, &message, SecureMeshPayloadKind::Command)
        .unwrap_err();
    assert!(
        error
            .to_string()
            .contains("encrypted inner payload kind mismatch")
    );
}

#[test]
fn secure_mesh_openmls_group_payload_rejects_authenticated_data_wire_tamper() {
    let (alice, bob, mut alice_group, mut bob_group) =
        active_payload_group_pair(b"secure-mesh-group-payload-public-aad-test");
    let context = content_context_fixture(
        "msg_group_payload_public_aad",
        "desktop_gui:alice",
        "mobile:bob",
        format!(
            "mls:{}:{}",
            alice_group.epoch(),
            "secure-mesh-group-payload-public-aad-test"
        ),
    );
    let plaintext = SecureMeshPlaintext::new(SecureMeshPayloadKind::Command, b"aad-private");
    let mut message = alice_group
        .seal_payload_message(&alice, &context, &plaintext)
        .unwrap();
    let aad_offset = message
        .windows(SECURE_MESH_MLS_APPLICATION_PUBLIC_AAD.len())
        .position(|window| window == SECURE_MESH_MLS_APPLICATION_PUBLIC_AAD)
        .expect("fixed MLS authenticated_data must be serialized on the wire");
    message[aad_offset + SECURE_MESH_MLS_APPLICATION_PUBLIC_AAD.len() - 1] ^= 0x01;
    deserialize_protocol_message(
        &message,
        "secure mesh MLS authenticated_data tamper must remain structurally parseable",
    )
    .unwrap();

    let error = bob_group
        .open_payload_message(&bob, &context, &message, SecureMeshPayloadKind::Command)
        .unwrap_err();
    assert!(
        error.to_string().contains("open failed")
            || error.to_string().contains("rejected")
            || error.to_string().contains("AAD mismatch")
    );
}
