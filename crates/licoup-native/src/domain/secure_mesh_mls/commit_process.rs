use anyhow::{Result, ensure};
use serde_json::{Value, json};
use time::OffsetDateTime;

use crate::core::secure_mesh_mls_product::{
    SecureMeshMlsOperationState, process_product_commit_prepared,
};

use super::group_state::{load_group_for_journal, require_group_base_current};
use super::input_codec::{
    CommitProcessRequest, MAX_GROUP_ID_BYTES, MAX_MLS_MESSAGE_BYTES, PublicIdentityInput,
    decode_base64url, parse_params, reject_caller_asserted_trust, trusted_roster_with_local_policy,
};
use super::journal_recovery::{
    abort_empty_prepared_on_error, commit_staged_journaled_operation, current_group_metadata,
    journal_operation_identity, open_security_ledger, resume_journaled_operation,
};
use super::participant_runtime::{
    ParticipantRequirement, group_state_store, with_local_participant,
};

pub(super) fn commit_process(params: &Value) -> Result<Value> {
    reject_caller_asserted_trust(params)?;
    let request: CommitProcessRequest = parse_params(params)?;
    let group_id = decode_base64url(
        &request.group_id_base64url,
        "MLS group id",
        MAX_GROUP_ID_BYTES,
    )?;
    let commit = decode_base64url(
        &request.commit_message_base64url,
        "MLS commit message",
        MAX_MLS_MESSAGE_BYTES,
    )?;
    let committer_identity = request.committer_identity.to_identity()?;
    let added_member_identity = request
        .added_member_identity
        .as_ref()
        .map(PublicIdentityInput::to_identity)
        .transpose()?;
    let removed_member_identity = request
        .removed_member_identity
        .as_ref()
        .map(PublicIdentityInput::to_identity)
        .transpose()?;
    ensure!(
        added_member_identity.is_none() || removed_member_identity.is_none(),
        "secure mesh MLS commit cannot add and remove a member in one product transition"
    );
    with_local_participant(params, ParticipantRequirement::Required, |runtime| {
        let local_is_removed = removed_member_identity.as_ref() == Some(runtime.identity);
        let trusted_roster = trusted_roster_with_local_policy(
            &request.trusted_roster,
            runtime.config,
            runtime.identity,
            !local_is_removed,
        )?;
        let committer_trust_state = trusted_roster.state_for(&committer_identity)?.clone();
        let mut group = load_group_for_journal(
            group_state_store(&mut *runtime.group_store)?,
            runtime.participant,
            runtime.identity,
            &group_id,
        )?;
        let mut security_ledger = open_security_ledger()?;
        let (operation_id, request_digest) = journal_operation_identity(
            "secure_mesh.mls.commit.process",
            &request,
            runtime.identity,
        )?;
        let mut operation = security_ledger.begin_operation(
            &operation_id,
            "secure_mesh.mls.commit.process",
            &request_digest,
            runtime.identity,
            OffsetDateTime::now_utc().unix_timestamp(),
        )?;
        if let Some(response) = resume_journaled_operation(
            group_state_store(&mut *runtime.group_store)?,
            &mut security_ledger,
            operation.clone(),
            Some(&group),
            runtime.identity,
        )? {
            return Ok((response, false));
        }
        if operation.state == SecureMeshMlsOperationState::CryptoPrepared {
            operation = security_ledger.reset_crypto_prepared_operation_for_retry(
                &operation_id,
                OffsetDateTime::now_utc().unix_timestamp(),
            )?;
        }
        ensure!(
            operation.state == SecureMeshMlsOperationState::Prepared,
            "secure mesh MLS commit operation is not retryable"
        );
        let staged_result = (|| {
            let base = current_group_metadata(&group, runtime.identity)?;
            require_group_base_current(
                group_state_store(&mut *runtime.group_store)?,
                Some(&base),
                &base.group_id_hash,
                &base.participant_endpoint_id,
            )?;
            let now = OffsetDateTime::now_utc();
            let prepared_security = process_product_commit_prepared(
                &mut group,
                runtime.participant,
                runtime.identity,
                &committer_identity,
                &committer_trust_state,
                added_member_identity.as_ref(),
                removed_member_identity.as_ref(),
                &trusted_roster.identities,
                &commit,
                now,
            )?;
            let expected = current_group_metadata(&group, runtime.identity)?;
            security_ledger.stage_operation(
                &operation_id,
                &json!({}),
                &group_id,
                Some(&base),
                &expected,
                &prepared_security,
                now.unix_timestamp(),
            )
        })();
        let staged =
            abort_empty_prepared_on_error(&mut security_ledger, &operation_id, staged_result)?;
        let response =
            commit_staged_journaled_operation(runtime, &mut security_ledger, staged, &group)?;
        Ok((response, false))
    })
}
