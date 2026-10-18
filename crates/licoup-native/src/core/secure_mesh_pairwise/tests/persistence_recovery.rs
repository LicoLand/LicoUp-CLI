use super::test_support::*;

#[test]
fn secure_mesh_pairwise_memory_only_restart_purges_unrecoverable_public_session() {
    let store_path = durable_store_path("memory-restart-purge");
    let _ = std::fs::remove_file(&store_path);
    let (alice_session, _) = pairwise_sessions();
    let namespace = durable_store_namespace("memory-restart-purge");
    {
        let mut store = SecureMeshPairwiseDurableStore::open_with_secret_store(
            &store_path,
            Arc::new(EphemeralSecretStore::new()),
            namespace.clone(),
        )
        .unwrap();
        store
            .upsert_initial(&alice_session, "2026-06-26T00:02:00Z")
            .unwrap();
        assert!(
            store
                .read_record(&alice_session.session_id, &alice_session.local_endpoint_id)
                .unwrap()
                .is_some()
        );
    }
    let mut restarted = SecureMeshPairwiseDurableStore::open_with_secret_store(
        &store_path,
        Arc::new(EphemeralSecretStore::new()),
        namespace,
    )
    .unwrap();
    assert_eq!(
        restarted
            .purge_unrecoverable_memory_only_sessions()
            .unwrap(),
        1
    );
    assert!(
        restarted
            .read_record(&alice_session.session_id, &alice_session.local_endpoint_id)
            .unwrap()
            .is_none()
    );
    assert!(
        restarted
            .load_session(&alice_session.session_id, &alice_session.local_endpoint_id)
            .unwrap()
            .is_none()
    );
    let _ = std::fs::remove_file(&store_path);
}

#[test]
fn secure_mesh_pairwise_pending_authenticated_ratchet_survives_restart() {
    let store_path = durable_store_path("pending-authenticated-ratchet");
    let _ = std::fs::remove_file(&store_path);
    let (alice_session, mut bob_session) = pairwise_sessions();
    assert!(alice_session.pending_sending_ratchet());
    let secret_store = test_secret_store();
    let mut store = open_test_durable_store(
        &store_path,
        Arc::clone(&secret_store),
        "pending-authenticated-ratchet",
    );
    let initial = store
        .upsert_initial(&alice_session, "2026-06-26T00:01:00Z")
        .unwrap();
    let pending_snapshot = stored_snapshot_json(
        &store_path,
        &alice_session.session_id,
        &alice_session.local_endpoint_id,
    );
    assert!(pending_snapshot.contains("\"pending_sending_ratchet\":true"));
    assert!(!pending_snapshot.contains("ack_barrier"));
    for forbidden in [
        "root_key",
        "sending_chain_key",
        "receiving_chain_key",
        "sending_header_key",
        "receiving_header_key",
        "next_sending_header_key",
        "next_receiving_header_key",
        "skipped_receiving_header_keys",
        "local_ratchet_secret",
        "pending_ratchet_secret_handle",
        "pending_commit_secret_handle",
    ] {
        assert!(
            !pending_snapshot.contains(forbidden),
            "pending commit snapshot leaked {forbidden}"
        );
    }
    drop(store);

    let mut reopened = open_test_durable_store(
        &store_path,
        Arc::clone(&secret_store),
        "pending-authenticated-ratchet",
    );
    let mut restored = reopened
        .load_session(&alice_session.session_id, &alice_session.local_endpoint_id)
        .unwrap()
        .unwrap();
    assert!(restored.pending_sending_ratchet());
    let first = restored
        .seal_message("msg-pending-authenticated-1", b"restart ratchet")
        .unwrap();
    assert_eq!(first.dh_epoch, 1);
    assert_eq!(first.chain_index, 0);
    let committed = reopened
        .commit_session(&initial, &restored, "2026-06-26T00:01:01Z")
        .unwrap();
    drop(reopened);

    let reopened_after_send = open_test_durable_store(
        &store_path,
        Arc::clone(&secret_store),
        "pending-authenticated-ratchet",
    );
    let restored_after_send = reopened_after_send
        .load_session(&alice_session.session_id, &alice_session.local_endpoint_id)
        .unwrap()
        .unwrap();
    assert_eq!(committed.state_version, 2);
    assert_eq!(restored_after_send.dh_epoch(), 1);
    assert!(!restored_after_send.pending_sending_ratchet());
    assert_eq!(
        bob_session.open_message(&first).unwrap().body,
        b"restart ratchet"
    );
    let _ = std::fs::remove_file(&store_path);
}
