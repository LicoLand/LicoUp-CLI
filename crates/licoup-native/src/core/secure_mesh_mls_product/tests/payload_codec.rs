use super::support::*;

#[test]
fn secure_mesh_mls_product_payload_rejects_forged_sender_context() {
    let alice = device("desktop_gui:alice");
    let bob = device("mobile:bob");
    let mut alice_group = create_product_group(
        &alice.participant,
        &alice.identity,
        &DeviceTrustState::Verified,
        b"sender-bind",
    )
    .unwrap();
    let bob_kp = bob.participant.generate_key_package().unwrap();
    let path = ledger_path("sender");
    let mut ledger = SecureMeshMlsSecurityLedger::open(&path).unwrap();
    let welcome = add_test_product_member(
        &mut alice_group,
        &alice,
        &bob,
        &bob_kp,
        &mut ledger,
        "kp-bob-sender",
    );
    let invitation = SecureMeshMlsExpectedInvitation::new(
        b"sender-bind",
        "desktop_gui:alice",
        ["desktop_gui:alice", "mobile:bob"],
    )
    .unwrap();
    let mut bob_group =
        join_test_product_group(&bob, &alice, &invitation, &welcome, &mut ledger).unwrap();
    let context = SecureMeshContentContext::new(
        "env-sender",
        "msg-sender",
        "mailbox-bob",
        "desktop_gui:alice",
        "mobile:bob",
        format!("mls:{}:sender-bind", alice_group.epoch()),
        "2026-07-11T00:00:00Z",
        "2026-07-11T00:10:00Z",
    );
    let plaintext = SecureMeshPlaintext::new(SecureMeshPayloadKind::Command, br#"{"op":"ping"}"#)
        .with_content_type("application/json");
    let trusted_roster = BTreeMap::from([
        (alice.identity.endpoint_id.clone(), alice.identity.clone()),
        (bob.identity.endpoint_id.clone(), bob.identity.clone()),
    ]);
    let sealed = seal_product_payload_message(
        &mut alice_group,
        &alice.participant,
        &alice.identity,
        &DeviceTrustState::Verified,
        &trusted_roster,
        &context,
        &plaintext,
    )
    .unwrap();
    authorize_sender_endpoint_binding(&context.sender_endpoint_id, "desktop_gui:alice").unwrap();
    let mut forged = context.clone();
    forged.sender_endpoint_id = "mobile:attacker".to_string();
    let error = authorize_sender_endpoint_binding(&forged.sender_endpoint_id, "desktop_gui:alice")
        .unwrap_err();
    assert!(error.to_string().contains("forged sender"));
    // Opening with forged sender context fails closed on AAD/exporter binding.
    let open_error = open_product_payload_message(
        &mut bob_group,
        &bob.participant,
        &bob.identity,
        &alice.identity,
        &DeviceTrustState::Verified,
        &trusted_roster,
        &forged,
        &sealed,
        SecureMeshPayloadKind::Command,
    )
    .unwrap_err();
    assert!(
        open_error.to_string().contains("forged sender")
            || open_error.to_string().contains("AAD")
            || open_error.to_string().contains("open failed")
            || open_error.to_string().contains("mismatch")
    );
    let opened = open_product_payload_message(
        &mut bob_group,
        &bob.participant,
        &bob.identity,
        &alice.identity,
        &DeviceTrustState::Verified,
        &trusted_roster,
        &context,
        &sealed,
        SecureMeshPayloadKind::Command,
    )
    .unwrap();
    assert_eq!(opened.body, br#"{"op":"ping"}"#);
    let _ = std::fs::remove_file(&path);
}
