use ed25519_dalek::SigningKey;
use rand_core::OsRng;
use serde_json::json;
use time::OffsetDateTime;

use crate::core::secure_mesh_mls_product::{
    SecureMeshMlsSecurityLedger, create_product_group, participant_from_device_identity,
    sign_mls_keypackage_capability_proof,
};
use crate::core::secure_mesh_trust::{DeviceTrustPublicIdentity, DeviceTrustState};

use super::super::group_state::reconcile_group_metadata;
use super::super::input_codec::hex_sha256;
use super::super::journal_recovery::current_group_metadata;
use super::super::participant_runtime::handle_missing_participant_snapshot;

#[test]
fn missing_mls_snapshot_purges_only_memory_custody_and_fails_closed_for_persistent_custody() {
    let root = std::env::temp_dir().join(format!(
        "lico-mls-missing-snapshot-{}",
        uuid::Uuid::new_v4()
    ));
    let previous =
        licoup_foundation::platform::paths::set_portable_data_dir_override(Some(root.clone()));
    let identity_key = SigningKey::generate(&mut OsRng);
    let signing_key = SigningKey::generate(&mut OsRng);
    let identity = DeviceTrustPublicIdentity::new(
        "mobile:missing-snapshot",
        identity_key.verifying_key().to_bytes(),
        signing_key.verifying_key().to_bytes(),
        1,
    )
    .unwrap();
    let participant = participant_from_device_identity(&identity, &signing_key).unwrap();
    let group = create_product_group(
        &participant,
        &identity,
        &DeviceTrustState::Verified,
        b"missing-snapshot-group",
    )
    .unwrap();
    let mut group_store = crate::platform::secure_mesh_mls_store::open(
        crate::domain::mobile_relay::secure_mesh_mls_state_dir()
            .unwrap()
            .join("group-state.sqlite3"),
    )
    .unwrap();
    reconcile_group_metadata(&mut group_store, &group, &identity).unwrap();

    let member_identity_key = SigningKey::generate(&mut OsRng);
    let member_signing_key = SigningKey::generate(&mut OsRng);
    let member_identity = DeviceTrustPublicIdentity::new(
        "mobile:replay-ledger-member",
        member_identity_key.verifying_key().to_bytes(),
        member_signing_key.verifying_key().to_bytes(),
        1,
    )
    .unwrap();
    let member_participant =
        participant_from_device_identity(&member_identity, &member_signing_key).unwrap();
    let key_package = member_participant.generate_key_package().unwrap();
    let capability_evaluation = crate::core::secure_mesh_capability::capability_catalog()
        .unwrap()
        .evaluate(
            &crate::core::secure_mesh_capability::mandatory_protocol_facts(
                crate::core::secure_mesh_capability::CapabilityEvidenceKind::TestFixture,
            )
            .unwrap(),
        )
        .unwrap();
    let now = OffsetDateTime::now_utc();
    let local_proof = sign_mls_keypackage_capability_proof(
        &identity,
        &signing_key,
        &capability_evaluation,
        &key_package,
        now,
    )
    .unwrap();
    let member_proof = sign_mls_keypackage_capability_proof(
        &member_identity,
        &member_signing_key,
        &capability_evaluation,
        &key_package,
        now,
    )
    .unwrap();
    let state_dir = crate::domain::mobile_relay::secure_mesh_mls_state_dir().unwrap();
    let ledger_path = state_dir.join("security-ledger.sqlite3");
    let group_id = group.group_id_bytes().unwrap();
    let base = current_group_metadata(&group, &identity).unwrap();
    let mut expected = base.clone();
    expected.epoch += 1;
    expected.member_count += 1;
    expected.public_state_digest = format!(
        "sha256:{}",
        hex_sha256(b"memory-restart-replay-ledger-expected")
    );
    let prepared = crate::core::secure_mesh_mls_product::prepare_member_add_security_inputs(
        &identity,
        "memory-restart-key-package",
        key_package.as_public_bytes(),
        &expected.group_id_hash,
        &local_proof,
        &member_proof,
        now.unix_timestamp(),
    )
    .unwrap();
    let operation_id = hex_sha256(b"memory-restart-replay-ledger-operation");
    let mut ledger = SecureMeshMlsSecurityLedger::open(&ledger_path).unwrap();
    ledger
        .begin_operation(
            &operation_id,
            "secure_mesh.mls.member.add",
            &hex_sha256(b"memory-restart-replay-ledger-request"),
            &identity,
            now.unix_timestamp(),
        )
        .unwrap();
    let staged = ledger
        .stage_operation(
            &operation_id,
            &json!({}),
            &group_id,
            Some(&base),
            &expected,
            &prepared,
            now.unix_timestamp(),
        )
        .unwrap();
    let committed = ledger
        .commit_operation_crypto(&staged.operation_id, &expected, now.unix_timestamp())
        .unwrap();
    let reconciled = ledger
        .mark_operation_metadata_reconciled(
            &committed.operation_id,
            &json!({"ok": true}),
            now.unix_timestamp(),
        )
        .unwrap();
    ledger
        .mark_operation_delivered(&reconciled.operation_id, now.unix_timestamp())
        .unwrap();
    drop(ledger);

    handle_missing_participant_snapshot(&mut None, &identity, "memory-only-ephemeral").unwrap();
    let store = crate::platform::secure_mesh_mls_store::open(state_dir.join("group-state.sqlite3"))
        .unwrap();
    assert!(
        !store
            .has_records_for_participant(&identity.fingerprint().unwrap())
            .unwrap()
    );
    let mut reopened_ledger = SecureMeshMlsSecurityLedger::open(&ledger_path).unwrap();
    let replay_operation = hex_sha256(b"memory-restart-keypackage-replay-operation");
    reopened_ledger
        .begin_operation(
            &replay_operation,
            "secure_mesh.mls.member.add",
            &hex_sha256(b"memory-restart-keypackage-replay-request"),
            &identity,
            now.unix_timestamp(),
        )
        .unwrap();
    let key_package_replay = reopened_ledger
        .stage_operation(
            &replay_operation,
            &json!({}),
            &group_id,
            Some(&base),
            &expected,
            &prepared,
            now.unix_timestamp(),
        )
        .unwrap_err();
    assert!(key_package_replay.to_string().contains("already consumed"));
    assert!(
        reopened_ledger
            .abort_empty_prepared_operation(&replay_operation)
            .unwrap()
    );
    let proof_prepared = crate::core::secure_mesh_mls_product::prepare_capability_security_inputs(
        &identity,
        &local_proof,
        &member_proof,
        now.unix_timestamp(),
    )
    .unwrap();
    let proof_operation = hex_sha256(b"memory-restart-proof-replay-operation");
    reopened_ledger
        .begin_operation(
            &proof_operation,
            "secure_mesh.mls.commit.process",
            &hex_sha256(b"memory-restart-proof-replay-request"),
            &identity,
            now.unix_timestamp(),
        )
        .unwrap();
    let proof_replay = reopened_ledger
        .stage_operation(
            &proof_operation,
            &json!({}),
            &group_id,
            Some(&base),
            &expected,
            &proof_prepared,
            now.unix_timestamp(),
        )
        .unwrap_err();
    assert!(proof_replay.to_string().contains("replay rejected"));
    assert!(
        reopened_ledger
            .abort_empty_prepared_operation(&proof_operation)
            .unwrap()
    );

    let mut group_store = crate::platform::secure_mesh_mls_store::open(
        crate::domain::mobile_relay::secure_mesh_mls_state_dir()
            .unwrap()
            .join("group-state.sqlite3"),
    )
    .unwrap();
    reconcile_group_metadata(&mut group_store, &group, &identity).unwrap();
    let persistent_error =
        handle_missing_participant_snapshot(&mut None, &identity, "android-keystore").unwrap_err();
    assert!(persistent_error.to_string().contains("snapshot is missing"));
    let store = crate::platform::secure_mesh_mls_store::open(state_dir.join("group-state.sqlite3"))
        .unwrap();
    assert!(
        store
            .has_records_for_participant(&identity.fingerprint().unwrap())
            .unwrap()
    );

    licoup_foundation::platform::paths::set_portable_data_dir_override(previous);
    let _ = std::fs::remove_dir_all(root);
}
