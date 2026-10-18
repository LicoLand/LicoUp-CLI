use super::support::*;

#[test]
fn secure_mesh_mls_product_identity_bound_credentials_and_welcome_roster() {
    let alice = device("desktop_gui:alice");
    let bob = device("mobile:bob");
    let group_id = b"product-group-1".as_slice();
    let mut group = create_product_group(
        &alice.participant,
        &alice.identity,
        &DeviceTrustState::Verified,
        group_id,
    )
    .unwrap();
    let bob_kp = bob.participant.generate_key_package().unwrap();
    let path = ledger_path("welcome");
    let mut ledger = SecureMeshMlsSecurityLedger::open(&path).unwrap();
    let welcome =
        add_test_product_member(&mut group, &alice, &bob, &bob_kp, &mut ledger, "kp-bob-1");

    let invitation = SecureMeshMlsExpectedInvitation::new(
        group_id,
        "desktop_gui:alice",
        ["desktop_gui:alice", "mobile:bob"],
    )
    .unwrap();
    let bob_group =
        join_test_product_group(&bob, &alice, &invitation, &welcome, &mut ledger).unwrap();
    assert_eq!(bob_group.member_count(), 2);

    let unexpected = SecureMeshMlsExpectedInvitation::new(
        b"other-group",
        "desktop_gui:alice",
        ["desktop_gui:alice", "mobile:bob"],
    )
    .unwrap();
    let rejected = join_test_product_group(&bob, &alice, &unexpected, &welcome, &mut ledger);
    assert!(rejected.is_err());
    let rejected = rejected.err().unwrap();
    assert!(
        rejected.to_string().contains("group id mismatch")
            || rejected.to_string().contains("welcome")
    );

    let unverified =
        authorize_welcome_acceptance(&invitation, &DeviceTrustState::Unverified, group_id)
            .unwrap_err();
    assert!(unverified.to_string().contains("not verified"));
    let _ = std::fs::remove_file(&path);
}

#[test]
fn secure_mesh_mls_product_binds_claimed_identity_to_openmls_signers() {
    let alice = device("desktop_gui:signer-alice");
    let bob = device("mobile:signer-bob");
    let wrong_owner = create_product_group(
        &alice.participant,
        &bob.identity,
        &DeviceTrustState::Verified,
        b"wrong-owner",
    )
    .err()
    .expect("mismatched participant identity must fail");
    assert!(wrong_owner.to_string().contains("identity-bound"));

    let mut alice_group = create_product_group(
        &alice.participant,
        &alice.identity,
        &DeviceTrustState::Verified,
        b"signer-binding",
    )
    .unwrap();
    let bob_key_package = bob.participant.generate_key_package().unwrap();
    let path = ledger_path("signer-binding");
    let mut ledger = SecureMeshMlsSecurityLedger::open(&path).unwrap();
    let welcome = add_test_product_member(
        &mut alice_group,
        &alice,
        &bob,
        &bob_key_package,
        &mut ledger,
        "kp-signer-bob",
    );
    let invitation = SecureMeshMlsExpectedInvitation::new(
        b"signer-binding",
        &alice.identity.endpoint_id,
        [
            alice.identity.endpoint_id.clone(),
            bob.identity.endpoint_id.clone(),
        ],
    )
    .unwrap();
    let mut bob_group =
        join_test_product_group(&bob, &alice, &invitation, &welcome, &mut ledger).unwrap();
    let trusted_roster = BTreeMap::from([
        (alice.identity.endpoint_id.clone(), alice.identity.clone()),
        (bob.identity.endpoint_id.clone(), bob.identity.clone()),
    ]);
    let context = SecureMeshContentContext::new(
        "env-actual-signer",
        "msg-actual-signer",
        "mailbox-actual-signer",
        &bob.identity.endpoint_id,
        &bob.identity.endpoint_id,
        format!("mls:{}:actual-signer", alice_group.epoch()),
        "2026-07-11T00:00:00Z",
        "2026-07-11T00:10:00Z",
    );
    let plaintext = SecureMeshPlaintext::new(SecureMeshPayloadKind::Command, b"signed-by-alice");

    let claimed_sender_error = seal_product_payload_message(
        &mut alice_group,
        &alice.participant,
        &bob.identity,
        &DeviceTrustState::Verified,
        &trusted_roster,
        &context,
        &plaintext,
    )
    .unwrap_err();
    assert!(claimed_sender_error.to_string().contains("signer"));

    // A crate-internal raw message simulates an attempted bypass. Product open still checks
    // the actual OpenMLS credential and leaf signing key rather than trusting caller labels.
    let raw_message = alice_group
        .seal_payload_message(&alice.participant, &context, &plaintext)
        .unwrap();
    let actual_signer_error = open_product_payload_message(
        &mut bob_group,
        &bob.participant,
        &bob.identity,
        &bob.identity,
        &DeviceTrustState::Verified,
        &trusted_roster,
        &context,
        &raw_message,
        SecureMeshPayloadKind::Command,
    )
    .unwrap_err();
    assert!(actual_signer_error.to_string().contains("payload signer"));
    let _ = std::fs::remove_file(path);
}

#[test]
fn secure_mesh_mls_product_rejects_commit_claimed_as_another_member() {
    let alice = device("desktop_gui:commit-signer-alice");
    let bob = device("mobile:commit-signer-bob");
    let charlie = device("mobile:commit-signer-charlie");
    let group_id = b"commit-signer-binding";
    let mut alice_group = create_product_group(
        &alice.participant,
        &alice.identity,
        &DeviceTrustState::Verified,
        group_id,
    )
    .unwrap();
    let path = ledger_path("commit-signer-binding");
    let mut ledger = SecureMeshMlsSecurityLedger::open(&path).unwrap();
    let bob_key_package = bob.participant.generate_key_package().unwrap();
    let bob_welcome = add_test_product_member(
        &mut alice_group,
        &alice,
        &bob,
        &bob_key_package,
        &mut ledger,
        "kp-commit-signer-bob",
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
        "kp-commit-signer-charlie",
    );
    let trusted_roster = BTreeMap::from([
        (alice.identity.endpoint_id.clone(), alice.identity.clone()),
        (bob.identity.endpoint_id.clone(), bob.identity.clone()),
        (
            charlie.identity.endpoint_id.clone(),
            charlie.identity.clone(),
        ),
    ]);
    let epoch_before = bob_group.epoch();
    let error = process_test_product_commit(
        &mut bob_group,
        &bob.participant,
        &bob.identity,
        &bob.identity,
        &DeviceTrustState::Verified,
        Some(&charlie.identity),
        None,
        &trusted_roster,
        &charlie_welcome.commit_message,
        &mut ledger,
        capability_now(),
    )
    .unwrap_err();
    assert!(error.to_string().contains("commit signer"));
    assert_eq!(bob_group.epoch(), epoch_before);
    let _ = std::fs::remove_file(path);
}

#[test]
fn secure_mesh_mls_product_roster_cross_check() {
    let alice = device("desktop_gui:alice");
    let bob = device("mobile:bob");
    let expected = BTreeSet::from([
        alice.identity.endpoint_id.clone(),
        bob.identity.endpoint_id.clone(),
    ]);
    let mut trusted = BTreeMap::new();
    trusted.insert(alice.identity.endpoint_id.clone(), alice.identity.clone());
    trusted.insert(bob.identity.endpoint_id.clone(), bob.identity.clone());
    let observed = vec![
        mls_credential_identity_bytes(&alice.identity).unwrap(),
        mls_credential_identity_bytes(&bob.identity).unwrap(),
    ];
    cross_check_roster(&expected, &observed, &trusted).unwrap();
    let diverged = cross_check_roster(&expected, &observed[..1], &trusted).unwrap_err();
    assert!(diverged.to_string().contains("roster size divergence"));
}
