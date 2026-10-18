use super::super::constants::{
    MAX_PERSISTED_CHECKPOINTS, MAX_PERSISTED_DIRECTORY_AUTHORIZATIONS,
    MAX_PERSISTED_DIRECTORY_LABELS,
};
use super::super::persistence::{
    enforce_directory_authorization_quota, enforce_directory_label_quota,
    reclaim_stale_directory_authorizations, u64_to_sql,
};
use super::super::*;
use super::support::{leaf, state_path};
use ed25519_dalek::SigningKey;
use rand_core::OsRng;
use rusqlite::{Connection, TransactionBehavior, params};

#[test]
fn sqlite_checkpoint_requires_consistency_and_persists_rollback_across_restart() {
    let path = state_path("rollback");
    let mut log = SecureMeshKtLog::new(SigningKey::generate(&mut OsRng));
    log.append_leaf(&leaf("device-a", 1, "active")).unwrap();
    let first_sth = log.sign_tree_head(100).unwrap();
    let pin = log.pin();
    let policy = KtFreshnessPolicy::strict(120, 2).unwrap();
    {
        let mut state = SecureMeshKtClientState::open(&path, pin.clone(), policy).unwrap();
        state.observe_tree_head(&first_sth, None, 100).unwrap();
        log.append_leaf(&leaf("device-b", 1, "active")).unwrap();
        let second_sth = log.sign_tree_head(101).unwrap();
        let missing = state.observe_tree_head(&second_sth, None, 101).unwrap_err();
        assert!(
            missing
                .to_string()
                .contains("consistency proof is required")
        );
        assert_eq!(state.checkpoint_count().unwrap(), 1);
        let consistency = log.consistency_proof_at(1, 101).unwrap();
        state
            .observe_tree_head(&second_sth, Some(&consistency), 101)
            .unwrap();
    }
    {
        let mut restored = SecureMeshKtClientState::open(&path, pin.clone(), policy).unwrap();
        let rollback = restored
            .observe_tree_head(&first_sth, None, 102)
            .unwrap_err();
        assert!(rollback.to_string().contains("tree rollback"));
    }
    let restored = SecureMeshKtClientState::open(&path, pin, policy).unwrap();
    assert!(restored.equivocation_detected().unwrap());
    let _ = std::fs::remove_file(path);
}

#[test]
fn durable_time_watermark_prevents_clock_rollback_and_expiry_revival() {
    let path = state_path("durable-time-watermark");
    let mut log = SecureMeshKtLog::new(SigningKey::generate(&mut OsRng));
    log.append_leaf(&leaf("device-time", 1, "active")).unwrap();
    let sth = log.sign_tree_head(100).unwrap();
    let pin = log.pin();
    let policy = KtFreshnessPolicy::strict(60, 2).unwrap();
    let mut state = SecureMeshKtClientState::open(&path, pin.clone(), policy).unwrap();
    state.observe_tree_head(&sth, None, 150).unwrap();
    state.observe_tree_head(&sth, None, 90).unwrap();
    let watermark: i64 = state
        .connection
        .query_row(
            "SELECT max_observed_epoch_seconds FROM secure_mesh_kt_time_guard WHERE singleton = 1",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(watermark, 150);

    let expired = state.observe_tree_head(&sth, None, 161).unwrap_err();
    assert!(expired.to_string().contains("authenticated_sth_expired"));
    drop(state);

    let mut rolled_back = SecureMeshKtClientState::open(&path, pin, policy).unwrap();
    let blocked = rolled_back.observe_tree_head(&sth, None, 100).unwrap_err();
    assert!(blocked.to_string().contains("previously persisted"));
    assert!(rolled_back.equivocation_detected().unwrap());
    let _ = std::fs::remove_file(path);
}

#[test]
fn unauthenticated_temporal_input_cannot_persist_security_block() {
    let mut log = SecureMeshKtLog::new(SigningKey::generate(&mut OsRng));
    log.append_leaf(&leaf("device-invalid-time", 1, "active"))
        .unwrap();
    let mut forged = log.sign_tree_head(100).unwrap();
    forged.signature = "00".repeat(64);
    let mut state = SecureMeshKtClientState::open_in_memory(
        log.pin(),
        KtFreshnessPolicy::strict(60, 2).unwrap(),
    )
    .unwrap();

    let error = state
        .observe_tree_head(&forged, None, 10_000)
        .unwrap_err()
        .to_string();
    assert!(error.contains("signature is invalid"));
    assert!(!state.equivocation_detected().unwrap());
}

#[test]
fn directory_label_and_authorization_quotas_are_bounded_with_stale_reclamation() {
    let log = SecureMeshKtLog::new(SigningKey::generate(&mut OsRng));
    let pin = log.pin();
    let policy = KtFreshnessPolicy::strict(60, 2).unwrap();
    let mut state = SecureMeshKtClientState::open_in_memory(pin.clone(), policy).unwrap();
    let transaction = state
        .connection
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .unwrap();
    transaction
            .execute(
                "WITH digits(value) AS (
                    VALUES(0),(1),(2),(3),(4),(5),(6),(7),(8),(9)
                 ), counter(value) AS (
                    SELECT ones.value + 10 * tens.value + 100 * hundreds.value + 1000 * thousands.value
                    FROM digits AS ones, digits AS tens, digits AS hundreds, digits AS thousands
                 )
                 INSERT INTO secure_mesh_kt_directory_latest(
                    log_id, stable_label, version, leaf_hash, revoked,
                    identity_fingerprint, identity_rotation_epoch, identity_key_digest,
                    pairwise_prekey_version, signed_prekey_digest, one_time_prekey_digest,
                    mls_key_package_version, mls_key_package_digest, tree_size
                 )
                 SELECT ?1, printf('%064x', value), 1, printf('%064x', value), 0,
                    'fingerprint', 1, printf('%064x', value), 1,
                    printf('%064x', value), printf('%064x', value), 1,
                    printf('%064x', value), 1
                 FROM counter WHERE value < ?2",
                params![pin.log_id(), u64_to_sql(MAX_PERSISTED_DIRECTORY_LABELS).unwrap()],
            )
            .unwrap();
    let label_error = enforce_directory_label_quota(&transaction, &pin, &"f".repeat(64))
        .unwrap_err()
        .to_string();
    assert!(label_error.contains("label quota"));
    enforce_directory_label_quota(&transaction, &pin, &format!("{:064x}", 1)).unwrap();

    transaction
            .execute(
                "WITH digits(value) AS (
                    VALUES(0),(1),(2),(3),(4),(5),(6),(7),(8),(9)
                 ), counter(value) AS (
                    SELECT ones.value + 10 * tens.value + 100 * hundreds.value + 1000 * thousands.value
                    FROM digits AS ones, digits AS tens, digits AS hundreds, digits AS thousands
                 )
                 INSERT INTO secure_mesh_kt_directory_authorizations(
                    log_id, stable_label, purpose, directory_version, leaf_hash, revoked,
                    tree_size, root_hash, map_root_hash, issued_at_epoch_seconds,
                    observed_at_epoch_seconds, inclusion_json, map_proof_json
                 )
                 SELECT ?1, printf('%064x', value), 'purpose-' || value, 1,
                    printf('%064x', value), 0, 1, printf('%064x', value),
                    printf('%064x', value), 1, 1, '{}', '{}'
                 FROM counter WHERE value < ?2",
                params![
                    pin.log_id(),
                    u64_to_sql(MAX_PERSISTED_DIRECTORY_AUTHORIZATIONS).unwrap()
                ],
            )
            .unwrap();
    let authorization_error =
        enforce_directory_authorization_quota(&transaction, &pin, &"e".repeat(64), "new-purpose")
            .unwrap_err()
            .to_string();
    assert!(authorization_error.contains("authorization quota"));
    reclaim_stale_directory_authorizations(&transaction, &pin, 2).unwrap();
    enforce_directory_authorization_quota(&transaction, &pin, &"e".repeat(64), "new-purpose")
        .unwrap();
    let remaining: i64 = transaction
        .query_row(
            "SELECT COUNT(*) FROM secure_mesh_kt_directory_authorizations WHERE log_id = ?1",
            params![pin.log_id()],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(remaining, 0);
    transaction.commit().unwrap();
}

#[test]
fn unsupported_schema_requires_explicit_state_reset() {
    let path = state_path("unsupported-schema-reset");
    let connection = Connection::open(&path).unwrap();
    connection
        .execute_batch("PRAGMA user_version = 999;")
        .unwrap();
    drop(connection);
    let log = SecureMeshKtLog::new(SigningKey::generate(&mut OsRng));
    let error =
        SecureMeshKtClientState::open(&path, log.pin(), KtFreshnessPolicy::strict(60, 2).unwrap())
            .err()
            .expect("unsupported KT schema must fail closed");
    assert!(error.to_string().contains("explicit security reset"));
    let _ = std::fs::remove_file(path);
}

#[test]
fn checkpoint_retention_is_bounded_without_weakening_latest_rollback_guard() {
    let path = state_path("bounded-checkpoints");
    let mut log = SecureMeshKtLog::new(SigningKey::generate(&mut OsRng));
    log.append_leaf(&leaf("device-0", 1, "active")).unwrap();
    let first_sth = log.sign_tree_head(100).unwrap();
    let pin = log.pin();
    let policy = KtFreshnessPolicy::strict(600, 2).unwrap();
    let mut state = SecureMeshKtClientState::open(&path, pin.clone(), policy).unwrap();
    state.observe_tree_head(&first_sth, None, 100).unwrap();
    for index in 1..80u64 {
        let previous_size = log.tree_size();
        log.append_leaf(&leaf(&format!("device-{index}"), 1, "active"))
            .unwrap();
        let issued_at = 100 + index;
        let sth = log.sign_tree_head(issued_at).unwrap();
        let consistency = log.consistency_proof_at(previous_size, issued_at).unwrap();
        state
            .observe_tree_head(&sth, Some(&consistency), issued_at)
            .unwrap();
    }
    assert_eq!(state.checkpoint_count().unwrap(), MAX_PERSISTED_CHECKPOINTS);
    let rollback = state.observe_tree_head(&first_sth, None, 200).unwrap_err();
    assert!(rollback.to_string().contains("tree rollback"));
    drop(state);
    let restored = SecureMeshKtClientState::open(&path, pin, policy).unwrap();
    assert!(restored.equivocation_detected().unwrap());
    let _ = std::fs::remove_file(path);
}
