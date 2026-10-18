use super::test_support::*;

#[test]
fn secure_mesh_pairwise_local_prekey_proofs_and_initial_session_are_atomic() {
    let store_path = durable_store_path("prekey-proof-session-atomic");
    let _ = std::fs::remove_file(&store_path);
    let fixture = handshake_fixture();
    let mut store = open_test_durable_store(
        &store_path,
        test_secret_store(),
        "prekey-proof-session-atomic",
    );
    let session = fixture.bob_session;
    let original_claim = SecureMeshLocalPreKeyUse {
        local_endpoint_id: session.local_endpoint_id.clone(),
        local_identity_fingerprint: fixture.bob.identity.fingerprint().unwrap(),
        one_time_prekey_id: "atomic-local-prekey-1".to_string(),
        one_time_prekey_public_key_hash: "sha256:atomic-local-prekey-1".to_string(),
        one_time_mlkem1024_prekey_id: "atomic-local-pq-prekey-1".to_string(),
        one_time_mlkem1024_prekey_public_key_hash: "sha256:atomic-local-pq-prekey-1".to_string(),
    };
    store
        .upsert_initial_with_local_prekey_claim_and_capability_proofs(
            &session,
            &original_claim,
            &fixture.accepted.responder_capability_proof,
            &fixture.intro.initiator_capability_proof,
            handshake_now().unix_timestamp(),
            "2026-06-26T00:00:01Z",
        )
        .unwrap();

    let mut replay_session = session.clone();
    replay_session.session_id.push_str("-replay");
    let replay_claim = SecureMeshLocalPreKeyUse {
        local_endpoint_id: replay_session.local_endpoint_id.clone(),
        local_identity_fingerprint: fixture.bob.identity.fingerprint().unwrap(),
        one_time_prekey_id: "atomic-local-prekey-must-rollback".to_string(),
        one_time_prekey_public_key_hash: "sha256:atomic-local-prekey-must-rollback".to_string(),
        one_time_mlkem1024_prekey_id: "atomic-local-pq-prekey-must-rollback".to_string(),
        one_time_mlkem1024_prekey_public_key_hash: "sha256:atomic-local-pq-prekey-must-rollback"
            .to_string(),
    };
    let replay = store
        .upsert_initial_with_local_prekey_claim_and_capability_proofs(
            &replay_session,
            &replay_claim,
            &fixture.accepted.responder_capability_proof,
            &fixture.intro.initiator_capability_proof,
            handshake_now().unix_timestamp(),
            "2026-06-26T00:00:02Z",
        )
        .unwrap_err();
    assert!(replay.to_string().contains("replay rejected"));
    assert!(
        store
            .read_record(
                &replay_session.session_id,
                &replay_session.local_endpoint_id,
            )
            .unwrap()
            .is_none()
    );
    let connection = TestConnection::open(&store_path).unwrap();
    let rolled_back_prekeys: i64 = connection
            .query_row(
                "SELECT COUNT(*) FROM secure_mesh_pairwise_local_prekey_uses WHERE one_time_prekey_id = ?1",
                params![replay_claim.one_time_prekey_id],
                |row| row.get(0),
            )
            .unwrap();
    assert_eq!(rolled_back_prekeys, 0);
    let _ = std::fs::remove_file(store_path);
}

#[test]
fn secure_mesh_pairwise_remote_prekey_and_initial_session_are_atomic() {
    let store_path = durable_store_path("remote-prekey-session-atomic");
    let _ = std::fs::remove_file(&store_path);
    let (session, _) = pairwise_sessions();
    let mut store = open_test_durable_store(
        &store_path,
        test_secret_store(),
        "remote-prekey-session-atomic",
    );
    let original = SecureMeshRemotePreKeyUse {
        session_id: session.session_id.clone(),
        local_endpoint_id: session.local_endpoint_id.clone(),
        remote_endpoint_id: session.remote_endpoint_id.clone(),
        remote_identity_fingerprint: "sha256:remote-identity-atomic".to_string(),
        signed_prekey_id: "spk-remote-atomic".to_string(),
        one_time_prekey_id: "otpk-remote-atomic".to_string(),
        one_time_prekey_public_key_hash: "sha256:remote-prekey-atomic".to_string(),
        one_time_mlkem1024_prekey_id: "pqotpk-remote-atomic".to_string(),
        one_time_mlkem1024_prekey_public_key_hash: "sha256:remote-pq-prekey-atomic".to_string(),
        directory_authorization_digest: "11".repeat(32),
    };
    store
        .upsert_initial_with_remote_prekey_claim(&session, &original, "2026-06-26T00:00:01Z")
        .unwrap();

    let mut replay_session = session.clone();
    replay_session.session_id.push_str("-replay");
    let mut replay = original.clone();
    replay.session_id = replay_session.session_id.clone();
    let error = store
        .upsert_initial_with_remote_prekey_claim(&replay_session, &replay, "2026-06-26T00:00:02Z")
        .unwrap_err();
    assert!(error.to_string().contains("already used"));
    assert!(
        store
            .read_record(
                &replay_session.session_id,
                &replay_session.local_endpoint_id,
            )
            .unwrap()
            .is_none()
    );
    let _ = std::fs::remove_file(store_path);
}

#[test]
fn secure_mesh_pairwise_local_one_time_prekey_ledger_is_atomic_and_survives_session_purge() {
    let store_path = durable_store_path("local-prekey-ledger");
    let _ = std::fs::remove_file(&store_path);
    let (session, _) = pairwise_sessions();
    let secret_store = test_secret_store();
    let mut store = open_test_durable_store(
        &store_path,
        Arc::clone(&secret_store),
        "local-prekey-ledger",
    );
    let original_claim = SecureMeshLocalPreKeyUse {
        local_endpoint_id: session.local_endpoint_id.clone(),
        local_identity_fingerprint: "sha256:local-identity".to_string(),
        one_time_prekey_id: "otpk-local-1".to_string(),
        one_time_prekey_public_key_hash: "sha256:local-prekey-1".to_string(),
        one_time_mlkem1024_prekey_id: "pqotpk-local-1".to_string(),
        one_time_mlkem1024_prekey_public_key_hash: "sha256:local-pq-prekey-1".to_string(),
    };
    store
        .upsert_initial_with_local_prekey_claim(&session, &original_claim, "2026-06-26T00:02:00Z")
        .unwrap();

    let mut reused_id_session = session.clone();
    reused_id_session.session_id.push_str("-reused-id");
    let mut reused_id_claim = original_claim.clone();
    reused_id_claim.one_time_prekey_public_key_hash = "sha256:different-local-prekey".to_string();
    assert!(
        store
            .upsert_initial_with_local_prekey_claim(
                &reused_id_session,
                &reused_id_claim,
                "2026-06-26T00:02:01Z",
            )
            .unwrap_err()
            .to_string()
            .contains("already consumed")
    );
    assert!(
        store
            .read_record(
                &reused_id_session.session_id,
                &reused_id_session.local_endpoint_id,
            )
            .unwrap()
            .is_none()
    );

    let mut reused_key_session = session.clone();
    reused_key_session.session_id.push_str("-reused-key");
    let mut reused_key_claim = original_claim.clone();
    reused_key_claim.one_time_prekey_id = "otpk-local-2".to_string();
    assert!(
        store
            .upsert_initial_with_local_prekey_claim(
                &reused_key_session,
                &reused_key_claim,
                "2026-06-26T00:02:02Z",
            )
            .unwrap_err()
            .to_string()
            .contains("already consumed")
    );

    store.purge_sessions_preserving_prekey_history().unwrap();
    assert!(
        store
            .read_record(&session.session_id, &session.local_endpoint_id)
            .unwrap()
            .is_none()
    );
    let mut after_purge_session = session.clone();
    after_purge_session.session_id.push_str("-after-purge");
    assert!(
        store
            .upsert_initial_with_local_prekey_claim(
                &after_purge_session,
                &original_claim,
                "2026-06-26T00:02:03Z",
            )
            .unwrap_err()
            .to_string()
            .contains("already consumed")
    );
    let _ = std::fs::remove_file(&store_path);
}
#[test]
fn secure_mesh_pairwise_remote_one_time_prekey_reuse_ignores_authorization_digest_changes() {
    let store_path = durable_store_path("remote-prekey-ledger");
    let _ = std::fs::remove_file(&store_path);
    let secret_store = test_secret_store();
    let mut store = open_test_durable_store(
        &store_path,
        Arc::clone(&secret_store),
        "remote-prekey-ledger",
    );
    let original = SecureMeshRemotePreKeyUse {
        session_id: "session-remote-prekey-1".to_string(),
        local_endpoint_id: "mobile:local".to_string(),
        remote_endpoint_id: "desktop_gui:remote".to_string(),
        remote_identity_fingerprint: "sha256:remote-identity".to_string(),
        signed_prekey_id: "spk-remote-1".to_string(),
        one_time_prekey_id: "otpk-remote-1".to_string(),
        one_time_prekey_public_key_hash: "sha256:remote-prekey-1".to_string(),
        one_time_mlkem1024_prekey_id: "pqotpk-remote-1".to_string(),
        one_time_mlkem1024_prekey_public_key_hash: "sha256:remote-pq-prekey-1".to_string(),
        directory_authorization_digest: "21".repeat(32),
    };
    store
        .record_remote_prekey_use(&original, "2026-06-26T00:02:10Z")
        .unwrap();

    let mut reused_id = original.clone();
    reused_id.session_id = "session-remote-prekey-2".to_string();
    reused_id.one_time_prekey_public_key_hash = "sha256:different-remote-prekey".to_string();
    reused_id.directory_authorization_digest = "22".repeat(32);
    assert!(
        store
            .record_remote_prekey_use(&reused_id, "2026-06-26T00:02:11Z")
            .unwrap_err()
            .to_string()
            .contains("already used")
    );

    let mut reused_key = original.clone();
    reused_key.session_id = "session-remote-prekey-3".to_string();
    reused_key.one_time_prekey_id = "otpk-remote-2".to_string();
    reused_key.directory_authorization_digest = "23".repeat(32);
    assert!(
        store
            .record_remote_prekey_use(&reused_key, "2026-06-26T00:02:12Z")
            .unwrap_err()
            .to_string()
            .contains("already used")
    );

    store.purge_sessions_preserving_prekey_history().unwrap();
    assert!(
        store
            .record_remote_prekey_use(&original, "2026-06-26T00:02:13Z")
            .unwrap_err()
            .to_string()
            .contains("already used")
    );
    let _ = std::fs::remove_file(&store_path);
}
