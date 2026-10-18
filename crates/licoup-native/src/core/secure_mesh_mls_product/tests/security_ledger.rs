use super::support::*;

#[test]
fn secure_mesh_mls_journal_recovers_every_action_at_every_cross_store_boundary() {
    let local = device("desktop_gui:journal-recovery-local");
    let participant_scope = local.identity.fingerprint().unwrap();
    let actions = [
        "secure_mesh.mls.member.add",
        "secure_mesh.mls.member.remove",
        "secure_mesh.mls.group.join",
        "secure_mesh.mls.commit.process",
    ];
    let boundaries = [
        "after_stage_before_snapshot",
        "after_snapshot_before_crypto_commit",
        "after_crypto_commit_before_metadata",
        "after_metadata_before_delivery",
    ];

    for action in actions {
        for boundary in boundaries {
            let path = ledger_path(&format!("journal-{action}-{boundary}"));
            let group_id = format!("group-{action}-{boundary}").into_bytes();
            let base = (action != "secure_mesh.mls.group.join")
                .then(|| journal_metadata(&group_id, &participant_scope, 1, "base"));
            let expected = journal_metadata(&group_id, &participant_scope, 2, "expected");
            let operation_id = hex_sha256(format!("{action}:{boundary}:operation").as_bytes());
            let request_digest = hex_sha256(format!("{action}:{boundary}:request").as_bytes());
            let now = capability_now().unix_timestamp();
            let prepared = empty_prepared_security_inputs(&local.identity, now).unwrap();
            let response = serde_json::json!({"ok": true, "action": action});

            let mut ledger = SecureMeshMlsSecurityLedger::open(&path).unwrap();
            ledger
                .begin_operation(&operation_id, action, &request_digest, &local.identity, now)
                .unwrap();
            ledger
                .stage_operation(
                    &operation_id,
                    &serde_json::json!({}),
                    &group_id,
                    base.as_ref(),
                    &expected,
                    &prepared,
                    now,
                )
                .unwrap();

            match boundary {
                "after_stage_before_snapshot" => {}
                "after_snapshot_before_crypto_commit" => {}
                "after_crypto_commit_before_metadata" => {
                    ledger
                        .commit_operation_crypto(&operation_id, &expected, now + 1)
                        .unwrap();
                }
                "after_metadata_before_delivery" => {
                    ledger
                        .commit_operation_crypto(&operation_id, &expected, now + 1)
                        .unwrap();
                    ledger
                        .mark_operation_metadata_reconciled(&operation_id, &response, now + 2)
                        .unwrap();
                }
                _ => unreachable!(),
            }
            drop(ledger);

            let mut recovered = SecureMeshMlsSecurityLedger::open(&path).unwrap();
            let record = recovered.operation(&operation_id).unwrap().unwrap();
            match boundary {
                "after_stage_before_snapshot" => {
                    assert_eq!(record.state, SecureMeshMlsOperationState::CryptoPrepared);
                    assert_eq!(record.base_metadata, base);
                    recovered
                        .reset_crypto_prepared_operation_for_retry(&operation_id, now + 3)
                        .unwrap();
                    assert!(
                        recovered
                            .abort_empty_prepared_operation(&operation_id)
                            .unwrap()
                    );
                    assert!(recovered.operation(&operation_id).unwrap().is_none());

                    let next_operation_id =
                        hex_sha256(format!("{action}:{boundary}:next").as_bytes());
                    recovered
                        .begin_operation(
                            &next_operation_id,
                            action,
                            &hex_sha256(b"different-request-after-abandon"),
                            &local.identity,
                            now + 4,
                        )
                        .unwrap();
                    assert!(
                        recovered
                            .abort_empty_prepared_operation(&next_operation_id)
                            .unwrap()
                    );
                }
                "after_snapshot_before_crypto_commit" => {
                    assert_eq!(record.state, SecureMeshMlsOperationState::CryptoPrepared);
                    recovered
                        .commit_operation_crypto(&operation_id, &expected, now + 3)
                        .unwrap();
                    recovered
                        .mark_operation_metadata_reconciled(&operation_id, &response, now + 4)
                        .unwrap();
                    recovered
                        .mark_operation_delivered(&operation_id, now + 5)
                        .unwrap();
                }
                "after_crypto_commit_before_metadata" => {
                    assert_eq!(record.state, SecureMeshMlsOperationState::CryptoCommitted);
                    recovered
                        .mark_operation_metadata_reconciled(&operation_id, &response, now + 3)
                        .unwrap();
                    recovered
                        .mark_operation_delivered(&operation_id, now + 4)
                        .unwrap();
                }
                "after_metadata_before_delivery" => {
                    assert_eq!(
                        record.state,
                        SecureMeshMlsOperationState::MetadataReconciled
                    );
                    assert_eq!(record.response.as_ref(), Some(&response));
                    recovered
                        .mark_operation_delivered(&operation_id, now + 3)
                        .unwrap();
                }
                _ => unreachable!(),
            }
            if boundary != "after_stage_before_snapshot" {
                assert_eq!(
                    recovered.operation(&operation_id).unwrap().unwrap().state,
                    SecureMeshMlsOperationState::Delivered
                );
            }
            assert!(
                recovered
                    .incomplete_writer_operations(&local.identity)
                    .unwrap()
                    .is_empty()
            );
            let foreign_key_errors: i64 = recovered
                .connection
                .query_row("SELECT COUNT(*) FROM pragma_foreign_key_check", [], |row| {
                    row.get(0)
                })
                .unwrap();
            assert_eq!(foreign_key_errors, 0);
            let _ = std::fs::remove_file(path);
        }
    }
}

#[test]
fn secure_mesh_mls_invalid_prepared_requests_do_not_consume_journal_capacity() {
    let local = device("mobile:journal-invalid-local");
    let path = ledger_path("invalid-prepared-capacity");
    let now = capability_now().unix_timestamp();
    let mut ledger = SecureMeshMlsSecurityLedger::open(&path).unwrap();

    for index in 0..(MAX_INCOMPLETE_MLS_OPERATIONS_PER_SCOPE * 2) {
        let operation_id = hex_sha256(format!("invalid-operation-{index}").as_bytes());
        ledger
            .begin_operation(
                &operation_id,
                "secure_mesh.mls.commit.process",
                &hex_sha256(format!("invalid-request-{index}").as_bytes()),
                &local.identity,
                now,
            )
            .unwrap();
        assert!(
            ledger
                .abort_empty_prepared_operation(&operation_id)
                .unwrap()
        );
    }

    let valid_operation = hex_sha256(b"valid-operation-after-invalid-inputs");
    ledger
        .begin_operation(
            &valid_operation,
            "secure_mesh.mls.commit.process",
            &hex_sha256(b"valid-request-after-invalid-inputs"),
            &local.identity,
            now + 1,
        )
        .unwrap();
    assert!(
        ledger
            .abort_empty_prepared_operation(&valid_operation)
            .unwrap()
    );
    let _ = std::fs::remove_file(path);
}

#[test]
fn secure_mesh_mls_journal_enforces_single_writer_exact_state_and_bounded_gc() {
    let local = device("desktop_gui:journal-invariants-local");
    let participant_scope = local.identity.fingerprint().unwrap();
    let path = ledger_path("journal-invariants");
    let now = capability_now().unix_timestamp();
    let prepared = empty_prepared_security_inputs(&local.identity, now).unwrap();
    let mut ledger = SecureMeshMlsSecurityLedger::open(&path).unwrap();

    let first_group = b"journal-writer-first";
    let first_base = journal_metadata(first_group, &participant_scope, 1, "first-base");
    let first_expected = journal_metadata(first_group, &participant_scope, 2, "first-expected");
    let first_operation = hex_sha256(b"journal-writer-first-operation");
    ledger
        .begin_operation(
            &first_operation,
            "secure_mesh.mls.member.add",
            &hex_sha256(b"journal-writer-first-request"),
            &local.identity,
            now,
        )
        .unwrap();
    ledger
        .stage_operation(
            &first_operation,
            &serde_json::json!({"ok": true, "group": null}),
            first_group,
            Some(&first_base),
            &first_expected,
            &prepared,
            now,
        )
        .unwrap();

    let second_group = b"journal-writer-second";
    let second_base = journal_metadata(second_group, &participant_scope, 4, "second-base");
    let second_expected = journal_metadata(second_group, &participant_scope, 5, "second-expected");
    let second_operation = hex_sha256(b"journal-writer-second-operation");
    ledger
        .begin_operation(
            &second_operation,
            "secure_mesh.mls.commit.process",
            &hex_sha256(b"journal-writer-second-request"),
            &local.identity,
            now,
        )
        .unwrap();
    let writer_error = ledger
        .stage_operation(
            &second_operation,
            &serde_json::json!({}),
            second_group,
            Some(&second_base),
            &second_expected,
            &prepared,
            now,
        )
        .unwrap_err();
    assert!(writer_error.to_string().contains("another writer"));

    let mut same_epoch_divergence = first_expected.clone();
    same_epoch_divergence.public_state_digest = format!(
        "sha256:{}",
        hex_sha256(b"same-epoch-divergent-public-state")
    );
    let divergence_error = ledger
        .commit_operation_crypto(&first_operation, &same_epoch_divergence, now + 1)
        .unwrap_err();
    assert!(divergence_error.to_string().contains("does not match"));
    ledger
        .commit_operation_crypto(&first_operation, &first_expected, now + 1)
        .unwrap();
    let first_response = serde_json::json!({"ok": true, "group": {"epoch": 2}});
    ledger
        .mark_operation_metadata_reconciled(&first_operation, &first_response, now + 2)
        .unwrap();
    ledger
        .mark_operation_metadata_reconciled(&first_operation, &first_response, now + 2)
        .unwrap();
    let response_divergence = ledger
        .mark_operation_metadata_reconciled(
            &first_operation,
            &serde_json::json!({"ok": false}),
            now + 2,
        )
        .unwrap_err();
    assert!(
        response_divergence
            .to_string()
            .contains("response diverges")
    );
    ledger
        .mark_operation_delivered(&first_operation, now + 3)
        .unwrap();

    ledger
        .stage_operation(
            &second_operation,
            &serde_json::json!({}),
            second_group,
            Some(&second_base),
            &second_expected,
            &prepared,
            now + 4,
        )
        .unwrap();
    ledger
        .commit_operation_crypto(&second_operation, &second_expected, now + 5)
        .unwrap();
    ledger
        .mark_operation_metadata_reconciled(
            &second_operation,
            &serde_json::json!({"ok": true}),
            now + 6,
        )
        .unwrap();
    ledger
        .mark_operation_delivered(&second_operation, now + 7)
        .unwrap();

    let bound_operation = hex_sha256(b"journal-group-binding-operation");
    ledger
        .begin_operation(
            &bound_operation,
            "secure_mesh.mls.group.join",
            &hex_sha256(b"journal-group-binding-request"),
            &local.identity,
            now + 8,
        )
        .unwrap();
    let group_binding_error = ledger
        .stage_operation(
            &bound_operation,
            &serde_json::json!({}),
            b"wrong-group-id",
            None,
            &second_expected,
            &prepared,
            now + 8,
        )
        .unwrap_err();
    assert!(group_binding_error.to_string().contains("group id"));
    assert!(
        ledger
            .abort_empty_prepared_operation(&bound_operation)
            .unwrap()
    );

    let cascading_operation = hex_sha256(b"journal-cascade-operation");
    ledger
        .begin_operation(
            &cascading_operation,
            "secure_mesh.mls.commit.process",
            &hex_sha256(b"journal-cascade-request"),
            &local.identity,
            now + 9,
        )
        .unwrap();
    ledger
        .stage_operation(
            &cascading_operation,
            &serde_json::json!({}),
            second_group,
            Some(&second_base),
            &second_expected,
            &prepared,
            now + 9,
        )
        .unwrap();
    ledger
        .connection
        .execute(
            "DELETE FROM secure_mesh_mls_operations WHERE operation_id = ?1",
            params![cascading_operation],
        )
        .unwrap();
    let dangling_reservations: i64 = ledger
        .connection
        .query_row(
            "SELECT COUNT(*) FROM secure_mesh_mls_operation_reservations WHERE operation_id = ?1",
            params![cascading_operation],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(dangling_reservations, 0);
    let foreign_key_errors: i64 = ledger
        .connection
        .query_row("SELECT COUNT(*) FROM pragma_foreign_key_check", [], |row| {
            row.get(0)
        })
        .unwrap();
    assert_eq!(foreign_key_errors, 0);

    let protected_prepared = hex_sha256(b"journal-gc-protected-prepared");
    ledger
        .begin_operation(
            &protected_prepared,
            "secure_mesh.mls.commit.process",
            &hex_sha256(b"journal-gc-protected-request"),
            &local.identity,
            now + 10,
        )
        .unwrap();
    let local_scope = mls_security_scope_hash(&local.identity).unwrap();
    {
        let tx = ledger.connection.transaction().unwrap();
        for index in 0..(MAX_DELIVERED_MLS_OPERATIONS_PER_SCOPE + 64) {
            tx.execute(
                r#"
                    INSERT INTO secure_mesh_mls_operations (
                        operation_id, local_endpoint_scope_hash, action, request_digest, state,
                        response_json, group_id_base64url, base_metadata_json,
                        expected_metadata_json, prepared_security_json,
                        created_at_unix_seconds, updated_at_unix_seconds
                    ) VALUES (?1, ?2, 'secure_mesh.mls.commit.process', ?3, 'delivered',
                              '{}', NULL, NULL, NULL, NULL, ?4, ?4)
                    "#,
                params![
                    hex_sha256(format!("gc-delivered-{index}").as_bytes()),
                    local_scope,
                    hex_sha256(format!("gc-request-{index}").as_bytes()),
                    now + i64::try_from(index).unwrap(),
                ],
            )
            .unwrap();
        }
        tx.commit().unwrap();
    }
    let gc_trigger = hex_sha256(b"journal-gc-trigger");
    ledger
        .begin_operation(
            &gc_trigger,
            "secure_mesh.mls.commit.process",
            &hex_sha256(b"journal-gc-trigger-request"),
            &local.identity,
            now + 1000,
        )
        .unwrap();
    let delivered_count: i64 = ledger
            .connection
            .query_row(
                "SELECT COUNT(*) FROM secure_mesh_mls_operations WHERE local_endpoint_scope_hash = ?1 AND state = 'delivered'",
                params![local_scope],
                |row| row.get(0),
            )
            .unwrap();
    assert!(delivered_count <= i64::try_from(MAX_DELIVERED_MLS_OPERATIONS_PER_SCOPE).unwrap());
    assert!(ledger.operation(&protected_prepared).unwrap().is_some());
    assert!(
        ledger
            .abort_empty_prepared_operation(&protected_prepared)
            .unwrap()
    );
    assert!(ledger.abort_empty_prepared_operation(&gc_trigger).unwrap());
    let _ = std::fs::remove_file(path);
}

#[test]
fn secure_mesh_mls_journal_and_replay_ledgers_fail_closed_at_capacity() {
    let local = device("mobile:journal-capacity-local");
    let participant_scope = local.identity.fingerprint().unwrap();
    let path = ledger_path("journal-capacity");
    let now = capability_now().unix_timestamp();
    let mut ledger = SecureMeshMlsSecurityLedger::open(&path).unwrap();
    let mut prepared_operations = Vec::new();

    for index in 0..MAX_INCOMPLETE_MLS_OPERATIONS_PER_SCOPE {
        let operation_id = hex_sha256(format!("capacity-operation-{index}").as_bytes());
        ledger
            .begin_operation(
                &operation_id,
                "secure_mesh.mls.commit.process",
                &hex_sha256(format!("capacity-request-{index}").as_bytes()),
                &local.identity,
                now,
            )
            .unwrap();
        prepared_operations.push(operation_id);
    }
    let overflow = hex_sha256(b"capacity-overflow-operation");
    let capacity_error = ledger
        .begin_operation(
            &overflow,
            "secure_mesh.mls.commit.process",
            &hex_sha256(b"capacity-overflow-request"),
            &local.identity,
            now,
        )
        .unwrap_err();
    assert!(capacity_error.to_string().contains("at capacity"));
    assert!(
        ledger
            .abort_empty_prepared_operation(&prepared_operations[0])
            .unwrap()
    );
    ledger
        .begin_operation(
            &overflow,
            "secure_mesh.mls.commit.process",
            &hex_sha256(b"capacity-overflow-request"),
            &local.identity,
            now + 1,
        )
        .unwrap();
    for operation_id in prepared_operations.iter().skip(1) {
        assert!(ledger.abort_empty_prepared_operation(operation_id).unwrap());
    }
    assert!(ledger.abort_empty_prepared_operation(&overflow).unwrap());

    let local_scope = mls_security_scope_hash(&local.identity).unwrap();
    {
        let tx = ledger.connection.transaction().unwrap();
        for index in 0..MAX_PERSISTED_MLS_KEY_PACKAGES_PER_SCOPE {
            tx.execute(
                r#"
                    INSERT INTO secure_mesh_mls_keypackage_uses (
                        consumer_endpoint_id, key_package_id, key_package_public_key_hash,
                        group_id_hash, used_at
                    ) VALUES (?1, ?2, ?3, ?4, ?5)
                    "#,
                params![
                    local_scope,
                    hex_sha256(format!("keypackage-id-{index}").as_bytes()),
                    hex_sha256(format!("keypackage-public-{index}").as_bytes()),
                    format!("sha256:{}", hex_sha256(b"capacity-group")),
                    now.to_string(),
                ],
            )
            .unwrap();
        }
        tx.commit().unwrap();
    }
    let capacity_group = b"capacity-group";
    let base = journal_metadata(capacity_group, &participant_scope, 1, "capacity-base");
    let expected = journal_metadata(capacity_group, &participant_scope, 2, "capacity-expected");
    let keypackage_operation = hex_sha256(b"keypackage-capacity-operation");
    ledger
        .begin_operation(
            &keypackage_operation,
            "secure_mesh.mls.member.add",
            &hex_sha256(b"keypackage-capacity-request"),
            &local.identity,
            now + 2,
        )
        .unwrap();
    let keypackage_prepared = PreparedMlsSecurityInputs {
        local_endpoint_scope_hash: local_scope.clone(),
        key_package: Some(PreparedMlsKeyPackageUse {
            key_package_id_hash: hex_sha256(b"new-keypackage-id"),
            key_package_public_key_hash: hex_sha256(b"new-keypackage-public"),
            group_id_hash: expected.group_id_hash.clone(),
        }),
        capability_proofs: [
            PreparedMlsCapabilityProofUse {
                proof_digest: hex_sha256(b"keypackage-proof-one"),
                expires_at_unix_seconds: now + 100,
            },
            PreparedMlsCapabilityProofUse {
                proof_digest: hex_sha256(b"keypackage-proof-two"),
                expires_at_unix_seconds: now + 100,
            },
        ],
        consumed_at_unix_seconds: now,
    };
    let keypackage_capacity_error = ledger
        .stage_operation(
            &keypackage_operation,
            &serde_json::json!({}),
            capacity_group,
            Some(&base),
            &expected,
            &keypackage_prepared,
            now + 2,
        )
        .unwrap_err();
    assert!(
        keypackage_capacity_error
            .to_string()
            .contains("at capacity")
    );
    assert!(
        ledger
            .abort_empty_prepared_operation(&keypackage_operation)
            .unwrap()
    );

    {
        let tx = ledger.connection.transaction().unwrap();
        for index in 0..(MAX_PERSISTED_MLS_CAPABILITY_PROOFS - 1) {
            tx.execute(
                r#"
                    INSERT INTO secure_mesh_mls_capability_proof_uses (
                        local_endpoint_scope_hash, proof_digest,
                        expires_at_unix_seconds, consumed_at_unix_seconds
                    ) VALUES (?1, ?2, ?3, ?4)
                    "#,
                params![
                    local_scope,
                    hex_sha256(format!("capability-proof-{index}").as_bytes()),
                    now + 100,
                    now,
                ],
            )
            .unwrap();
        }
        tx.commit().unwrap();
    }
    let proof_operation = hex_sha256(b"proof-capacity-operation");
    ledger
        .begin_operation(
            &proof_operation,
            "secure_mesh.mls.commit.process",
            &hex_sha256(b"proof-capacity-request"),
            &local.identity,
            now + 3,
        )
        .unwrap();
    let proof_prepared = PreparedMlsSecurityInputs {
        local_endpoint_scope_hash: local_scope,
        key_package: None,
        capability_proofs: [
            PreparedMlsCapabilityProofUse {
                proof_digest: hex_sha256(b"new-capability-proof-one"),
                expires_at_unix_seconds: now + 100,
            },
            PreparedMlsCapabilityProofUse {
                proof_digest: hex_sha256(b"new-capability-proof-two"),
                expires_at_unix_seconds: now + 100,
            },
        ],
        consumed_at_unix_seconds: now,
    };
    let proof_capacity_error = ledger
        .stage_operation(
            &proof_operation,
            &serde_json::json!({}),
            capacity_group,
            Some(&base),
            &expected,
            &proof_prepared,
            now + 3,
        )
        .unwrap_err();
    assert!(proof_capacity_error.to_string().contains("at capacity"));
    assert!(
        ledger
            .abort_empty_prepared_operation(&proof_operation)
            .unwrap()
    );
    let _ = std::fs::remove_file(path);
}

#[test]
fn secure_mesh_mls_product_keypackage_one_time_consumption() {
    let alice = device("desktop_gui:alice");
    let bob = device("mobile:bob");
    let mut group = create_product_group(
        &alice.participant,
        &alice.identity,
        &DeviceTrustState::Verified,
        b"kp-once",
    )
    .unwrap();
    let bob_kp = bob.participant.generate_key_package().unwrap();
    let path = ledger_path("once");
    let mut ledger = SecureMeshMlsSecurityLedger::open(&path).unwrap();
    add_test_product_member(
        &mut group,
        &alice,
        &bob,
        &bob_kp,
        &mut ledger,
        "kp-bob-reuse",
    );
    let owner_proof = sign_mls_keypackage_capability_proof(
        &alice.identity,
        &alice.signing_key,
        &capability_evaluation(),
        &bob_kp,
        capability_now(),
    )
    .unwrap();
    let member_proof = sign_mls_keypackage_capability_proof(
        &bob.identity,
        &bob.signing_key,
        &capability_evaluation(),
        &bob_kp,
        capability_now(),
    )
    .unwrap();
    let group_id = group.group_id_bytes().unwrap();
    let base = group
        .public_metadata(alice.identity.fingerprint().unwrap())
        .unwrap();
    let mut expected = base.clone();
    expected.epoch += 1;
    expected.member_count += 1;
    expected.public_state_digest =
        format!("sha256:{}", hex_sha256(b"keypackage-replay-expected-state"));
    let prepared = prepare_member_add_security_inputs(
        &alice.identity,
        "kp-bob-reuse",
        bob_kp.as_public_bytes(),
        &expected.group_id_hash,
        &owner_proof,
        &member_proof,
        capability_now().unix_timestamp(),
    )
    .unwrap();
    let operation_id = begin_test_journal_operation(
        &mut ledger,
        "secure_mesh.mls.member.add",
        b"keypackage-replay-attempt",
        &alice.identity,
        capability_now(),
    )
    .unwrap();
    let reuse = stage_test_journal_operation(
        &mut ledger,
        &operation_id,
        &group_id,
        Some(&base),
        &expected,
        &prepared,
        &serde_json::json!({}),
        capability_now(),
    )
    .unwrap_err();
    assert!(reuse.to_string().contains("already consumed"));
    assert!(
        ledger
            .was_key_package_consumed(&alice.identity, "kp-bob-reuse")
            .unwrap()
    );
    assert_eq!(
        ledger
            .key_package_consumed_at(&alice.identity, "kp-bob-reuse")
            .unwrap(),
        Some(capability_now().unix_timestamp())
    );
    let _ = std::fs::remove_file(&path);
}

#[test]
fn secure_mesh_mls_replay_watermark_rejects_expiry_revival_after_clock_rollback() {
    let path = ledger_path("capability-proof-clock-rollback");
    let _ = std::fs::remove_file(&path);
    let scope = hex_sha256(b"mls-clock-rollback-scope");
    let prepared =
        |label: &str, consumed_at_unix_seconds: i64, expiry: i64| PreparedMlsSecurityInputs {
            local_endpoint_scope_hash: scope.clone(),
            key_package: None,
            capability_proofs: [
                PreparedMlsCapabilityProofUse {
                    proof_digest: format!("sha256:{}", hex_sha256(format!("{label}-a").as_bytes())),
                    expires_at_unix_seconds: expiry,
                },
                PreparedMlsCapabilityProofUse {
                    proof_digest: format!("sha256:{}", hex_sha256(format!("{label}-b").as_bytes())),
                    expires_at_unix_seconds: expiry,
                },
            ],
            consumed_at_unix_seconds,
        };
    let old = prepared("old", 900, 1_000);
    let new = prepared("new", 2_000, 2_100);
    {
        let mut ledger = SecureMeshMlsSecurityLedger::open(&path).unwrap();
        let tx = ledger
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .unwrap();
        consume_prepared_security_transaction(&tx, &old, 900).unwrap();
        tx.commit().unwrap();
        let tx = ledger
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .unwrap();
        consume_prepared_security_transaction(&tx, &new, 2_000).unwrap();
        tx.commit().unwrap();
    }
    let mut reopened = SecureMeshMlsSecurityLedger::open(&path).unwrap();
    let tx = reopened
        .connection
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .unwrap();
    let revived = consume_prepared_security_transaction(&tx, &old, 950).unwrap_err();
    assert!(revived.to_string().contains("clock rollback"));
    drop(tx);
    let _ = std::fs::remove_file(path);
}

#[test]
fn secure_mesh_mls_security_ledger_survives_restart_and_rolls_back_atomically() {
    let alice = device("desktop_gui:ledger-alice");
    let bob = device("mobile:ledger-bob");
    let mut group = create_product_group(
        &alice.participant,
        &alice.identity,
        &DeviceTrustState::Verified,
        b"persistent-ledger",
    )
    .unwrap();
    let bob_key_package = bob.participant.generate_key_package().unwrap();
    let path = ledger_path("persistent-replay");
    let mut ledger = SecureMeshMlsSecurityLedger::open(&path).unwrap();
    add_test_product_member(
        &mut group,
        &alice,
        &bob,
        &bob_key_package,
        &mut ledger,
        "sensitive-key-package-id",
    );
    let extension = group.capability_extension().unwrap();
    let (first, second) = active_pair_capability_proofs(&extension).unwrap();
    let first = first.clone();
    let second = second.clone();
    drop(ledger);

    let mut reopened = SecureMeshMlsSecurityLedger::open(&path).unwrap();
    let group_id = group.group_id_bytes().unwrap();
    let base = group
        .public_metadata(alice.identity.fingerprint().unwrap())
        .unwrap();
    let mut expected = base.clone();
    expected.epoch += 1;
    expected.public_state_digest =
        format!("sha256:{}", hex_sha256(b"capability-replay-expected-state"));
    let replay_prepared = prepare_capability_security_inputs(
        &alice.identity,
        &first,
        &second,
        capability_now().unix_timestamp(),
    )
    .unwrap();
    let replay_operation = begin_test_journal_operation(
        &mut reopened,
        "secure_mesh.mls.commit.process",
        b"capability-replay-after-reopen",
        &alice.identity,
        capability_now(),
    )
    .unwrap();
    let replay = stage_test_journal_operation(
        &mut reopened,
        &replay_operation,
        &group_id,
        Some(&base),
        &expected,
        &replay_prepared,
        &serde_json::json!({}),
        capability_now(),
    )
    .unwrap_err();
    assert!(replay.to_string().contains("replay"));

    let atomic_prepared = prepare_member_add_security_inputs(
        &alice.identity,
        "must-roll-back",
        b"different-public-key",
        &expected.group_id_hash,
        &first,
        &second,
        capability_now().unix_timestamp(),
    )
    .unwrap();
    let atomic_operation = begin_test_journal_operation(
        &mut reopened,
        "secure_mesh.mls.member.add",
        b"atomic-replay-after-reopen",
        &alice.identity,
        capability_now(),
    )
    .unwrap();
    let atomic_error = stage_test_journal_operation(
        &mut reopened,
        &atomic_operation,
        &group_id,
        Some(&base),
        &expected,
        &atomic_prepared,
        &serde_json::json!({}),
        capability_now(),
    )
    .unwrap_err();
    assert!(atomic_error.to_string().contains("replay"));
    assert!(
        !reopened
            .was_key_package_consumed(&alice.identity, "must-roll-back")
            .unwrap()
    );
    drop(reopened);

    let database_bytes = std::fs::read(&path).unwrap();
    let database_text = String::from_utf8_lossy(&database_bytes);
    assert!(!database_text.contains(&alice.identity.endpoint_id));
    assert!(!database_text.contains("sensitive-key-package-id"));
    assert!(!database_text.contains(&first.signature));
    assert!(!database_text.contains(&second.signature));
    let _ = std::fs::remove_file(path);
}
