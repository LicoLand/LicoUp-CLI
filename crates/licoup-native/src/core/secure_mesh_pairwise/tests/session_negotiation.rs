use super::test_support::*;

#[test]
fn secure_mesh_pairwise_session_id_binds_classical_and_pq_one_time_prekeys() {
    let alice = fixed_endpoint("desktop_gui:alice-vector", 1, 91);
    let bob = fixed_endpoint("mobile:bob-vector", 2, 92);
    let alice_ephemeral = fixed_pairwise_key(5);
    let bob_signed_prekey = fixed_pairwise_key(3);
    let bob_one_time_prekey = fixed_pairwise_key(4);
    let replaced_one_time_prekey = fixed_pairwise_key(44);
    let alice_ephemeral_public_key = alice_ephemeral.public_key();
    let bob_signed_prekey_public_key = bob_signed_prekey.public_key();
    let bob_one_time_prekey_public_key = bob_one_time_prekey.public_key();
    let replaced_one_time_prekey_public_key = replaced_one_time_prekey.public_key();
    let pq_seed =
        SecureMeshMlKem1024PreKeySeed::from_bytes([0x71; ML_KEM_1024_KEY_GENERATION_SEED_BYTES]);
    let pq_public_key = pq_seed.public_key();
    let mlkem1024_ciphertext = [0x81; ML_KEM_1024_CIPHERTEXT_BYTES];
    let original_session_id = derive_session_id(
        &alice.identity,
        &bob.identity,
        &alice_ephemeral_public_key,
        "spk-vector",
        &bob_signed_prekey_public_key,
        Some("otpk-vector"),
        Some(&bob_one_time_prekey_public_key),
        "pqotpk-vector",
        &pq_public_key,
        &mlkem1024_ciphertext,
        "sha256:vector-tree-head",
    )
    .unwrap();
    let repeated_session_id = derive_session_id(
        &alice.identity,
        &bob.identity,
        &alice_ephemeral_public_key,
        "spk-vector",
        &bob_signed_prekey_public_key,
        Some("otpk-vector"),
        Some(&bob_one_time_prekey_public_key),
        "pqotpk-vector",
        &pq_public_key,
        &mlkem1024_ciphertext,
        "sha256:vector-tree-head",
    )
    .unwrap();
    let replaced_session_id = derive_session_id(
        &alice.identity,
        &bob.identity,
        &alice_ephemeral_public_key,
        "spk-vector",
        &bob_signed_prekey_public_key,
        Some("otpk-vector"),
        Some(&replaced_one_time_prekey_public_key),
        "pqotpk-vector",
        &pq_public_key,
        &mlkem1024_ciphertext,
        "sha256:vector-tree-head",
    )
    .unwrap();
    let replaced_pq_seed =
        SecureMeshMlKem1024PreKeySeed::from_bytes([0x72; ML_KEM_1024_KEY_GENERATION_SEED_BYTES]);
    let replaced_pq_session_id = derive_session_id(
        &alice.identity,
        &bob.identity,
        &alice_ephemeral_public_key,
        "spk-vector",
        &bob_signed_prekey_public_key,
        Some("otpk-vector"),
        Some(&bob_one_time_prekey_public_key),
        "pqotpk-vector",
        &replaced_pq_seed.public_key(),
        &mlkem1024_ciphertext,
        "sha256:vector-tree-head",
    )
    .unwrap();
    let replaced_ciphertext_session_id = derive_session_id(
        &alice.identity,
        &bob.identity,
        &alice_ephemeral_public_key,
        "spk-vector",
        &bob_signed_prekey_public_key,
        Some("otpk-vector"),
        Some(&bob_one_time_prekey_public_key),
        "pqotpk-vector",
        &pq_public_key,
        &[0x82; ML_KEM_1024_CIPHERTEXT_BYTES],
        "sha256:vector-tree-head",
    )
    .unwrap();

    assert_eq!(original_session_id, repeated_session_id);
    assert_ne!(original_session_id, replaced_session_id);
    assert_ne!(original_session_id, replaced_pq_session_id);
    assert_ne!(original_session_id, replaced_ciphertext_session_id);
}

#[test]
fn secure_mesh_pairwise_rejects_non_contributory_x25519_keys() {
    let local_key = fixed_pairwise_key(7);
    let low_order_public_key = [0u8; PUBLIC_KEY_LEN];
    let error = local_key.diffie_hellman(&low_order_public_key).unwrap_err();
    assert!(error.to_string().contains("non-contributory"));

    let alice = endpoint("desktop_gui:alice-low-order");
    let bob = endpoint("mobile:bob-low-order");
    let mut bob_prekeys = prekeys(&bob);
    bob_prekeys.bundle.signed_prekey = sign_prekey_record(
        &bob.signing_key,
        &bob.identity,
        SecureMeshPreKeyKind::SignedPreKey,
        "spk-low-order",
        low_order_public_key,
        "2026-06-26T00:00:00Z",
        "2026-07-26T00:00:00Z",
    )
    .unwrap();
    let bob_directory = authorize_test_pairwise_prekey_bundle(&bob_prekeys.bundle);
    let now = OffsetDateTime::parse(
        "2026-06-26T00:00:01Z",
        &time::format_description::well_known::Rfc3339,
    )
    .unwrap();
    let handshake_error = SecureMeshPairwiseSession::initiate(
        &alice.identity,
        &alice.identity_secret,
        &alice.signing_key,
        &bob_prekeys.bundle,
        &bob_directory,
        &SecureMeshPreKeyValidationPolicy::default(),
        &secure_mesh_pairwise_test_capability_evaluation().unwrap(),
        now,
    )
    .err()
    .expect("low-order signed prekey must fail");
    assert!(handshake_error.to_string().contains("non-contributory"));

    let (mut alice_session, mut bob_session) = pairwise_sessions();
    let first = alice_session
        .seal_message("msg-low-order-ratchet", b"valid first message")
        .unwrap();
    let mut malicious = first.clone();
    malicious.sender_ratchet_public_key = low_order_public_key.to_vec();
    let ratchet_error = bob_session.open_message(&malicious).unwrap_err();
    assert!(ratchet_error.to_string().contains("non-contributory"));
    assert_eq!(bob_session.dh_epoch(), 0);
    assert_eq!(bob_session.received_count(), 0);
    assert!(bob_session.initiator_key_confirmed);
    assert_eq!(
        bob_session.open_message(&first).unwrap().body,
        b"valid first message"
    );
}

#[test]
fn secure_mesh_pairwise_rejects_server_tampering_with_signed_handshake_transcript() {
    let fixture = handshake_fixture();
    let mut tampered_intros = Vec::new();

    let mut changed_session = fixture.intro.clone();
    changed_session.session_id.push_str("-server-substitution");
    tampered_intros.push(changed_session);

    let mut changed_endpoint = fixture.intro.clone();
    changed_endpoint.responder_endpoint_id = "mobile:attacker".to_string();
    tampered_intros.push(changed_endpoint);

    let mut changed_ephemeral = fixture.intro.clone();
    changed_ephemeral.initiator_ephemeral_public_key = fixed_pairwise_key(73).public_key().to_vec();
    tampered_intros.push(changed_ephemeral);

    let mut changed_directory_authorization = fixture.intro.clone();
    changed_directory_authorization.directory_authorization_digest = "cd".repeat(32);
    tampered_intros.push(changed_directory_authorization);

    let mut changed_signature = fixture.intro.clone();
    let mut signature = general_purpose::URL_SAFE_NO_PAD
        .decode(&changed_signature.initiator_signature)
        .unwrap();
    signature[0] ^= 1;
    changed_signature.initiator_signature = general_purpose::URL_SAFE_NO_PAD.encode(signature);
    tampered_intros.push(changed_signature);

    for tampered in tampered_intros {
        assert!(
            SecureMeshPairwiseSession::accept(
                &fixture.bob.identity,
                &fixture.bob.identity_secret,
                &fixture.bob.signing_key,
                &fixture.alice.identity,
                &fixture.bob_prekeys.signed_secret,
                Some(&fixture.bob_prekeys.one_time_secret),
                &fixture.bob_prekeys.one_time_mlkem1024_seed,
                &tampered,
                &secure_mesh_pairwise_test_capability_evaluation().unwrap(),
                handshake_now(),
                &mut CapabilityProofReplayGuard::default(),
            )
            .is_err(),
            "server-modified intro was accepted"
        );
    }

    let mut tampered_accepts = Vec::new();
    let mut changed_ratchet = fixture.accepted.clone();
    changed_ratchet.responder_initial_ratchet_public_key =
        fixed_pairwise_key(74).public_key().to_vec();
    tampered_accepts.push(changed_ratchet);

    let mut changed_hash = fixture.accepted.clone();
    changed_hash.handshake_transcript_hash =
        general_purpose::URL_SAFE_NO_PAD.encode([9u8; HANDSHAKE_HASH_LEN]);
    tampered_accepts.push(changed_hash);

    let mut changed_accept_signature = fixture.accepted.clone();
    let mut signature = general_purpose::URL_SAFE_NO_PAD
        .decode(&changed_accept_signature.responder_signature)
        .unwrap();
    signature[0] ^= 1;
    changed_accept_signature.responder_signature =
        general_purpose::URL_SAFE_NO_PAD.encode(signature);
    tampered_accepts.push(changed_accept_signature);

    let mut changed_confirmation = fixture.accepted.clone();
    let mut confirmation = general_purpose::URL_SAFE_NO_PAD
        .decode(&changed_confirmation.key_confirmation)
        .unwrap();
    confirmation[0] ^= 1;
    changed_confirmation.key_confirmation = general_purpose::URL_SAFE_NO_PAD.encode(confirmation);
    tampered_accepts.push(changed_confirmation);

    for tampered in tampered_accepts {
        let mut candidate = fixture.alice_session.clone();
        assert!(
            candidate
                .complete_initiator_handshake(
                    &fixture.alice.identity,
                    &fixture.bob.identity,
                    &tampered,
                    handshake_now(),
                    &mut CapabilityProofReplayGuard::default(),
                )
                .is_err(),
            "server-modified accept was accepted"
        );
        assert!(!candidate.initiator_key_confirmed);
        assert_eq!(candidate.remote_ratchet_public_key, [0u8; PUBLIC_KEY_LEN]);
        assert!(
            candidate
                .seal_message("msg-before-confirmation", b"blocked")
                .is_err()
        );
    }
}

#[test]
fn secure_mesh_pairwise_capability_proofs_gate_production_handshake_and_replay() {
    let fixture = handshake_fixture();
    let responder_projection = fixture
        .bob_session
        .capability_projection()
        .expect("responder capability negotiation must be verified during accept");
    assert!(responder_projection.peer.is_some());
    assert!(
        responder_projection
            .negotiated_protocol_capabilities
            .iter()
            .all(|capability| capability.id().starts_with("protocol."))
    );

    let mut pending = fixture.alice_session.clone();
    let blocked = pending
        .seal_message("msg-capability-pending", b"blocked")
        .unwrap_err();
    assert!(blocked.to_string().contains("capability negotiation"));

    let mut tampered_intro = fixture.intro.clone();
    tampered_intro
        .initiator_capability_proof
        .claims
        .policy_revision += 1;
    tampered_intro.initiator_signature = sign_pairwise_transcript(
        &fixture.alice.signing_key,
        &intro_signature_payload(&tampered_intro).unwrap(),
    );
    let rejected_intro = SecureMeshPairwiseSession::accept(
        &fixture.bob.identity,
        &fixture.bob.identity_secret,
        &fixture.bob.signing_key,
        &fixture.alice.identity,
        &fixture.bob_prekeys.signed_secret,
        Some(&fixture.bob_prekeys.one_time_secret),
        &fixture.bob_prekeys.one_time_mlkem1024_seed,
        &tampered_intro,
        &secure_mesh_pairwise_test_capability_evaluation().unwrap(),
        handshake_now(),
        &mut CapabilityProofReplayGuard::default(),
    )
    .err()
    .expect("tampered capability proof must be rejected");
    assert!(rejected_intro.to_string().contains("capability proof"));

    let mut tampered_accepted = fixture.accepted.clone();
    tampered_accepted
        .capability_binding
        .negotiated_protocol_capabilities
        .remove(&crate::core::secure_mesh_capability::SecurityCapability::AuthenticatedEncryption);
    tampered_accepted.responder_signature = sign_pairwise_transcript(
        &fixture.bob.signing_key,
        &accept_signature_payload(&tampered_accepted).unwrap(),
    );
    tampered_accepted.key_confirmation =
        pairwise_key_confirmation(&fixture.alice_session.root_key, &tampered_accepted).unwrap();
    let rejected_binding = pending
        .complete_initiator_handshake(
            &fixture.alice.identity,
            &fixture.bob.identity,
            &tampered_accepted,
            handshake_now(),
            &mut CapabilityProofReplayGuard::default(),
        )
        .unwrap_err();
    assert!(
        rejected_binding
            .to_string()
            .contains("capability transcript binding")
    );

    let mut first = fixture.alice_session.clone();
    let mut replay = fixture.alice_session.clone();
    let mut replay_guard = CapabilityProofReplayGuard::default();
    first
        .complete_initiator_handshake(
            &fixture.alice.identity,
            &fixture.bob.identity,
            &fixture.accepted,
            handshake_now(),
            &mut replay_guard,
        )
        .unwrap();
    let initiator_projection = first
        .capability_projection()
        .expect("initiator capability negotiation must be verified during completion");
    assert_eq!(
        initiator_projection.negotiated_protocol_capabilities,
        fixture
            .accepted
            .capability_binding
            .negotiated_protocol_capabilities
    );
    let replay_error = replay
        .complete_initiator_handshake(
            &fixture.alice.identity,
            &fixture.bob.identity,
            &fixture.accepted,
            handshake_now(),
            &mut replay_guard,
        )
        .unwrap_err();
    assert!(replay_error.to_string().contains("replay rejected"));
}

#[test]
fn secure_mesh_pairwise_replay_guards_are_explicitly_owned_and_parallel_isolated() {
    let fixture = handshake_fixture();
    let mut workers = (0..16)
        .map(|index| {
            let mut session = fixture.alice_session.clone();
            let local_identity = fixture.alice.identity.clone();
            let remote_identity = fixture.bob.identity.clone();
            let accepted = fixture.accepted.clone();
            std::thread::spawn(move || {
                if index % 3 == 0 {
                    std::thread::yield_now();
                }
                let mut replay_guard = CapabilityProofReplayGuard::default();
                session.complete_initiator_handshake(
                    &local_identity,
                    &remote_identity,
                    &accepted,
                    handshake_now(),
                    &mut replay_guard,
                )
            })
        })
        .collect::<Vec<_>>();
    if workers.len() % 2 == 0 {
        workers.reverse();
    }
    for worker in workers {
        worker
            .join()
            .expect("parallel replay-guard worker must not panic")
            .expect("independently owned replay guard must not inherit another worker's state");
    }
}

#[test]
fn secure_mesh_pairwise_responder_requires_valid_initiator_finished_message() {
    let mut fixture = handshake_fixture();
    let finished = fixture
        .alice_session
        .complete_initiator_handshake(
            &fixture.alice.identity,
            &fixture.bob.identity,
            &fixture.accepted,
            handshake_now(),
            &mut CapabilityProofReplayGuard::default(),
        )
        .unwrap();
    assert!(
        fixture
            .bob_session
            .seal_message("msg-responder-too-early", b"blocked")
            .is_err()
    );

    let first = fixture
        .alice_session
        .seal_message("msg-after-finished", b"initiator application data")
        .unwrap();
    let early_error = fixture.bob_session.open_message(&first).unwrap_err();
    assert!(
        early_error
            .to_string()
            .contains("confirmation is incomplete")
    );
    assert!(!fixture.bob_session.initiator_key_confirmed);
    assert_eq!(fixture.bob_session.dh_epoch(), 0);

    let mut wrong_binding = finished.clone();
    wrong_binding.capability_transcript_digest =
        crate::core::secure_mesh_capability_proof::encode_sha256_digest(&[0x5a; 32]);
    let wrong_binding_error = fixture
        .bob_session
        .complete_responder_handshake(&wrong_binding)
        .unwrap_err();
    assert!(
        wrong_binding_error
            .to_string()
            .contains("capability transcript mismatch")
    );

    let mut forged_finished = finished.clone();
    let mut confirmation = general_purpose::URL_SAFE_NO_PAD
        .decode(&forged_finished.key_confirmation)
        .unwrap();
    confirmation[0] ^= 1;
    forged_finished.key_confirmation = general_purpose::URL_SAFE_NO_PAD.encode(confirmation);
    let forged_error = fixture
        .bob_session
        .complete_responder_handshake(&forged_finished)
        .unwrap_err();
    assert!(forged_error.to_string().contains("verification failed"));
    assert!(
        fixture
            .bob_session
            .seal_message("msg-responder-still-too-early", b"blocked")
            .is_err()
    );

    fixture
        .bob_session
        .complete_responder_handshake(&finished)
        .unwrap();
    assert!(fixture.bob_session.initiator_key_confirmed);
    assert!(!fixture.bob_session.pending_sending_ratchet());
    assert_eq!(
        fixture.bob_session.open_message(&first).unwrap().body,
        b"initiator application data"
    );
    assert!(fixture.bob_session.initiator_key_confirmed);
    assert!(fixture.bob_session.pending_sending_ratchet());
    assert_eq!(
        fixture
            .bob_session
            .seal_message("msg-responder-confirmed", b"allowed")
            .unwrap()
            .dh_epoch,
        2
    );
}

#[test]
fn secure_mesh_pairwise_pqxdh_derives_matching_independent_triple_ratchet_secrets() {
    let alice = fixed_endpoint("desktop_gui:alice-vector", 1, 91);
    let bob = fixed_endpoint("mobile:bob-vector", 2, 92);
    let bob_prekeys = fixed_prekeys(&bob, 3, 4);
    let alice_ephemeral = fixed_pairwise_key(5);
    let alice_ratchet = fixed_pairwise_key(6);
    let initiator_classical_secret = derive_pqxdh_classical_initiator_secret(
        &alice.identity,
        &alice.identity_secret,
        &alice_ephemeral,
        &bob_prekeys.bundle,
    )
    .unwrap();
    let mlkem1024 =
        encapsulate_ml_kem_1024(&bob_prekeys.bundle.one_time_mlkem1024_prekey.public_key).unwrap();
    let session_id = derive_session_id(
        &alice.identity,
        &bob.identity,
        &alice_ephemeral.public_key(),
        "spk-vector",
        &bob_prekeys.bundle.signed_prekey.public_key,
        Some("otpk-vector"),
        bob_prekeys
            .bundle
            .one_time_prekey
            .as_ref()
            .map(|record| record.public_key.as_slice()),
        "pqotpk-vector",
        &bob_prekeys.bundle.one_time_mlkem1024_prekey.public_key,
        &mlkem1024.ciphertext,
        "sha256:vector-tree-head",
    )
    .unwrap();
    let mut intro = SecureMeshPairwiseSessionIntro {
        protocol_version: SECURE_MESH_PROTOCOL_VERSION.to_string(),
        cipher_suite: SECURE_MESH_PAIRWISE_CIPHER_SUITE.to_string(),
        session_id: session_id.clone(),
        initiator_endpoint_id: alice.identity.endpoint_id.clone(),
        responder_endpoint_id: bob.identity.endpoint_id.clone(),
        initiator_identity_public_key: alice.identity.identity_public_key.to_vec(),
        initiator_ephemeral_public_key: alice_ephemeral.public_key().to_vec(),
        initiator_initial_ratchet_public_key: alice_ratchet.public_key().to_vec(),
        responder_signed_prekey_id: "spk-vector".to_string(),
        responder_one_time_prekey_id: Some("otpk-vector".to_string()),
        responder_one_time_mlkem1024_prekey_id: "pqotpk-vector".to_string(),
        mlkem1024_ciphertext: mlkem1024.ciphertext.clone(),
        directory_authorization_digest: "42".repeat(32),
        initiator_capability_proof: sign_capability_proof(
            &alice.identity,
            &alice.signing_key,
            &secure_mesh_pairwise_test_capability_evaluation().unwrap(),
            &capability_proof_request([0x42; 32], handshake_now()).unwrap(),
        )
        .unwrap(),
        initiator_signature: String::new(),
    };
    intro.initiator_signature = sign_pairwise_transcript(
        &alice.signing_key,
        &intro_signature_payload(&intro).unwrap(),
    );
    let responder_classical_secret = derive_pqxdh_classical_responder_secret(
        &bob.identity_secret,
        &bob_prekeys.signed_secret,
        Some(&bob_prekeys.one_time_secret),
        &intro,
    )
    .unwrap();
    assert_eq!(
        initiator_classical_secret.as_slice(),
        responder_classical_secret.as_slice()
    );
    let responder_mlkem1024_secret = decapsulate_ml_kem_1024(
        &bob_prekeys.one_time_mlkem1024_seed,
        &bob_prekeys.bundle.one_time_mlkem1024_prekey.public_key,
        &intro.mlkem1024_ciphertext,
    )
    .unwrap();
    let initiator_triple_secrets = derive_triple_ratchet_initial_secrets(
        initiator_classical_secret.as_slice(),
        mlkem1024.shared_secret(),
        &alice.identity.identity_public_key,
        &bob.identity.identity_public_key,
        session_id.as_bytes(),
    )
    .unwrap();
    let responder_triple_secrets = derive_triple_ratchet_initial_secrets(
        responder_classical_secret.as_slice(),
        &responder_mlkem1024_secret,
        &alice.identity.identity_public_key,
        &bob.identity.identity_public_key,
        session_id.as_bytes(),
    )
    .unwrap();
    assert_eq!(
        initiator_triple_secrets.ec_secret(),
        responder_triple_secrets.ec_secret()
    );
    assert_eq!(
        initiator_triple_secrets.scka_secret(),
        responder_triple_secrets.scka_secret()
    );
    assert_ne!(
        initiator_triple_secrets.ec_secret(),
        initiator_triple_secrets.scka_secret()
    );
}

#[test]
fn secure_mesh_pairwise_triple_ratchet_combines_ec_and_sparse_pq_messages() {
    let alice_ratchet = fixed_pairwise_key(34);
    let bob_ratchet = fixed_pairwise_key(35);
    let mut alice_session = deterministic_pairwise_session(
        "pairwise-vector-session",
        "desktop_gui:alice-vector",
        "mobile:bob-vector",
        "desktop_gui:alice-vector",
        "mobile:bob-vector",
        [33u8; PUBLIC_KEY_LEN],
        alice_ratchet.clone(),
        bob_ratchet.public_key(),
    )
    .unwrap();
    let mut bob_session = deterministic_pairwise_session(
        "pairwise-vector-session",
        "mobile:bob-vector",
        "desktop_gui:alice-vector",
        "desktop_gui:alice-vector",
        "mobile:bob-vector",
        [33u8; PUBLIC_KEY_LEN],
        bob_ratchet,
        alice_ratchet.public_key(),
    )
    .unwrap();
    let first = alice_session
        .seal_message_with_nonce(
            "msg-vector-1",
            b"pairwise deterministic vector",
            [44u8; NONCE_LEN],
        )
        .unwrap();
    assert_eq!(first.dh_epoch, 0);
    assert_eq!(first.chain_index, 0);
    assert_eq!(first.previous_chain_length, 0);
    assert_eq!(first.sparse_pq_header.message_number, 1);
    assert!(!message_aad(&first).unwrap().is_empty());
    assert_eq!(first.encrypted_header, "LCwsLCwsLCwsLCws");
    assert_eq!(first.ciphertext_size, 45);
    assert!(!first.ciphertext.contains("pairwise deterministic vector"));
    let opened_first = bob_session.open_message(&first).unwrap();
    assert_eq!(opened_first.body, b"pairwise deterministic vector");

    alice_session
        .rotate_sending_ratchet_with_secret(fixed_pairwise_key(36))
        .unwrap();
    let after_ratchet = alice_session
        .seal_message_with_nonce("msg-vector-2", b"dh ratchet vector", [45u8; NONCE_LEN])
        .unwrap();
    assert_eq!(after_ratchet.dh_epoch, 1);
    assert_eq!(after_ratchet.chain_index, 0);
    assert_eq!(after_ratchet.previous_chain_length, 1);
    assert_eq!(after_ratchet.sparse_pq_header.message_number, 2);
    assert_eq!(after_ratchet.encrypted_header, "LS0tLS0tLS0tLS0t");
    assert_eq!(after_ratchet.ciphertext_size, 33);
    let opened_after_ratchet = bob_session.open_message(&after_ratchet).unwrap();
    assert_eq!(opened_after_ratchet.body, b"dh ratchet vector");
}

#[test]
fn secure_mesh_pairwise_wire_profile_ignores_app_version_and_rejects_revision_mismatch() {
    let simulated_app_versions = ["0.0.1-alpha", "0.0.2", "27.4.9"];
    let digests = simulated_app_versions
        .iter()
        .map(|_| secure_mesh_pairwise_build_protocol_digest().unwrap())
        .collect::<Vec<_>>();
    assert!(digests.windows(2).all(|pair| pair[0] == pair[1]));

    let endpoint = endpoint("desktop_gui:wire-profile-revision");
    for incompatible_revision in [
        SECURE_MESH_PROTOCOL_BUILD_REVISION - 1,
        SECURE_MESH_PROTOCOL_BUILD_REVISION + 1,
    ] {
        let incompatible_digest =
            secure_mesh_pairwise_build_protocol_digest_for_revision(incompatible_revision).unwrap();
        assert_ne!(digests[0], incompatible_digest);
        let request = CapabilityProofRequest {
            build_protocol_digest: incompatible_digest,
            policy_revision: SECURE_MESH_PAIRWISE_CAPABILITY_POLICY_REVISION,
            challenge: [0x6d; 32],
            issued_at_unix_seconds: handshake_now().unix_timestamp() - 1,
            expires_at_unix_seconds: handshake_now().unix_timestamp() + 60,
        };
        let proof = sign_capability_proof(
            &endpoint.identity,
            &endpoint.signing_key,
            &secure_mesh_pairwise_test_capability_evaluation().unwrap(),
            &request,
        )
        .unwrap();
        let context = CapabilityProofVerificationContext {
            expected_build_protocol_digest: digests[0].clone(),
            expected_policy_revision: SECURE_MESH_PAIRWISE_CAPABILITY_POLICY_REVISION,
            expected_challenge: request.challenge,
            now_unix_seconds: handshake_now().unix_timestamp(),
        };
        let error = crate::core::secure_mesh_capability_proof::verify_capability_proof(
            &endpoint.identity,
            &proof,
            &context,
        )
        .unwrap_err();
        assert!(error.to_string().contains("build protocol binding"));
    }
}
