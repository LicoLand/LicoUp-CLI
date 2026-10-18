use super::test_support::*;

#[test]
fn secure_mesh_pairwise_durable_capability_replay_ledger_survives_reopen_and_is_redacted() {
    let store_path = durable_store_path("capability-replay");
    let _ = std::fs::remove_file(&store_path);
    let fixture = handshake_fixture();
    let namespace = "pairwise-test-capability-replay";
    let now = handshake_now().unix_timestamp();
    {
        let mut store = SecureMeshPairwiseDurableStore::open_with_secret_store(
            &store_path,
            test_secret_store(),
            namespace,
        )
        .unwrap();
        store
            .consume_capability_proof_pair(
                &fixture.bob.identity.endpoint_id,
                &fixture.accepted.responder_capability_proof,
                &fixture.intro.initiator_capability_proof,
                now,
            )
            .unwrap();
    }
    let mut reopened = SecureMeshPairwiseDurableStore::open_with_secret_store(
        &store_path,
        test_secret_store(),
        namespace,
    )
    .unwrap();
    let replay = reopened
        .consume_capability_proof_pair(
            &fixture.bob.identity.endpoint_id,
            &fixture.accepted.responder_capability_proof,
            &fixture.intro.initiator_capability_proof,
            now,
        )
        .unwrap_err();
    assert!(replay.to_string().contains("replay rejected"));

    let connection = TestConnection::open(&store_path).unwrap();
    let count: i64 = connection
        .query_row(
            "SELECT COUNT(*) FROM secure_mesh_pairwise_capability_proof_uses",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(count, 2);
    let rows = connection
            .prepare(
                "SELECT local_endpoint_scope_hash, proof_digest, expires_at_unix_seconds FROM secure_mesh_pairwise_capability_proof_uses ORDER BY proof_digest",
            )
            .unwrap()
            .query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, i64>(2)?,
                ))
            })
            .unwrap()
            .collect::<rusqlite::Result<Vec<_>>>()
            .unwrap();
    assert!(rows.iter().all(|(scope, digest, expiry)| {
        scope.len() == 64
            && digest.starts_with("sha256:")
            && *expiry >= now
            && !scope.contains(&fixture.bob.identity.endpoint_id)
            && !digest.contains(&fixture.bob.identity.endpoint_id)
    }));
    drop(connection);
    let database_bytes = std::fs::read(&store_path).unwrap();
    for forbidden in [
        fixture.bob.identity.endpoint_id.as_bytes(),
        fixture.alice.identity.endpoint_id.as_bytes(),
        fixture
            .accepted
            .responder_capability_proof
            .signature
            .as_bytes(),
        fixture
            .intro
            .initiator_capability_proof
            .signature
            .as_bytes(),
    ] {
        assert!(
            !database_bytes
                .windows(forbidden.len())
                .any(|window| window == forbidden)
        );
    }
    let _ = std::fs::remove_file(&store_path);
}

#[test]
fn secure_mesh_pairwise_capability_replay_and_session_commit_are_atomic() {
    let store_path = durable_store_path("capability-session-atomic");
    let _ = std::fs::remove_file(&store_path);
    let fixture = handshake_fixture();
    let mut store = open_test_durable_store(
        &store_path,
        test_secret_store(),
        "capability-session-atomic",
    );
    let mut session = fixture.alice_session;
    let initial = store
        .upsert_initial(&session, "2026-06-26T00:00:01Z")
        .unwrap();
    session
        .complete_initiator_handshake(
            &fixture.alice.identity,
            &fixture.bob.identity,
            &fixture.accepted,
            handshake_now(),
            &mut CapabilityProofReplayGuard::default(),
        )
        .unwrap();
    let authorization = store
        .begin_authorized_session(&SecretStoreAuthorizationRequest::new(
            "pairwise atomic capability commit test",
            6,
        ))
        .unwrap();
    let committed = store
        .commit_session_with_authorized_session_and_capability_proofs(
            &initial,
            &session,
            session.local_capability_proof(),
            &fixture.accepted.responder_capability_proof,
            handshake_now().unix_timestamp(),
            "2026-06-26T00:00:02Z",
            &authorization,
        )
        .unwrap();
    let replay = store
        .commit_session_with_authorized_session_and_capability_proofs(
            &committed,
            &session,
            session.local_capability_proof(),
            &fixture.accepted.responder_capability_proof,
            handshake_now().unix_timestamp(),
            "2026-06-26T00:00:03Z",
            &authorization,
        )
        .unwrap_err();
    assert!(replay.to_string().contains("replay rejected"));
    assert_eq!(
        store
            .read_record(&session.session_id, &session.local_endpoint_id)
            .unwrap()
            .unwrap(),
        committed
    );
    let _ = std::fs::remove_file(store_path);
}

#[test]
fn secure_mesh_pairwise_replay_watermark_rejects_expiry_revival_after_clock_rollback() {
    let store_path = durable_store_path("capability-replay-clock-rollback");
    let _ = std::fs::remove_file(&store_path);
    let alice = endpoint("desktop_gui:watermark-alice");
    let bob = endpoint("mobile:watermark-bob");
    let evaluation = secure_mesh_pairwise_test_capability_evaluation().unwrap();
    let sign = |endpoint: &EndpointFixture,
                challenge: [u8; 32],
                issued_at_unix_seconds: i64,
                expires_at_unix_seconds: i64| {
        sign_capability_proof(
            &endpoint.identity,
            &endpoint.signing_key,
            &evaluation,
            &CapabilityProofRequest {
                build_protocol_digest: secure_mesh_pairwise_build_protocol_digest().unwrap(),
                policy_revision: SECURE_MESH_PAIRWISE_CAPABILITY_POLICY_REVISION,
                challenge,
                issued_at_unix_seconds,
                expires_at_unix_seconds,
            },
        )
        .unwrap()
    };
    let old_first = sign(&alice, [0x31; 32], 900, 1_000);
    let old_second = sign(&bob, [0x32; 32], 900, 1_000);
    let new_first = sign(&alice, [0x41; 32], 2_000, 2_100);
    let new_second = sign(&bob, [0x42; 32], 2_000, 2_100);
    let namespace = "pairwise-test-capability-replay-clock-rollback";
    {
        let mut store = SecureMeshPairwiseDurableStore::open_with_secret_store(
            &store_path,
            test_secret_store(),
            namespace,
        )
        .unwrap();
        store
            .consume_capability_proof_pair(&bob.identity.endpoint_id, &old_first, &old_second, 900)
            .unwrap();
        store
            .consume_capability_proof_pair(
                &bob.identity.endpoint_id,
                &new_first,
                &new_second,
                2_000,
            )
            .unwrap();
    }
    let mut reopened = SecureMeshPairwiseDurableStore::open_with_secret_store(
        &store_path,
        test_secret_store(),
        namespace,
    )
    .unwrap();
    let revived = reopened
        .consume_capability_proof_pair(&bob.identity.endpoint_id, &old_first, &old_second, 950)
        .unwrap_err();
    assert!(revived.to_string().contains("clock rollback"));
    let _ = std::fs::remove_file(store_path);
}
