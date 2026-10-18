use super::support::*;

fn open_durable_store(path: &std::path::Path) -> SecureMeshMlsDurableStore {
    SecureMeshMlsDurableStore::open_with_path_hardener(path, |_| Ok(())).unwrap()
}

#[test]
fn secure_mesh_mls_durable_store_commits_epoch_with_compare_and_swap() {
    let store_path = durable_store_path("commit-cas");
    let _ = std::fs::remove_file(&store_path);
    let alice = SecureMeshMlsParticipant::new(b"desktop_gui:alice".to_vec()).unwrap();
    let bob = SecureMeshMlsParticipant::new(b"mobile:bob".to_vec()).unwrap();
    let bob_key_package = bob.generate_key_package().unwrap();
    let mut alice_group =
        SecureMeshMlsGroup::create(&alice, b"secure-mesh-durable-cas-group").unwrap();
    let mut store = open_durable_store(&store_path);

    let initial_metadata = alice_group.public_metadata("desktop_gui:alice").unwrap();
    let initial = store
        .upsert_initial(&initial_metadata, "2026-06-26T00:00:00Z")
        .unwrap();
    assert_eq!(initial.epoch, initial_metadata.epoch);
    assert_eq!(initial.state_version, 1);
    assert!(initial.revoked_at_epoch.is_none());

    let welcome = alice_group.add_member(&alice, &bob_key_package).unwrap();
    assert!(!welcome.commit_message.is_empty());
    let committed_metadata = alice_group.public_metadata("desktop_gui:alice").unwrap();
    assert!(committed_metadata.epoch > initial.epoch);
    let committed = store
        .commit_epoch(&initial, &committed_metadata, "2026-06-26T00:00:01Z")
        .unwrap();
    assert_eq!(committed.state_version, initial.state_version + 1);
    assert_eq!(committed.epoch, committed_metadata.epoch);
    assert_eq!(committed.member_count, 2);

    drop(store);
    let reopened = open_durable_store(&store_path);
    let persisted = reopened
        .read(&committed.group_id_hash, &committed.participant_endpoint_id)
        .unwrap()
        .unwrap();
    assert_eq!(persisted, committed);
    let _ = std::fs::remove_file(&store_path);
}

#[test]
fn secure_mesh_mls_old_public_state_schema_requires_authenticated_snapshot_reconciliation() {
    let store_path = durable_store_path("authenticated-schema-reconciliation");
    let _ = std::fs::remove_file(&store_path);
    let alice = SecureMeshMlsParticipant::new(b"desktop_gui:schema-alice".to_vec()).unwrap();
    let matching_group = SecureMeshMlsGroup::create(&alice, b"schema-matching-group").unwrap();
    let divergent_group = SecureMeshMlsGroup::create(&alice, b"schema-divergent-group").unwrap();
    let matching = matching_group
        .public_metadata("desktop_gui:schema-alice")
        .unwrap();
    let divergent = divergent_group
        .public_metadata("desktop_gui:schema-alice")
        .unwrap();

    let connection = Connection::open(&store_path).unwrap();
    connection
        .execute_batch(
            r#"
            CREATE TABLE secure_mesh_mls_group_state (
                group_id_hash TEXT NOT NULL,
                participant_endpoint_id TEXT NOT NULL,
                epoch INTEGER NOT NULL,
                state_version INTEGER NOT NULL,
                member_count INTEGER NOT NULL,
                own_leaf_index INTEGER NOT NULL,
                active INTEGER NOT NULL,
                revoked_at_epoch INTEGER,
                updated_at TEXT NOT NULL,
                PRIMARY KEY (group_id_hash, participant_endpoint_id)
            );
            "#,
        )
        .unwrap();
    connection
        .execute(
            r#"
            INSERT INTO secure_mesh_mls_group_state (
                group_id_hash, participant_endpoint_id, epoch, state_version,
                member_count, own_leaf_index, active, revoked_at_epoch, updated_at
            ) VALUES (?1, ?2, ?3, 4, ?4, ?5, ?6, NULL, '2026-06-26T00:03:00Z')
            "#,
            params![
                matching.group_id_hash,
                matching.participant_endpoint_id,
                matching.epoch as i64,
                matching.member_count as i64,
                matching.own_leaf_index as i64,
                i64::from(matching.active),
            ],
        )
        .unwrap();
    connection
        .execute(
            r#"
            INSERT INTO secure_mesh_mls_group_state (
                group_id_hash, participant_endpoint_id, epoch, state_version,
                member_count, own_leaf_index, active, revoked_at_epoch, updated_at
            ) VALUES (?1, ?2, ?3, 9, ?4, ?5, ?6, NULL, '2026-06-26T00:03:01Z')
            "#,
            params![
                divergent.group_id_hash,
                divergent.participant_endpoint_id,
                divergent.epoch.saturating_add(1) as i64,
                divergent.member_count as i64,
                divergent.own_leaf_index as i64,
                i64::from(divergent.active),
            ],
        )
        .unwrap();
    drop(connection);

    let mut store = open_durable_store(&store_path);
    let pending = store
        .read(&matching.group_id_hash, &matching.participant_endpoint_id)
        .unwrap()
        .unwrap();
    assert_eq!(
        pending.public_state_digest,
        MLS_PUBLIC_STATE_DIGEST_AUTHENTICATED_BACKFILL
    );
    let reconciled = store
        .reconcile_authenticated_snapshot(&matching, "2026-06-26T00:03:02Z")
        .unwrap();
    assert_eq!(reconciled.public_state_digest, matching.public_state_digest);
    assert_eq!(reconciled.state_version, 5);

    let error = store
        .reconcile_authenticated_snapshot(&divergent, "2026-06-26T00:03:03Z")
        .unwrap_err();
    assert!(error.to_string().contains("cannot authenticate"));
    let still_pending = store
        .read(&divergent.group_id_hash, &divergent.participant_endpoint_id)
        .unwrap()
        .unwrap();
    assert_eq!(
        still_pending.public_state_digest,
        MLS_PUBLIC_STATE_DIGEST_AUTHENTICATED_BACKFILL
    );
    let _ = std::fs::remove_file(&store_path);
}

#[test]
fn secure_mesh_mls_durable_store_rejects_rollback_and_stale_commit() {
    let store_path = durable_store_path("rollback-stale");
    let _ = std::fs::remove_file(&store_path);
    let alice = SecureMeshMlsParticipant::new(b"desktop_gui:alice".to_vec()).unwrap();
    let bob = SecureMeshMlsParticipant::new(b"mobile:bob".to_vec()).unwrap();
    let bob_key_package = bob.generate_key_package().unwrap();
    let mut alice_group =
        SecureMeshMlsGroup::create(&alice, b"secure-mesh-durable-rollback-group").unwrap();
    let mut store = open_durable_store(&store_path);

    let initial = store
        .upsert_initial(
            &alice_group.public_metadata("desktop_gui:alice").unwrap(),
            "2026-06-26T00:01:00Z",
        )
        .unwrap();
    let _welcome = alice_group.add_member(&alice, &bob_key_package).unwrap();
    let committed_metadata = alice_group.public_metadata("desktop_gui:alice").unwrap();
    let committed = store
        .commit_epoch(&initial, &committed_metadata, "2026-06-26T00:01:01Z")
        .unwrap();

    let mut rollback_metadata = committed_metadata.clone();
    rollback_metadata.epoch = committed.epoch - 1;
    let rollback_error = store
        .commit_epoch(&committed, &rollback_metadata, "2026-06-26T00:01:02Z")
        .unwrap_err();
    assert!(
        rollback_error
            .to_string()
            .contains("must strictly advance the epoch")
    );

    let update_commit = alice_group.self_update(&alice).unwrap();
    assert!(!update_commit.is_empty());
    let stale_metadata = alice_group.public_metadata("desktop_gui:alice").unwrap();
    assert!(stale_metadata.epoch > committed.epoch);
    let stale_error = store
        .commit_epoch(&initial, &stale_metadata, "2026-06-26T00:01:03Z")
        .unwrap_err();
    assert!(stale_error.to_string().contains("compare-and-swap failed"));
    let _ = std::fs::remove_file(&store_path);
}

#[test]
fn secure_mesh_mls_durable_store_marks_revoked_and_blocks_future_commit() {
    let store_path = durable_store_path("revoke");
    let _ = std::fs::remove_file(&store_path);
    let alice = SecureMeshMlsParticipant::new(b"desktop_gui:alice".to_vec()).unwrap();
    let mut alice_group =
        SecureMeshMlsGroup::create(&alice, b"secure-mesh-durable-revoke-group").unwrap();
    let mut store = open_durable_store(&store_path);

    let initial = store
        .upsert_initial(
            &alice_group.public_metadata("desktop_gui:alice").unwrap(),
            "2026-06-26T00:02:00Z",
        )
        .unwrap();
    let revoked = store
        .mark_revoked(&initial, initial.epoch, "2026-06-26T00:02:01Z")
        .unwrap();
    assert_eq!(revoked.revoked_at_epoch, Some(initial.epoch));
    assert!(!revoked.active);

    let update_commit = alice_group.self_update(&alice).unwrap();
    assert!(!update_commit.is_empty());
    let next_metadata = alice_group.public_metadata("desktop_gui:alice").unwrap();
    let commit_after_revoke = store
        .commit_epoch(&revoked, &next_metadata, "2026-06-26T00:02:02Z")
        .unwrap_err();
    assert!(
        commit_after_revoke
            .to_string()
            .contains("durable record is revoked")
    );
    let _ = std::fs::remove_file(&store_path);
}
