use super::support::*;

#[test]
fn secure_mesh_mls_capability_extension_is_in_authenticated_add_join_commit_and_payload_paths() {
    let alice = device("desktop_gui:capability-alice");
    let bob = device("desktop_sidecar:capability-bob");
    let charlie = device("mobile:capability-charlie");
    let group_id = b"secure-mesh-product-capability-group";
    let mut alice_group = create_product_group(
        &alice.participant,
        &alice.identity,
        &DeviceTrustState::Verified,
        group_id,
    )
    .unwrap();
    let pending_context = SecureMeshContentContext::new(
        "env-capability-pending",
        "msg-capability-pending",
        "mailbox-capability-pending",
        &alice.identity.endpoint_id,
        &bob.identity.endpoint_id,
        "mls:capability-pending",
        "2026-07-11T00:00:00Z",
        "2026-07-11T00:10:00Z",
    );
    let pending_plaintext = SecureMeshPlaintext::new(SecureMeshPayloadKind::Command, b"blocked");
    let pending_error = seal_product_payload_message(
        &mut alice_group,
        &alice.participant,
        &alice.identity,
        &DeviceTrustState::Verified,
        &BTreeMap::from([(alice.identity.endpoint_id.clone(), alice.identity.clone())]),
        &pending_context,
        &pending_plaintext,
    )
    .unwrap_err();
    assert!(
        pending_error
            .to_string()
            .contains("capability negotiation is incomplete")
    );

    let path = ledger_path("capability-group-context");
    let mut ledger = SecureMeshMlsSecurityLedger::open(&path).unwrap();
    let bob_key_package = bob.participant.generate_key_package().unwrap();
    let bob_welcome = add_test_product_member(
        &mut alice_group,
        &alice,
        &bob,
        &bob_key_package,
        &mut ledger,
        "kp-capability-bob",
    );
    let bob_invitation = SecureMeshMlsExpectedInvitation::new(
        group_id,
        &alice.identity.endpoint_id,
        [
            alice.identity.endpoint_id.clone(),
            bob.identity.endpoint_id.clone(),
        ],
    )
    .unwrap();
    let mut bob_group =
        join_test_product_group(&bob, &alice, &bob_invitation, &bob_welcome, &mut ledger).unwrap();
    alice_group.require_active_capability_negotiation().unwrap();
    bob_group.require_active_capability_negotiation().unwrap();

    let charlie_key_package = charlie.participant.generate_key_package().unwrap();
    let charlie_welcome = add_test_product_member(
        &mut alice_group,
        &alice,
        &charlie,
        &charlie_key_package,
        &mut ledger,
        "kp-capability-charlie",
    );
    assert!(!charlie_welcome.commit_message.is_empty());
    let trusted_roster = BTreeMap::from([
        (alice.identity.endpoint_id.clone(), alice.identity.clone()),
        (bob.identity.endpoint_id.clone(), bob.identity.clone()),
        (
            charlie.identity.endpoint_id.clone(),
            charlie.identity.clone(),
        ),
    ]);
    process_test_product_commit(
        &mut bob_group,
        &bob.participant,
        &bob.identity,
        &alice.identity,
        &DeviceTrustState::Verified,
        Some(&charlie.identity),
        None,
        &trusted_roster,
        &charlie_welcome.commit_message,
        &mut ledger,
        capability_now(),
    )
    .unwrap();
    assert_eq!(
        alice_group.capability_extension().unwrap(),
        bob_group.capability_extension().unwrap()
    );
    let charlie_invitation = SecureMeshMlsExpectedInvitation::new(
        group_id,
        &alice.identity.endpoint_id,
        [
            alice.identity.endpoint_id.clone(),
            bob.identity.endpoint_id.clone(),
            charlie.identity.endpoint_id.clone(),
        ],
    )
    .unwrap();
    let mut charlie_group = join_test_product_group_with_roster(
        &charlie,
        &alice,
        &charlie_invitation,
        &charlie_welcome,
        &trusted_roster,
        &mut ledger,
    )
    .unwrap();
    let joined_extension = charlie_group.capability_extension().unwrap();
    let SecureMeshMlsCapabilityExtension::Active {
        member_capability_proofs,
        ..
    } = &joined_extension
    else {
        panic!("joined MLS capability extension must be active");
    };
    assert_eq!(member_capability_proofs.len(), 3);
    assert_eq!(
        member_capability_proofs
            .keys()
            .cloned()
            .collect::<BTreeSet<_>>(),
        charlie_invitation.expected_roster_endpoint_ids
    );
    let mut incomplete_history = joined_extension.clone();
    let SecureMeshMlsCapabilityExtension::Active {
        member_capability_proofs,
        ..
    } = &mut incomplete_history
    else {
        unreachable!();
    };
    member_capability_proofs.remove(&bob.identity.endpoint_id);
    let incomplete_error = verify_complete_member_capability_proof_map(
        &incomplete_history,
        &charlie_invitation.expected_roster_endpoint_ids,
        &trusted_roster,
    )
    .unwrap_err();
    assert!(
        incomplete_error
            .to_string()
            .contains("does not match roster")
    );

    let context = SecureMeshContentContext::new(
        "env-capability-active",
        "msg-capability-active",
        "mailbox-capability-active",
        &alice.identity.endpoint_id,
        "secure-mesh-capability-group",
        format!("mls:{}:capability-active", alice_group.epoch()),
        "2026-07-11T00:00:00Z",
        "2026-07-11T00:10:00Z",
    );
    let plaintext = SecureMeshPlaintext::new(
        SecureMeshPayloadKind::Command,
        br#"{"op":"capability-bound-group"}"#,
    );
    let message = seal_product_payload_message(
        &mut alice_group,
        &alice.participant,
        &alice.identity,
        &DeviceTrustState::Verified,
        &trusted_roster,
        &context,
        &plaintext,
    )
    .unwrap();
    let bob_opened = open_product_payload_message(
        &mut bob_group,
        &bob.participant,
        &bob.identity,
        &alice.identity,
        &DeviceTrustState::Verified,
        &trusted_roster,
        &context,
        &message,
        SecureMeshPayloadKind::Command,
    )
    .unwrap();
    let charlie_opened = open_product_payload_message(
        &mut charlie_group,
        &charlie.participant,
        &charlie.identity,
        &alice.identity,
        &DeviceTrustState::Verified,
        &trusted_roster,
        &context,
        &message,
        SecureMeshPayloadKind::Command,
    )
    .unwrap();
    assert_eq!(bob_opened.body, charlie_opened.body);

    let stripped_extension_commit = alice_group
        .stage_test_stripped_capability_extension_commit(&alice.participant)
        .unwrap();
    let tamper_offset = stripped_extension_commit.len() / 2;
    let mut tampered_update = stripped_extension_commit.clone();
    tampered_update[tamper_offset] ^= 1;
    let bob_epoch_before = bob_group.epoch();
    let tampered_error = process_test_product_commit(
        &mut bob_group,
        &bob.participant,
        &bob.identity,
        &alice.identity,
        &DeviceTrustState::Verified,
        None,
        None,
        &trusted_roster,
        &tampered_update,
        &mut ledger,
        capability_now(),
    )
    .unwrap_err();
    assert!(
        tampered_error.to_string().contains("commit")
            || tampered_error.to_string().contains("signature")
            || tampered_error.to_string().contains("confirmation")
    );
    assert_eq!(bob_group.epoch(), bob_epoch_before);

    let charlie_epoch_before = charlie_group.epoch();
    let stripped_error = process_test_product_commit(
        &mut charlie_group,
        &charlie.participant,
        &charlie.identity,
        &alice.identity,
        &DeviceTrustState::Verified,
        None,
        None,
        &trusted_roster,
        &stripped_extension_commit,
        &mut ledger,
        capability_now(),
    )
    .unwrap_err();
    assert!(
        stripped_error.to_string().contains("extension is missing")
            || stripped_error.to_string().contains("downgrade")
    );
    assert_eq!(charlie_group.epoch(), charlie_epoch_before);
    let _ = std::fs::remove_file(path);
}

#[test]
fn secure_mesh_mls_product_remove_is_identity_exact_journaled_and_excludes_file_epoch() {
    let alice = device("desktop_gui:remove-alice");
    let bob = device("desktop_sidecar:remove-bob");
    let charlie = device("mobile:remove-charlie");
    let group_id = b"secure-mesh-product-remove-group";
    let now = capability_now();
    let path = ledger_path("product-remove");
    let mut ledger = SecureMeshMlsSecurityLedger::open(&path).unwrap();

    let mut alice_group = create_product_group(
        &alice.participant,
        &alice.identity,
        &DeviceTrustState::Verified,
        group_id,
    )
    .unwrap();
    let bob_key_package = bob.participant.generate_key_package().unwrap();
    let bob_welcome = add_test_product_member(
        &mut alice_group,
        &alice,
        &bob,
        &bob_key_package,
        &mut ledger,
        "kp-remove-bob",
    );
    let bob_invitation = SecureMeshMlsExpectedInvitation::new(
        group_id,
        &alice.identity.endpoint_id,
        [
            alice.identity.endpoint_id.clone(),
            bob.identity.endpoint_id.clone(),
        ],
    )
    .unwrap();
    let mut bob_group =
        join_test_product_group(&bob, &alice, &bob_invitation, &bob_welcome, &mut ledger).unwrap();

    let charlie_key_package = charlie.participant.generate_key_package().unwrap();
    let charlie_welcome = add_test_product_member(
        &mut alice_group,
        &alice,
        &charlie,
        &charlie_key_package,
        &mut ledger,
        "kp-remove-charlie",
    );
    let full_roster = BTreeMap::from([
        (alice.identity.endpoint_id.clone(), alice.identity.clone()),
        (bob.identity.endpoint_id.clone(), bob.identity.clone()),
        (
            charlie.identity.endpoint_id.clone(),
            charlie.identity.clone(),
        ),
    ]);
    process_test_product_commit(
        &mut bob_group,
        &bob.participant,
        &bob.identity,
        &alice.identity,
        &DeviceTrustState::Verified,
        Some(&charlie.identity),
        None,
        &full_roster,
        &charlie_welcome.commit_message,
        &mut ledger,
        now,
    )
    .unwrap();
    let charlie_invitation = SecureMeshMlsExpectedInvitation::new(
        group_id,
        &alice.identity.endpoint_id,
        full_roster.keys().cloned(),
    )
    .unwrap();
    let mut charlie_group = join_test_product_group_with_roster(
        &charlie,
        &alice,
        &charlie_invitation,
        &charlie_welcome,
        &full_roster,
        &mut ledger,
    )
    .unwrap();

    let forged_key = SigningKey::generate(&mut OsRng);
    let forged_target = DeviceTrustPublicIdentity::new(
        charlie.identity.endpoint_id.clone(),
        SigningKey::generate(&mut OsRng).verifying_key().to_bytes(),
        forged_key.verifying_key().to_bytes(),
        charlie.identity.rotation_epoch,
    )
    .unwrap();
    let epoch_before_forgery = alice_group.epoch();
    let forged_error = match remove_product_member_prepared(
        &mut alice_group,
        &alice.participant,
        &alice.identity,
        &DeviceTrustState::Verified,
        &forged_target,
        &DeviceTrustState::Verified,
        now,
    ) {
        Ok(_) => panic!("forged removal identity unexpectedly resolved"),
        Err(error) => error,
    };
    assert!(forged_error.to_string().contains("exact current roster"));
    assert_eq!(alice_group.epoch(), epoch_before_forgery);

    let base = alice_group
        .public_metadata(alice.identity.fingerprint().unwrap())
        .unwrap();
    let operation_id = begin_test_journal_operation(
        &mut ledger,
        "secure_mesh.mls.member.remove",
        charlie.identity.fingerprint().unwrap().as_bytes(),
        &alice.identity,
        now,
    )
    .unwrap();
    let (remove_commit, prepared) = remove_product_member_prepared(
        &mut alice_group,
        &alice.participant,
        &alice.identity,
        &DeviceTrustState::Verified,
        &charlie.identity,
        &DeviceTrustState::Revoked,
        now,
    )
    .unwrap();
    assert!(remove_commit.welcome_message.is_none());
    let expected = alice_group
        .public_metadata(alice.identity.fingerprint().unwrap())
        .unwrap();
    commit_test_journal_operation(
        &mut ledger,
        &operation_id,
        group_id,
        Some(&base),
        &expected,
        &prepared,
        &serde_json::json!({"ok": true, "group": null}),
        now,
    )
    .unwrap();
    let replay_record = ledger
        .begin_operation(
            &operation_id,
            "secure_mesh.mls.member.remove",
            &hex_sha256(charlie.identity.fingerprint().unwrap().as_bytes()),
            &alice.identity,
            now.unix_timestamp(),
        )
        .unwrap();
    assert_eq!(replay_record.state, SecureMeshMlsOperationState::Delivered);

    let post_roster = BTreeMap::from([
        (alice.identity.endpoint_id.clone(), alice.identity.clone()),
        (bob.identity.endpoint_id.clone(), bob.identity.clone()),
    ]);
    process_test_product_commit(
        &mut bob_group,
        &bob.participant,
        &bob.identity,
        &alice.identity,
        &DeviceTrustState::Verified,
        None,
        Some(&charlie.identity),
        &post_roster,
        &remove_commit.commit_message,
        &mut ledger,
        now,
    )
    .unwrap();
    process_test_product_commit(
        &mut charlie_group,
        &charlie.participant,
        &charlie.identity,
        &alice.identity,
        &DeviceTrustState::Verified,
        None,
        Some(&charlie.identity),
        &post_roster,
        &remove_commit.commit_message,
        &mut ledger,
        now,
    )
    .unwrap();
    assert!(!charlie_group.is_active());
    assert_eq!(alice_group.member_count(), 2);
    assert_eq!(bob_group.member_count(), 2);
    let SecureMeshMlsCapabilityExtension::Active {
        roster_transition,
        member_capability_proofs,
        ..
    } = alice_group.capability_extension().unwrap()
    else {
        panic!("removed-member group capability extension must remain active");
    };
    assert!(matches!(
        roster_transition.as_ref(),
        SecureMeshMlsRosterTransition::MemberRemoved { member_endpoint_id }
            if member_endpoint_id == &charlie.identity.endpoint_id
    ));
    assert_eq!(
        member_capability_proofs
            .keys()
            .cloned()
            .collect::<BTreeSet<_>>(),
        post_roster.keys().cloned().collect::<BTreeSet<_>>()
    );

    let context = SecureMeshContentContext::new(
        "env-file-after-remove",
        "msg-file-after-remove",
        "mailbox-file-after-remove",
        &alice.identity.endpoint_id,
        &bob.identity.endpoint_id,
        format!("mls:{}:file-after-remove", alice_group.epoch()),
        "2026-07-11T00:00:00Z",
        "2026-07-11T00:10:00Z",
    );
    let file_chunk = SecureMeshPlaintext::new(
        SecureMeshPayloadKind::FileChunk,
        b"new-epoch-file-key-material-is-not-for-removed-members",
    )
    .with_content_type("application/octet-stream");
    let sealed = seal_product_payload_message(
        &mut alice_group,
        &alice.participant,
        &alice.identity,
        &DeviceTrustState::Verified,
        &post_roster,
        &context,
        &file_chunk,
    )
    .unwrap();
    let opened = open_product_payload_message(
        &mut bob_group,
        &bob.participant,
        &bob.identity,
        &alice.identity,
        &DeviceTrustState::Verified,
        &post_roster,
        &context,
        &sealed,
        SecureMeshPayloadKind::FileChunk,
    )
    .unwrap();
    assert_eq!(opened.body, file_chunk.body);
    let removed_open_error = open_product_payload_message(
        &mut charlie_group,
        &charlie.participant,
        &charlie.identity,
        &alice.identity,
        &DeviceTrustState::Verified,
        &post_roster,
        &context,
        &sealed,
        SecureMeshPayloadKind::FileChunk,
    )
    .unwrap_err();
    assert!(
        removed_open_error.to_string().contains("not active")
            || removed_open_error.to_string().contains("inactive member")
            || removed_open_error.to_string().contains("eviction")
            || removed_open_error.to_string().contains("open failed")
            || removed_open_error.to_string().contains("epoch")
    );
    let _ = std::fs::remove_file(path);
}
