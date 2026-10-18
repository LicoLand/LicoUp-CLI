use std::sync::Arc;

use ed25519_dalek::SigningKey;
use rand_core::OsRng;
use serde_json::{Value, json};
use time::OffsetDateTime;

use crate::core::secure_mesh_mls::{SecureMeshMlsGroup, SecureMeshMlsParticipant};
use crate::core::secure_mesh_mls_product::{
    SecureMeshMlsOperationState, create_product_group, participant_from_device_identity,
};
use crate::core::secure_mesh_trust::{DeviceTrustPublicIdentity, DeviceTrustState};
use crate::platform::secure_mesh_secret_store::{
    EphemeralSecretStore, SecretStoreHandle, SecureMeshSecretStore,
};

use super::super::group_state::reconcile_group_metadata;
use super::super::input_codec::hex_sha256;
use super::super::journal_recovery::{
    commit_staged_journaled_operation, current_group_metadata, journal_failpoint,
    open_security_ledger, recover_incomplete_writer_operations, resume_journaled_operation,
    set_journal_failpoint,
};
use super::super::participant_runtime::LocalParticipantRuntime;

#[test]
fn mls_journal_failpoints_drive_reopen_recovery_for_every_mutating_action() {
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
            let root = std::env::temp_dir().join(format!(
                "lico-mls-journal-failpoint-{}",
                uuid::Uuid::new_v4()
            ));
            let previous = licoup_foundation::platform::paths::set_portable_data_dir_override(
                Some(root.clone()),
            );
            let identity_key = SigningKey::generate(&mut OsRng);
            let signing_key = SigningKey::generate(&mut OsRng);
            let identity = DeviceTrustPublicIdentity::new(
                format!("desktop_gui:failpoint-{action}-{boundary}"),
                identity_key.verifying_key().to_bytes(),
                signing_key.verifying_key().to_bytes(),
                1,
            )
            .unwrap();
            let mut participant =
                participant_from_device_identity(&identity, &signing_key).unwrap();
            let group_id = format!("failpoint-group-{action}-{boundary}").into_bytes();
            let mut group = create_product_group(
                &participant,
                &identity,
                &DeviceTrustState::Verified,
                &group_id,
            )
            .unwrap();
            let base = current_group_metadata(&group, &identity).unwrap();
            let mut group_store = crate::platform::secure_mesh_mls_store::open(
                crate::domain::mobile_relay::secure_mesh_mls_state_dir()
                    .unwrap()
                    .join("group-state.sqlite3"),
            )
            .unwrap();
            reconcile_group_metadata(&mut group_store, &group, &identity).unwrap();

            let selected_store: Arc<dyn SecureMeshSecretStore> =
                Arc::new(EphemeralSecretStore::new());
            let authorization = selected_store
                .begin_authorized_session(
                    &crate::platform::secure_mesh_secret_store::SecretStoreAuthorizationRequest::new(
                        "Secure Mesh MLS journal failpoint test",
                        4,
                    ),
                )
                .unwrap();
            let snapshot_handle = SecretStoreHandle::new(
                "secure-mesh-mls-journal-failpoint",
                hex_sha256(format!("{action}:{boundary}").as_bytes()),
            )
            .unwrap();
            participant
                .save_secret_store_with_session(
                    selected_store.as_ref(),
                    &snapshot_handle,
                    &authorization,
                )
                .unwrap();
            group.self_update(&participant).unwrap();
            let expected = current_group_metadata(&group, &identity).unwrap();

            let now = OffsetDateTime::now_utc().unix_timestamp();
            let operation_id = hex_sha256(format!("{action}:{boundary}:op").as_bytes());
            let mut ledger = open_security_ledger().unwrap();
            ledger
                .begin_operation(
                    &operation_id,
                    action,
                    &hex_sha256(format!("{action}:{boundary}:request").as_bytes()),
                    &identity,
                    now,
                )
                .unwrap();
            let prepared = crate::core::secure_mesh_mls_product::empty_prepared_security_inputs(
                &identity, now,
            )
            .unwrap();
            let staged = ledger
                .stage_operation(
                    &operation_id,
                    &if action == "secure_mesh.mls.member.add" {
                        json!({"ok": true, "group": Value::Null})
                    } else {
                        json!({})
                    },
                    &group_id,
                    Some(&base),
                    &expected,
                    &prepared,
                    now,
                )
                .unwrap();
            let mut config = json!({});
            let mut runtime = LocalParticipantRuntime {
                config: &mut config,
                identity: &identity,
                signing_key: &signing_key,
                secret_store: &selected_store,
                authorization: &authorization,
                snapshot_handle: &snapshot_handle,
                participant: &mut participant,
                group_store: &mut None,
            };
            let failpoint_guard = set_journal_failpoint(boundary);
            assert!(
                std::thread::spawn(move || journal_failpoint(boundary))
                    .join()
                    .unwrap()
                    .is_ok(),
                "another test thread must not consume this operation's failpoint"
            );
            let error =
                commit_staged_journaled_operation(&mut runtime, &mut ledger, staged, &group)
                    .unwrap_err();
            assert!(
                error
                    .to_string()
                    .contains("injected journal boundary failure")
            );
            drop(failpoint_guard);
            drop(ledger);

            let recovered_participant =
                SecureMeshMlsParticipant::load_from_secret_store_with_optional_session(
                    crate::core::secure_mesh_mls_product::mls_credential_identity_bytes(&identity)
                        .unwrap(),
                    identity.signing_public_key,
                    selected_store.as_ref(),
                    &snapshot_handle,
                    Some(&authorization),
                )
                .unwrap();
            recover_incomplete_writer_operations(&mut None, &recovered_participant, &identity)
                .unwrap();
            let mut recovered_ledger = open_security_ledger().unwrap();
            let mut recovered_record = recovered_ledger.operation(&operation_id).unwrap().unwrap();
            if recovered_record.state == SecureMeshMlsOperationState::MetadataReconciled {
                let recovered_group =
                    SecureMeshMlsGroup::load(&recovered_participant, &group_id).unwrap();
                let mut recovered_store = crate::platform::secure_mesh_mls_store::open(
                    crate::domain::mobile_relay::secure_mesh_mls_state_dir()
                        .unwrap()
                        .join("group-state.sqlite3"),
                )
                .unwrap();
                resume_journaled_operation(
                    &mut recovered_store,
                    &mut recovered_ledger,
                    recovered_record.clone(),
                    Some(&recovered_group),
                    &identity,
                )
                .unwrap();
                recovered_record = recovered_ledger.operation(&operation_id).unwrap().unwrap();
            }
            if boundary == "after_stage_before_snapshot" {
                assert_eq!(
                    recovered_record.state,
                    SecureMeshMlsOperationState::Prepared
                );
                assert!(
                    recovered_ledger
                        .abort_empty_prepared_operation(&operation_id)
                        .unwrap()
                );
            } else {
                assert_eq!(
                    recovered_record.state,
                    SecureMeshMlsOperationState::Delivered
                );
            }

            licoup_foundation::platform::paths::set_portable_data_dir_override(previous);
            let _ = std::fs::remove_dir_all(root);
        }
    }
}
