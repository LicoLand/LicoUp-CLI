use anyhow::{Result, ensure};
use serde_json::{Value, json};
use time::OffsetDateTime;

use crate::core::secure_mesh_directory::DirectoryAuthorizationPurpose;
use crate::core::secure_mesh_mls::SecureMeshMlsKeyPackage;
use crate::core::secure_mesh_mls_product::{
    SecureMeshMlsOperationState, add_product_member_prepared, directory_roster_from_group,
    remove_product_member_prepared,
};
use crate::core::secure_mesh_trust::DeviceTrustState;

use super::directory_authorization::{
    authorize_member_directory_response, require_mls_directory_authority,
};
use super::group_state::{load_group_for_journal, require_group_base_current};
use super::input_codec::{
    MAX_GROUP_ID_BYTES, MAX_KEY_PACKAGE_BYTES, MemberAddRequest, MemberRemoveRequest,
    decode_base64url, encode_base64url, hex_sha256, parse_params, reject_caller_asserted_trust,
};
use super::journal_recovery::{
    abort_empty_prepared_on_error, commit_staged_journaled_operation, current_group_metadata,
    journal_operation_identity, open_security_ledger, resume_journaled_operation,
};
use super::participant_runtime::{
    ParticipantRequirement, group_state_store, with_local_participant,
};

pub(super) fn member_add(params: &Value) -> Result<Value> {
    reject_caller_asserted_trust(params)?;
    let request: MemberAddRequest = parse_params(params)?;
    let group_id = decode_base64url(
        &request.group_id_base64url,
        "MLS group id",
        MAX_GROUP_ID_BYTES,
    )?;
    let key_package_bytes = decode_base64url(
        &request.member_key_package_base64url,
        "MLS key package",
        MAX_KEY_PACKAGE_BYTES,
    )?;
    ensure!(
        request.member_key_package_id == hex_sha256(&key_package_bytes),
        "secure mesh MLS key package id is not canonically bound to the package"
    );
    let member_key_package = SecureMeshMlsKeyPackage::from_public_bytes(&key_package_bytes)?;
    let member_identity = request.member_identity.to_identity()?;
    with_local_participant(params, ParticipantRequirement::Required, |runtime| {
        ensure!(
            member_identity != *runtime.identity,
            "secure mesh MLS cannot add the local identity as a remote member"
        );
        let member_trust_state = runtime.authoritative_trust_state(&member_identity)?;
        let mut group = load_group_for_journal(
            group_state_store(&mut *runtime.group_store)?,
            runtime.participant,
            runtime.identity,
            &group_id,
        )?;
        let current_roster = directory_roster_from_group(&group)?;
        require_mls_directory_authority(runtime.config, runtime.identity, &current_roster)?;
        let mut security_ledger = open_security_ledger()?;
        let (operation_id, request_digest) =
            journal_operation_identity("secure_mesh.mls.member.add", &request, runtime.identity)?;
        let mut operation = security_ledger.begin_operation(
            &operation_id,
            "secure_mesh.mls.member.add",
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
            "secure mesh MLS member-add operation is not retryable"
        );
        let staged_result = (|| {
            let base = current_group_metadata(&group, runtime.identity)?;
            require_group_base_current(
                group_state_store(&mut *runtime.group_store)?,
                Some(&base),
                &base.group_id_hash,
                &base.participant_endpoint_id,
            )?;
            let capability_evaluation = runtime.secret_store.capability_evaluation()?;
            let now = OffsetDateTime::now_utc();
            let key_package_directory_authorization = authorize_member_directory_response(
                runtime.config,
                runtime.identity,
                request.untrusted_directory_response.clone(),
                now,
                DirectoryAuthorizationPurpose::MlsKeyPackage,
                &member_identity,
                request.member_directory_version,
                &request.member_key_package_id,
                request.member_key_package_version,
            )?;
            key_package_directory_authorization
                .require_purpose(DirectoryAuthorizationPurpose::MlsKeyPackage)?;
            let member_directory_authorization = authorize_member_directory_response(
                runtime.config,
                runtime.identity,
                request.untrusted_directory_response.clone(),
                now,
                DirectoryAuthorizationPurpose::MlsMemberAdd,
                &member_identity,
                request.member_directory_version,
                &request.member_key_package_id,
                request.member_key_package_version,
            )?;
            let mut next_roster = directory_roster_from_group(&group)?;
            next_roster.insert(member_identity.endpoint_id.clone(), member_identity.clone());
            require_mls_directory_authority(runtime.config, runtime.identity, &next_roster)?;
            let (welcome, prepared_security) = add_product_member_prepared(
                &mut group,
                runtime.participant,
                runtime.identity,
                runtime.signing_key,
                &capability_evaluation,
                &DeviceTrustState::Verified,
                &member_key_package,
                &member_identity,
                &request.member_capability_proof,
                &member_trust_state,
                &member_directory_authorization,
                request.member_directory_version,
                request.member_key_package_version,
                &request.member_key_package_id,
                now,
            )?;
            let base_response = json!({
                "ok": true,
                "group": Value::Null,
                "commitMessageBase64url": encode_base64url(&welcome.commit_message),
                "welcomeMessageBase64url": encode_base64url(&welcome.welcome_message),
                "memberEndpointId": member_identity.endpoint_id
            });
            let expected = current_group_metadata(&group, runtime.identity)?;
            security_ledger.stage_operation(
                &operation_id,
                &base_response,
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

pub(super) fn member_remove(params: &Value) -> Result<Value> {
    reject_caller_asserted_trust(params)?;
    let request: MemberRemoveRequest = parse_params(params)?;
    let group_id = decode_base64url(
        &request.group_id_base64url,
        "MLS group id",
        MAX_GROUP_ID_BYTES,
    )?;
    let removed_member_identity = request.member_identity.to_identity()?;
    with_local_participant(params, ParticipantRequirement::Required, |runtime| {
        ensure!(
            removed_member_identity.endpoint_id != runtime.identity.endpoint_id,
            "secure mesh MLS member-remove action cannot remove the local identity"
        );
        let mut group = load_group_for_journal(
            group_state_store(&mut *runtime.group_store)?,
            runtime.participant,
            runtime.identity,
            &group_id,
        )?;
        let mut security_ledger = open_security_ledger()?;
        let (operation_id, request_digest) = journal_operation_identity(
            "secure_mesh.mls.member.remove",
            &request,
            runtime.identity,
        )?;
        let mut operation = security_ledger.begin_operation(
            &operation_id,
            "secure_mesh.mls.member.remove",
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
            "secure mesh MLS member-remove operation is not retryable"
        );
        let staged_result = (|| {
            ensure!(
                group.epoch() == request.expected_epoch,
                "secure mesh MLS member-remove expected epoch is stale"
            );
            let removed_member_trust_state =
                runtime.authoritative_trust_state(&removed_member_identity)?;
            ensure!(
                matches!(
                    removed_member_trust_state,
                    DeviceTrustState::Verified
                        | DeviceTrustState::CrossSigned
                        | DeviceTrustState::Revoked
                ),
                "secure mesh MLS removed member identity is not locally verified"
            );
            let current_roster = directory_roster_from_group(&group)?;
            ensure!(
                current_roster.get(&runtime.identity.endpoint_id) == Some(runtime.identity),
                "secure mesh MLS remover is not the exact current roster identity"
            );
            ensure!(
                current_roster.get(&removed_member_identity.endpoint_id)
                    == Some(&removed_member_identity),
                "secure mesh MLS removed member is not the exact current roster identity"
            );
            let mut next_roster = current_roster;
            next_roster.remove(&removed_member_identity.endpoint_id);
            require_mls_directory_authority(runtime.config, runtime.identity, &next_roster)?;
            let base = current_group_metadata(&group, runtime.identity)?;
            require_group_base_current(
                group_state_store(&mut *runtime.group_store)?,
                Some(&base),
                &base.group_id_hash,
                &base.participant_endpoint_id,
            )?;
            let now = OffsetDateTime::now_utc();
            let (commit, prepared_security) = remove_product_member_prepared(
                &mut group,
                runtime.participant,
                runtime.identity,
                &DeviceTrustState::Verified,
                &removed_member_identity,
                &removed_member_trust_state,
                now,
            )?;
            let base_response = json!({
                "ok": true,
                "group": Value::Null,
                "commitMessageBase64url": encode_base64url(&commit.commit_message),
                "memberEndpointId": removed_member_identity.endpoint_id
            });
            let expected = current_group_metadata(&group, runtime.identity)?;
            security_ledger.stage_operation(
                &operation_id,
                &base_response,
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
