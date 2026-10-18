use super::test_support::*;

#[test]
fn incompatible_pairwise_schema_resets_all_persistent_state() {
    let store_path = durable_store_path("incompatible-schema-reset");
    let _ = std::fs::remove_file(&store_path);
    let secret_store = test_secret_store();
    let (session, _) = pairwise_sessions();
    let prekey_use = SecureMeshRemotePreKeyUse {
        session_id: session.session_id.clone(),
        local_endpoint_id: session.local_endpoint_id.clone(),
        remote_endpoint_id: session.remote_endpoint_id.clone(),
        remote_identity_fingerprint: "sha256:reset-remote-identity".to_string(),
        signed_prekey_id: "spk-reset".to_string(),
        one_time_prekey_id: "otpk-reset".to_string(),
        one_time_prekey_public_key_hash: "sha256:reset-prekey".to_string(),
        one_time_mlkem1024_prekey_id: "pqotpk-reset".to_string(),
        one_time_mlkem1024_prekey_public_key_hash: "sha256:reset-pq-prekey".to_string(),
        directory_authorization_digest: "31".repeat(32),
    };
    {
        let mut store = open_test_durable_store(
            &store_path,
            Arc::clone(&secret_store),
            "incompatible-schema-reset",
        );
        store
            .upsert_initial(&session, "2026-06-26T00:02:40Z")
            .unwrap();
        store
            .record_remote_prekey_use(&prekey_use, "2026-06-26T00:02:41Z")
            .unwrap();
    }
    TestConnection::open(&store_path)
        .unwrap()
        .execute_batch("PRAGMA user_version = 999;")
        .unwrap();

    let mut reset = open_test_durable_store(
        &store_path,
        Arc::clone(&secret_store),
        "incompatible-schema-reset",
    );
    assert!(
        reset
            .read_record(&session.session_id, &session.local_endpoint_id)
            .unwrap()
            .is_none()
    );
    reset
        .record_remote_prekey_use(&prekey_use, "2026-06-26T00:02:42Z")
        .unwrap();
    let schema_version: u32 = TestConnection::open(&store_path)
        .unwrap()
        .query_row("PRAGMA user_version", [], |row| row.get(0))
        .unwrap();
    assert_eq!(schema_version, PAIRWISE_SNAPSHOT_SCHEMA_VERSION);
    let _ = std::fs::remove_file(store_path);
}
