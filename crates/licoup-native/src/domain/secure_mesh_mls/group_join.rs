use anyhow::{Result, ensure};
use serde_json::{Value, json};
use time::OffsetDateTime;

use crate::core::secure_mesh_mls::SecureMeshMlsGroup;
use crate::core::secure_mesh_mls_product::{
    SecureMeshMlsExpectedInvitation, SecureMeshMlsOperationState, cross_check_roster,
    join_product_group_from_welcome_prepared, require_verified_member_trust,
};

use super::group_state::{group_status_json, reconcile_group_metadata, require_group_base_current};
use super::input_codec::{
    GroupJoinRequest, MAX_GROUP_ID_BYTES, MAX_MLS_MESSAGE_BYTES, decode_base64url, hex_sha256,
    parse_params, reject_caller_asserted_trust, trusted_roster,
};
use super::journal_recovery::{
    abort_empty_prepared_on_error, commit_staged_journaled_operation, current_group_metadata,
    journal_operation_identity, open_security_ledger, resume_journaled_operation,
};
use super::participant_runtime::{
    ParticipantRequirement, group_state_store, with_local_participant,
};

pub(super) fn group_join(params: &Value) -> Result<Value> {
    reject_caller_asserted_trust(params)?;
    let request: GroupJoinRequest = parse_params(params)?;
    let group_id = decode_base64url(
        &request.group_id_base64url,
        "MLS group id",
        MAX_GROUP_ID_BYTES,
    )?;
    let welcome = decode_base64url(
        &request.welcome_message_base64url,
        "MLS welcome message",
        MAX_MLS_MESSAGE_BYTES,
    )?;
    let inviter_identity = request.inviter_identity.to_identity()?;
    ensure!(
        request
            .expected_roster_endpoint_ids
            .iter()
            .collect::<std::collections::BTreeSet<_>>()
            .len()
            == request.expected_roster_endpoint_ids.len(),
        "secure mesh MLS expected roster contains a duplicate endpoint"
    );
    let invitation = SecureMeshMlsExpectedInvitation::new(
        &group_id,
        inviter_identity.endpoint_id.clone(),
        request.expected_roster_endpoint_ids.clone(),
    )?;
    with_local_participant(params, ParticipantRequirement::Required, |runtime| {
        let trusted_roster =
            trusted_roster(&request.trusted_roster, runtime.config, runtime.identity)?;
        let inviter_trust_state = trusted_roster.state_for(&inviter_identity)?.clone();
        let mut security_ledger = open_security_ledger()?;
        let (operation_id, request_digest) =
            journal_operation_identity("secure_mesh.mls.group.join", &request, runtime.identity)?;
        let mut operation = security_ledger.begin_operation(
            &operation_id,
            "secure_mesh.mls.group.join",
            &request_digest,
            runtime.identity,
            OffsetDateTime::now_utc().unix_timestamp(),
        )?;
        if let Some(group) = SecureMeshMlsGroup::load_optional(runtime.participant, &group_id)? {
            ensure!(
                inviter_identity.endpoint_id == invitation.inviter_endpoint_id,
                "secure mesh MLS inviter identity does not match invitation"
            );
            require_verified_member_trust(&inviter_trust_state)?;
            trusted_roster.state_for(runtime.identity)?;
            cross_check_roster(
                &invitation.expected_roster_endpoint_ids,
                &group.member_credential_identities()?,
                &trusted_roster.identities,
            )?;
            group.require_active_capability_negotiation()?;
            if let Some(response) = resume_journaled_operation(
                group_state_store(&mut *runtime.group_store)?,
                &mut security_ledger,
                operation.clone(),
                Some(&group),
                runtime.identity,
            )? {
                return Ok((response, false));
            }
            ensure!(
                operation.state == SecureMeshMlsOperationState::Prepared,
                "secure mesh MLS joined state conflicts with an incomplete operation"
            );
            let metadata = reconcile_group_metadata(
                group_state_store(&mut *runtime.group_store)?,
                &group,
                runtime.identity,
            )?;
            ensure!(
                security_ledger.abort_empty_prepared_operation(&operation_id)?,
                "secure mesh MLS joined-state no-op did not release its empty journal entry"
            );
            return Ok((group_status_json(&group, &metadata), false));
        }
        if operation.state == SecureMeshMlsOperationState::CryptoPrepared {
            operation = security_ledger.reset_crypto_prepared_operation_for_retry(
                &operation_id,
                OffsetDateTime::now_utc().unix_timestamp(),
            )?;
        }
        ensure!(
            operation.state == SecureMeshMlsOperationState::Prepared,
            "secure mesh MLS join operation lost selected-custody state"
        );
        let staged_result = (|| {
            let join_group_id_hash = format!("sha256:{}", hex_sha256(&group_id));
            let participant_scope = runtime.identity.fingerprint()?;
            require_group_base_current(
                group_state_store(&mut *runtime.group_store)?,
                None,
                &join_group_id_hash,
                &participant_scope,
            )?;
            let now = OffsetDateTime::now_utc();
            let (group, prepared_security) = join_product_group_from_welcome_prepared(
                runtime.participant,
                runtime.identity,
                &invitation,
                &inviter_identity,
                &inviter_trust_state,
                &trusted_roster.identities,
                &crate::core::secure_mesh_mls::SecureMeshMlsWelcome {
                    commit_message: Vec::new(),
                    welcome_message: welcome,
                },
                now,
            )?;
            let expected = current_group_metadata(&group, runtime.identity)?;
            let staged = security_ledger.stage_operation(
                &operation_id,
                &json!({}),
                &group_id,
                None,
                &expected,
                &prepared_security,
                now.unix_timestamp(),
            )?;
            Ok((group, staged))
        })();
        let (group, staged) =
            abort_empty_prepared_on_error(&mut security_ledger, &operation_id, staged_result)?;
        let response =
            commit_staged_journaled_operation(runtime, &mut security_ledger, staged, &group)?;
        Ok((response, false))
    })
}
