use anyhow::{Result, anyhow, ensure};
use serde::Serialize;
use serde_json::Value;
use time::OffsetDateTime;

use crate::core::secure_mesh_mls::{
    SecureMeshMlsDurableStore, SecureMeshMlsGroup, SecureMeshMlsParticipant,
};
use crate::core::secure_mesh_mls_product::{
    SecureMeshMlsOperationRecord, SecureMeshMlsOperationState, SecureMeshMlsSecurityLedger,
};
use crate::core::secure_mesh_trust::DeviceTrustPublicIdentity;

use super::group_state::{group_status_json, reconcile_group_metadata};
use super::input_codec::hex_sha256;
use super::participant_runtime::{LocalParticipantRuntime, group_state_store};

pub(super) fn open_security_ledger() -> Result<SecureMeshMlsSecurityLedger> {
    SecureMeshMlsSecurityLedger::open(
        crate::domain::mobile_relay::secure_mesh_mls_state_dir()?.join("security-ledger.sqlite3"),
    )
}

pub(super) fn journal_operation_identity<T: Serialize>(
    action: &str,
    request: &T,
    identity: &DeviceTrustPublicIdentity,
) -> Result<(String, String)> {
    let request_bytes = serde_json::to_vec(request)
        .map_err(|_| anyhow!("secure mesh MLS operation request encoding failed"))?;
    let request_digest = hex_sha256(&request_bytes);
    let mut transcript = Vec::new();
    transcript.extend_from_slice(b"LICO-SM-MLS-OPERATION-v1");
    transcript.extend_from_slice(action.as_bytes());
    transcript.extend_from_slice(identity.fingerprint()?.as_bytes());
    transcript.extend_from_slice(request_digest.as_bytes());
    Ok((hex_sha256(&transcript), request_digest))
}

pub(super) fn recover_incomplete_writer_operations(
    group_store: &mut Option<SecureMeshMlsDurableStore>,
    participant: &SecureMeshMlsParticipant,
    identity: &DeviceTrustPublicIdentity,
) -> Result<()> {
    let mut ledger = open_security_ledger()?;
    for mut operation in ledger.incomplete_writer_operations(identity)? {
        let group_id = operation
            .group_id
            .as_ref()
            .ok_or_else(|| anyhow!("secure mesh MLS incomplete operation group id is missing"))?;
        let group = SecureMeshMlsGroup::load_optional(participant, group_id)?;
        match operation.state {
            SecureMeshMlsOperationState::CryptoPrepared => match group {
                None => {
                    ensure!(
                        operation.base_metadata.is_none(),
                        "secure mesh MLS incomplete operation lost its base snapshot"
                    );
                    ledger.reset_crypto_prepared_operation_for_retry(
                        &operation.operation_id,
                        OffsetDateTime::now_utc().unix_timestamp(),
                    )?;
                }
                Some(group) => {
                    let observed = current_group_metadata(&group, identity)?;
                    if operation.expected_metadata.as_ref() == Some(&observed) {
                        operation = ledger.commit_operation_crypto(
                            &operation.operation_id,
                            &observed,
                            OffsetDateTime::now_utc().unix_timestamp(),
                        )?;
                        let _ = finish_journaled_operation(
                            group_state_store(group_store)?,
                            &mut ledger,
                            operation,
                            &group,
                            identity,
                        )?;
                    } else if operation.base_metadata.as_ref() == Some(&observed) {
                        ledger.reset_crypto_prepared_operation_for_retry(
                            &operation.operation_id,
                            OffsetDateTime::now_utc().unix_timestamp(),
                        )?;
                    } else {
                        return Err(anyhow!(
                            "secure mesh MLS incomplete operation snapshot matches neither base nor expected state"
                        ));
                    }
                }
            },
            SecureMeshMlsOperationState::CryptoCommitted => {
                let group = group.ok_or_else(|| {
                    anyhow!("secure mesh MLS committed operation group snapshot is missing")
                })?;
                let observed = current_group_metadata(&group, identity)?;
                ensure!(
                    operation.expected_metadata.as_ref() == Some(&observed),
                    "secure mesh MLS committed operation snapshot diverges"
                );
                let _ = finish_journaled_operation(
                    group_state_store(group_store)?,
                    &mut ledger,
                    operation,
                    &group,
                    identity,
                )?;
            }
            _ => {
                return Err(anyhow!(
                    "secure mesh MLS writer reservation has an invalid journal state"
                ));
            }
        }
    }
    Ok(())
}

pub(super) fn current_group_metadata(
    group: &SecureMeshMlsGroup,
    identity: &DeviceTrustPublicIdentity,
) -> Result<crate::core::secure_mesh_mls::SecureMeshMlsGroupMetadata> {
    group.public_metadata(identity.fingerprint()?)
}

pub(super) fn abort_empty_prepared_on_error<T>(
    ledger: &mut SecureMeshMlsSecurityLedger,
    operation_id: &str,
    result: Result<T>,
) -> Result<T> {
    match result {
        Ok(value) => Ok(value),
        Err(error) => {
            let removed = ledger.abort_empty_prepared_operation(operation_id)?;
            if !removed {
                if let Some(record) = ledger.operation(operation_id)? {
                    ensure!(
                        record.state != SecureMeshMlsOperationState::Prepared,
                        "secure mesh MLS failed input left an unabortable prepared operation"
                    );
                }
            }
            Err(error)
        }
    }
}

pub(super) fn resume_journaled_operation(
    group_store: &mut SecureMeshMlsDurableStore,
    ledger: &mut SecureMeshMlsSecurityLedger,
    mut record: SecureMeshMlsOperationRecord,
    group: Option<&SecureMeshMlsGroup>,
    identity: &DeviceTrustPublicIdentity,
) -> Result<Option<Value>> {
    if record.state == SecureMeshMlsOperationState::Prepared {
        return Ok(None);
    }
    if record.state == SecureMeshMlsOperationState::MetadataReconciled {
        record = ledger.mark_operation_delivered(
            &record.operation_id,
            OffsetDateTime::now_utc().unix_timestamp(),
        )?;
        return record
            .response
            .map(Some)
            .ok_or_else(|| anyhow!("secure mesh MLS reconciled response is missing"));
    }
    if record.state == SecureMeshMlsOperationState::Delivered {
        return record
            .response
            .map(Some)
            .ok_or_else(|| anyhow!("secure mesh MLS delivered response is missing"));
    }
    if group.is_none()
        && record.state == SecureMeshMlsOperationState::CryptoPrepared
        && record.base_metadata.is_none()
    {
        ledger.reset_crypto_prepared_operation_for_retry(
            &record.operation_id,
            OffsetDateTime::now_utc().unix_timestamp(),
        )?;
        return Ok(None);
    }
    let group = group.ok_or_else(|| {
        anyhow!("secure mesh MLS committed operation is missing selected-custody group state")
    })?;
    let observed = current_group_metadata(group, identity)?;
    let expected = record
        .expected_metadata
        .as_ref()
        .ok_or_else(|| anyhow!("secure mesh MLS operation journal expected metadata is missing"))?;
    if record.state == SecureMeshMlsOperationState::CryptoPrepared {
        if &observed != expected {
            let base = record.base_metadata.as_ref().ok_or_else(|| {
                anyhow!("secure mesh MLS prepared operation has no recoverable base state")
            })?;
            ensure!(
                &observed == base,
                "secure mesh MLS prepared operation snapshot matches neither base nor expected state"
            );
            return Ok(None);
        }
        record = ledger.commit_operation_crypto(
            &record.operation_id,
            &observed,
            OffsetDateTime::now_utc().unix_timestamp(),
        )?;
    } else {
        ensure!(
            &observed == expected,
            "secure mesh MLS operation journal detected selected-custody rollback"
        );
    }
    finish_journaled_operation(group_store, ledger, record, group, identity).map(Some)
}

fn finish_journaled_operation(
    group_store: &mut SecureMeshMlsDurableStore,
    ledger: &mut SecureMeshMlsSecurityLedger,
    mut record: SecureMeshMlsOperationRecord,
    group: &SecureMeshMlsGroup,
    identity: &DeviceTrustPublicIdentity,
) -> Result<Value> {
    if record.state == SecureMeshMlsOperationState::CryptoCommitted {
        journal_failpoint("after_crypto_commit_before_metadata")?;
        let durable = reconcile_group_metadata(group_store, group, identity)?;
        let group_status = group_status_json(group, &durable);
        let mut final_response = record
            .response
            .clone()
            .ok_or_else(|| anyhow!("secure mesh MLS operation journal response is missing"))?;
        if matches!(
            record.action.as_str(),
            "secure_mesh.mls.member.add" | "secure_mesh.mls.member.remove"
        ) {
            final_response["group"] = group_status;
        } else {
            final_response = group_status;
        }
        record = ledger.mark_operation_metadata_reconciled(
            &record.operation_id,
            &final_response,
            OffsetDateTime::now_utc().unix_timestamp(),
        )?;
    }
    if record.state == SecureMeshMlsOperationState::MetadataReconciled {
        journal_failpoint("after_metadata_before_delivery")?;
        record = ledger.mark_operation_delivered(
            &record.operation_id,
            OffsetDateTime::now_utc().unix_timestamp(),
        )?;
    }
    ensure!(
        record.state == SecureMeshMlsOperationState::Delivered,
        "secure mesh MLS operation journal did not reach delivery state"
    );
    record
        .response
        .ok_or_else(|| anyhow!("secure mesh MLS delivered operation response is missing"))
}

pub(super) fn commit_staged_journaled_operation(
    runtime: &mut LocalParticipantRuntime<'_>,
    ledger: &mut SecureMeshMlsSecurityLedger,
    staged: SecureMeshMlsOperationRecord,
    group: &SecureMeshMlsGroup,
) -> Result<Value> {
    journal_failpoint("after_stage_before_snapshot")?;
    runtime.persist_participant()?;
    journal_failpoint("after_snapshot_before_crypto_commit")?;
    let observed = current_group_metadata(group, runtime.identity)?;
    let committed = ledger.commit_operation_crypto(
        &staged.operation_id,
        &observed,
        OffsetDateTime::now_utc().unix_timestamp(),
    )?;
    finish_journaled_operation(
        group_state_store(&mut *runtime.group_store)?,
        ledger,
        committed,
        group,
        runtime.identity,
    )
}

#[cfg(not(test))]
pub(super) fn journal_failpoint(_name: &str) -> Result<()> {
    Ok(())
}

#[cfg(test)]
std::thread_local! {
    static MLS_JOURNAL_FAILPOINT: std::cell::Cell<Option<&'static str>> = const {
        std::cell::Cell::new(None)
    };
}

#[cfg(test)]
pub(super) struct MlsJournalFailpointGuard;

#[cfg(test)]
impl Drop for MlsJournalFailpointGuard {
    fn drop(&mut self) {
        MLS_JOURNAL_FAILPOINT.with(|failpoint| failpoint.set(None));
    }
}

#[cfg(test)]
pub(super) fn set_journal_failpoint(name: &'static str) -> MlsJournalFailpointGuard {
    MLS_JOURNAL_FAILPOINT.with(|failpoint| {
        assert!(
            failpoint.replace(Some(name)).is_none(),
            "secure mesh MLS journal failpoint is already active on this test thread"
        );
    });
    MlsJournalFailpointGuard
}

#[cfg(test)]
pub(super) fn journal_failpoint(name: &str) -> Result<()> {
    MLS_JOURNAL_FAILPOINT.with(|failpoint| {
        if failpoint.get() == Some(name) {
            failpoint.set(None);
            Err(anyhow!("secure mesh MLS injected journal boundary failure"))
        } else {
            Ok(())
        }
    })
}
