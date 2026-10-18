use super::test_support::*;

#[test]
fn secure_mesh_pairwise_durable_store_keeps_secret_material_out_of_sqlite_snapshot() {
    let store_path = durable_store_path("redacted-snapshot");
    let _ = std::fs::remove_file(&store_path);
    let (mut alice_session, _) = pairwise_sessions();
    let initial_root_key = encode_secret(&alice_session.root_key);
    let initial_sending_chain_key = encode_secret(&alice_session.sending_chain_key);
    let initial_receiving_chain_key = encode_secret(&alice_session.receiving_chain_key);
    let initial_local_ratchet_secret =
        encode_secret(&alice_session.local_ratchet_secret.to_bytes());
    let secret_store = test_secret_store();
    let mut store =
        open_test_durable_store(&store_path, Arc::clone(&secret_store), "redacted-snapshot");
    let initial = store
        .upsert_initial(&alice_session, "2026-06-26T00:02:00Z")
        .unwrap();
    let initial_snapshot = stored_snapshot_json(
        &store_path,
        &alice_session.session_id,
        &alice_session.local_endpoint_id,
    );
    assert!(initial_snapshot.contains("secret_store_key"));
    for forbidden_field in [
        "root_key",
        "sending_chain_key",
        "receiving_chain_key",
        "sending_header_key",
        "receiving_header_key",
        "next_sending_header_key",
        "next_receiving_header_key",
        "skipped_receiving_header_keys",
        "local_ratchet_secret",
        "message_key",
    ] {
        assert!(!initial_snapshot.contains(forbidden_field));
    }
    for forbidden_value in [
        initial_root_key.as_str(),
        initial_sending_chain_key.as_str(),
        initial_receiving_chain_key.as_str(),
        initial_local_ratchet_secret.as_str(),
    ] {
        assert!(!initial_snapshot.contains(forbidden_value));
    }
    let initial_public: PersistedPairwisePublicSession =
        serde_json::from_str(&initial_snapshot).unwrap();
    let initial_handle = SecretStoreHandle::new(
        initial_public.secret_store_namespace.clone(),
        initial_public.secret_store_key.clone(),
    )
    .unwrap();
    assert!(secret_store.get_secret(&initial_handle).unwrap().is_some());

    alice_session.seal_message("msg-1", b"persist me").unwrap();
    let committed_sending_chain_key = encode_secret(&alice_session.sending_chain_key);
    store
        .commit_session(&initial, &alice_session, "2026-06-26T00:02:01Z")
        .unwrap();
    let committed_snapshot = stored_snapshot_json(
        &store_path,
        &alice_session.session_id,
        &alice_session.local_endpoint_id,
    );
    assert!(!committed_snapshot.contains("sending_chain_key"));
    assert!(!committed_snapshot.contains(committed_sending_chain_key.as_str()));
    assert!(secret_store.get_secret(&initial_handle).unwrap().is_none());
    let _ = std::fs::remove_file(&store_path);
}

#[test]
fn secure_mesh_pairwise_durable_store_keeps_skipped_message_keys_out_of_sqlite_snapshot() {
    let store_path = durable_store_path("redacted-skipped-key");
    let _ = std::fs::remove_file(&store_path);
    let (mut alice_session, mut bob_session) = pairwise_sessions();
    let first = alice_session.seal_message("msg-skipped-1", b"one").unwrap();
    let second = alice_session.seal_message("msg-skipped-2", b"two").unwrap();
    let opened_second = bob_session.open_message(&second).unwrap();
    assert_eq!(opened_second.body, b"two");
    assert_eq!(bob_session.skipped_key_count(), 1);
    let skipped_message_key = encode_secret(&bob_session.skipped_keys[0].message_key);

    let secret_store = test_secret_store();
    let mut store = open_test_durable_store(
        &store_path,
        Arc::clone(&secret_store),
        "redacted-skipped-key",
    );
    store
        .upsert_initial(&bob_session, "2026-06-26T00:02:15Z")
        .unwrap();
    let snapshot = stored_snapshot_json(
        &store_path,
        &bob_session.session_id,
        &bob_session.local_endpoint_id,
    );
    assert!(snapshot.contains("skipped_keys"));
    assert!(snapshot.contains("secret_store_key"));
    assert!(!snapshot.contains("message_key"));
    assert!(!snapshot.contains(skipped_message_key.as_str()));
    let public: PersistedPairwisePublicSession = serde_json::from_str(&snapshot).unwrap();
    assert_eq!(public.skipped_keys.len(), 1);
    let handle =
        SecretStoreHandle::new(public.secret_store_namespace, public.secret_store_key).unwrap();
    assert!(secret_store.get_secret(&handle).unwrap().is_some());

    drop(store);
    let reopened = open_test_durable_store(
        &store_path,
        Arc::clone(&secret_store),
        "redacted-skipped-key",
    );
    let mut restored = reopened
        .load_session(&bob_session.session_id, &bob_session.local_endpoint_id)
        .unwrap()
        .unwrap();
    let opened_first = restored.open_message(&first).unwrap();
    assert_eq!(opened_first.body, b"one");
    assert_eq!(restored.skipped_key_count(), 0);
    let _ = std::fs::remove_file(&store_path);
}
