use super::test_support::*;

#[test]
fn secure_mesh_pairwise_durable_store_commits_reopens_and_rejects_stale_cas() {
    let store_path = durable_store_path("commit");
    let _ = std::fs::remove_file(&store_path);
    let (mut alice_session, mut bob_session) = pairwise_sessions();
    let secret_store = test_secret_store();
    let mut store = open_test_durable_store(&store_path, Arc::clone(&secret_store), "commit");
    let initial = store
        .upsert_initial(&alice_session, "2026-06-26T00:03:00Z")
        .unwrap();
    let initial_snapshot = stored_snapshot_json(
        &store_path,
        &alice_session.session_id,
        &alice_session.local_endpoint_id,
    );
    let initial_public: PersistedPairwisePublicSession =
        serde_json::from_str(&initial_snapshot).unwrap();
    let initial_handle = SecretStoreHandle::new(
        initial_public.secret_store_namespace.clone(),
        initial_public.secret_store_key.clone(),
    )
    .unwrap();
    assert!(secret_store.get_secret(&initial_handle).unwrap().is_some());
    let message = alice_session.seal_message("msg-1", b"persist me").unwrap();
    assert_eq!(
        bob_session.open_message(&message).unwrap().body,
        b"persist me"
    );
    let committed = store
        .commit_session(&initial, &alice_session, "2026-06-26T00:03:01Z")
        .unwrap();
    assert_eq!(committed.state_version, 2);
    assert_eq!(committed.sent_count, 1);
    assert!(secret_store.get_secret(&initial_handle).unwrap().is_none());

    drop(store);
    let reopened = open_test_durable_store(&store_path, Arc::clone(&secret_store), "commit");
    let restored = reopened
        .load_session(&alice_session.session_id, &alice_session.local_endpoint_id)
        .unwrap()
        .unwrap();
    assert_eq!(restored.sent_count(), 1);
    assert_eq!(restored.session_id, alice_session.session_id);

    let mut reopened_mut =
        open_test_durable_store(&store_path, Arc::clone(&secret_store), "commit");
    let stale_error = reopened_mut
        .commit_session(&initial, &alice_session, "2026-06-26T00:03:02Z")
        .unwrap_err();
    assert!(stale_error.to_string().contains("compare-and-swap failed"));
    let winner = reopened_mut
        .load_session(&alice_session.session_id, &alice_session.local_endpoint_id)
        .unwrap()
        .unwrap();
    assert_eq!(winner.sent_count(), 1);
    assert_eq!(winner.dh_epoch(), 1);
    let _ = std::fs::remove_file(&store_path);
}

#[test]
fn secure_mesh_pairwise_failed_secret_deletion_is_queued_and_retried() {
    let store_path = durable_store_path("secret-cleanup-retry");
    let _ = std::fs::remove_file(&store_path);
    let (mut alice_session, _) = pairwise_sessions();
    let secret_store = Arc::new(FailOnceDeleteSecretStore::new());
    let secret_store_trait: Arc<dyn SecureMeshSecretStore> = secret_store;
    let mut store =
        open_test_durable_store(&store_path, secret_store_trait, "secret-cleanup-retry");
    let initial = store
        .upsert_initial(&alice_session, "2026-06-26T00:04:00Z")
        .unwrap();
    alice_session
        .seal_message("msg-secret-cleanup-retry", b"advance snapshot")
        .unwrap();

    let cleanup_error = store
        .commit_session(&initial, &alice_session, "2026-06-26T00:04:01Z")
        .unwrap_err();
    assert!(cleanup_error.to_string().contains("cleanup is incomplete"));
    assert_eq!(store.pending_secret_cleanup_count().unwrap(), 1);
    let committed = store
        .read_record(&alice_session.session_id, &alice_session.local_endpoint_id)
        .unwrap()
        .unwrap();
    assert_eq!(committed.state_version, 2);

    assert_eq!(store.retry_pending_secret_cleanup().unwrap(), 1);
    assert_eq!(store.pending_secret_cleanup_count().unwrap(), 0);
    assert!(
        store
            .load_session(&alice_session.session_id, &alice_session.local_endpoint_id)
            .unwrap()
            .is_some()
    );
    let _ = std::fs::remove_file(&store_path);
}

#[test]
fn secure_mesh_pairwise_durable_store_rejects_stale_receive_snapshot_with_current_record() {
    let store_path = durable_store_path("receive-rollback");
    let _ = std::fs::remove_file(&store_path);
    let (mut alice_session, mut bob_session) = pairwise_sessions();
    let stale_bob_session = bob_session.clone();
    let secret_store = test_secret_store();
    let mut store =
        open_test_durable_store(&store_path, Arc::clone(&secret_store), "receive-rollback");
    let initial = store
        .upsert_initial(&bob_session, "2026-06-26T00:05:00Z")
        .unwrap();
    let message = alice_session
        .seal_message("msg-receive-rollback", b"receive once")
        .unwrap();
    assert_eq!(
        bob_session.open_message(&message).unwrap().body,
        b"receive once"
    );
    let committed = store
        .commit_session(&initial, &bob_session, "2026-06-26T00:05:01Z")
        .unwrap();

    let rollback = store
        .commit_session(&committed, &stale_bob_session, "2026-06-26T00:05:02Z")
        .unwrap_err();

    assert!(rollback.to_string().contains("durable rollback detected"));
    let _ = std::fs::remove_file(&store_path);
}

#[test]
fn secure_mesh_pairwise_durable_store_rejects_skipped_key_replay_window_rollback() {
    let store_path = durable_store_path("skipped-rollback");
    let _ = std::fs::remove_file(&store_path);
    let (mut alice_session, mut bob_session) = pairwise_sessions();
    let secret_store = test_secret_store();
    let mut store =
        open_test_durable_store(&store_path, Arc::clone(&secret_store), "skipped-rollback");
    let initial = store
        .upsert_initial(&bob_session, "2026-06-26T00:05:10Z")
        .unwrap();
    let first = alice_session
        .seal_message("msg-skipped-rollback-1", b"first")
        .unwrap();
    let second = alice_session
        .seal_message("msg-skipped-rollback-2", b"second")
        .unwrap();
    assert_eq!(bob_session.open_message(&second).unwrap().body, b"second");
    assert_eq!(bob_session.skipped_key_count(), 1);
    let stale_with_skipped_key = bob_session.clone();
    let committed_second = store
        .commit_session(&initial, &bob_session, "2026-06-26T00:05:11Z")
        .unwrap();
    assert_eq!(bob_session.open_message(&first).unwrap().body, b"first");
    assert_eq!(bob_session.skipped_key_count(), 0);
    let committed_first = store
        .commit_session(&committed_second, &bob_session, "2026-06-26T00:05:12Z")
        .unwrap();

    let rollback = store
        .commit_session(
            &committed_first,
            &stale_with_skipped_key,
            "2026-06-26T00:05:13Z",
        )
        .unwrap_err();

    assert!(
        rollback
            .to_string()
            .contains("replay cache rollback detected")
            || rollback
                .to_string()
                .contains("skipped-key rollback detected")
    );
    let _ = std::fs::remove_file(&store_path);
}

#[test]
fn secure_mesh_pairwise_durable_store_marks_revoked_and_blocks_commit() {
    let store_path = durable_store_path("revoke");
    let _ = std::fs::remove_file(&store_path);
    let (mut alice_session, _) = pairwise_sessions();
    let secret_store = test_secret_store();
    let mut store = open_test_durable_store(&store_path, Arc::clone(&secret_store), "revoke");
    let initial = store
        .upsert_initial(&alice_session, "2026-06-26T00:04:00Z")
        .unwrap();
    let initial_snapshot = stored_snapshot_json(
        &store_path,
        &alice_session.session_id,
        &alice_session.local_endpoint_id,
    );
    let initial_public: PersistedPairwisePublicSession =
        serde_json::from_str(&initial_snapshot).unwrap();
    let initial_handle = SecretStoreHandle::new(
        initial_public.secret_store_namespace.clone(),
        initial_public.secret_store_key.clone(),
    )
    .unwrap();
    assert!(secret_store.get_secret(&initial_handle).unwrap().is_some());
    let revoked = store
        .mark_revoked(&initial, "2026-06-26T00:04:01Z")
        .unwrap();
    assert!(revoked.revoked_at.is_some());
    assert!(secret_store.get_secret(&initial_handle).unwrap().is_none());
    assert!(
        store
            .load_session(&alice_session.session_id, &alice_session.local_endpoint_id)
            .unwrap()
            .is_none()
    );
    alice_session
        .seal_message("msg-1", b"local state changed")
        .unwrap();
    let commit_error = store
        .commit_session(&revoked, &alice_session, "2026-06-26T00:04:02Z")
        .unwrap_err();
    assert!(
        commit_error
            .to_string()
            .contains("durable session is revoked")
    );
    let _ = std::fs::remove_file(&store_path);
}
