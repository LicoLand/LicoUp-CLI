pub(super) use super::super::capability_proof::*;
pub(super) use super::super::constants::*;
pub(super) use super::super::helpers::*;
pub(super) use super::super::ledger_transaction::{
    consume_prepared_security_transaction, mls_security_scope_hash,
};
pub(super) use super::super::security_ledger::{
    PreparedMlsCapabilityProofUse, PreparedMlsKeyPackageUse, PreparedMlsSecurityInputs,
};
pub(super) use super::super::*;
pub(super) use crate::core::secure_mesh::SECURE_MESH_PROTOCOL_BUILD_REVISION;
pub(super) use crate::core::secure_mesh_capability::{CapabilityEvaluation, capability_catalog};
pub(super) use crate::core::secure_mesh_capability_proof::{
    CAPABILITY_PROOF_MAX_CLOCK_SKEW_SECONDS, CAPABILITY_PROOF_MAX_LIFETIME_SECONDS,
    CapabilityProofRequest, sign_capability_proof,
};
pub(super) use crate::core::secure_mesh_crypto::{
    SecureMeshContentContext, SecureMeshPayloadKind, SecureMeshPlaintext,
};
pub(super) use crate::core::secure_mesh_directory::{
    AuthorizedDirectoryLeaf, DirectoryAuthorizationPurpose, SecureMeshDirectoryAuthority,
    SecureMeshDirectoryKeyMaterialCommitment, SecureMeshDirectoryLeafClaim,
    UntrustedDirectoryResponse,
};
pub(super) use crate::core::secure_mesh_mls::{
    SecureMeshMlsCapabilityExtension, SecureMeshMlsGroup, SecureMeshMlsGroupMetadata,
    SecureMeshMlsKeyPackage, SecureMeshMlsParticipant, SecureMeshMlsRosterTransition,
    SecureMeshMlsWelcome,
};
pub(super) use crate::core::secure_mesh_transparency::{
    KtFreshnessPolicy, SecureMeshKtLog, SecureMeshTransparencyLeafBody, directory_scope_commitment,
};
pub(super) use crate::core::secure_mesh_trust::{DeviceTrustPublicIdentity, DeviceTrustState};
pub(super) use anyhow::Result;
pub(super) use ed25519_dalek::SigningKey;
pub(super) use rand_core::OsRng;
pub(super) use rusqlite::{TransactionBehavior, params};
pub(super) use serde_json::Value;
pub(super) use std::collections::{BTreeMap, BTreeSet};
pub(super) use std::time::{SystemTime, UNIX_EPOCH};
pub(super) use time::OffsetDateTime;

pub(super) struct DeviceFixture {
    pub(super) identity: DeviceTrustPublicIdentity,
    pub(super) signing_key: SigningKey,
    pub(super) participant: SecureMeshMlsParticipant,
}

pub(super) fn device(endpoint_id: &str) -> DeviceFixture {
    let identity_key = SigningKey::generate(&mut OsRng);
    let signing_key = SigningKey::generate(&mut OsRng);
    let identity = DeviceTrustPublicIdentity::new(
        endpoint_id,
        identity_key.verifying_key().to_bytes(),
        signing_key.verifying_key().to_bytes(),
        1,
    )
    .unwrap();
    let participant = participant_from_device_identity(&identity, &signing_key).unwrap();
    DeviceFixture {
        identity,
        signing_key,
        participant,
    }
}

pub(super) fn capability_evaluation() -> CapabilityEvaluation {
    let facts = crate::core::secure_mesh_capability::mandatory_protocol_facts(
        crate::core::secure_mesh_capability::CapabilityEvidenceKind::TestFixture,
    )
    .unwrap();
    capability_catalog().unwrap().evaluate(&facts).unwrap()
}

pub(super) fn capability_now() -> OffsetDateTime {
    OffsetDateTime::from_unix_timestamp(1_900_000_000).unwrap()
}

pub(super) fn authorized_member_add_directory(
    member: &DeviceFixture,
    member_key_package: &SecureMeshMlsKeyPackage,
    member_directory_version: u64,
    member_key_package_version: u64,
    issued_at: OffsetDateTime,
    purpose: DirectoryAuthorizationPurpose,
) -> AuthorizedDirectoryLeaf {
    let mut log = SecureMeshKtLog::new(SigningKey::generate(&mut OsRng));
    let claim = SecureMeshDirectoryLeafClaim {
        endpoint: SecureMeshTransparencyLeafBody {
            directory_scope_commitment: directory_scope_commitment(
                "test-tenant",
                "test-account",
                "test-workspace",
            ),
            endpoint_id: member.identity.endpoint_id.clone(),
            endpoint_kind: "test".to_string(),
            identity_public_key: member
                .identity
                .identity_public_key
                .iter()
                .map(|byte| format!("{byte:02x}"))
                .collect(),
            signing_public_key: member
                .identity
                .signing_public_key
                .iter()
                .map(|byte| format!("{byte:02x}"))
                .collect(),
            fingerprint: member.identity.fingerprint().unwrap(),
            rotation_epoch: member.identity.rotation_epoch,
            directory_state: "active".to_string(),
            updated_at: "2026-07-12T00:00:00Z".to_string(),
        },
        key_material: SecureMeshDirectoryKeyMaterialCommitment {
            signed_prekey_bundle_digest: hex_sha256(b"test-signed-prekey-bundle"),
            one_time_prekey_batch_digest: hex_sha256(b"test-one-time-prekey-batch"),
            pairwise_prekey_version: 1,
            mls_key_package_digest: hex_sha256(member_key_package.as_public_bytes()),
            mls_key_package_version: member_key_package_version,
        },
        directory_version: member_directory_version,
    };
    let leaf_index = log
        .append_hashed_directory_leaf(
            &claim.stable_label(),
            claim.version(),
            claim.revoked(),
            claim.leaf_hash().unwrap(),
        )
        .unwrap();
    let issued_at = u64::try_from(issued_at.unix_timestamp()).unwrap();
    let response = UntrustedDirectoryResponse {
        claim: claim.clone(),
        inclusion: log.inclusion_proof_at(leaf_index, issued_at).unwrap(),
        latest_map: log.map_proof_at(&claim.stable_label(), issued_at).unwrap(),
        consistency: None,
    };
    let mut authority = SecureMeshDirectoryAuthority::open_in_memory(
        log.pin(),
        KtFreshnessPolicy::strict(60, 2).unwrap(),
    )
    .unwrap();
    authority.authorize(response, purpose, issued_at).unwrap()
}

pub(super) fn begin_test_journal_operation(
    ledger: &mut SecureMeshMlsSecurityLedger,
    action: &str,
    request_binding: &[u8],
    identity: &DeviceTrustPublicIdentity,
    now: OffsetDateTime,
) -> Result<String> {
    let request_digest = hex_sha256(request_binding);
    let mut operation_binding = Vec::new();
    operation_binding.extend_from_slice(b"LICO-SM-MLS-TEST-OPERATION-v1");
    operation_binding.extend_from_slice(action.as_bytes());
    operation_binding.extend_from_slice(identity.fingerprint()?.as_bytes());
    operation_binding.extend_from_slice(request_digest.as_bytes());
    let operation_id = hex_sha256(&operation_binding);
    ledger.begin_operation(
        &operation_id,
        action,
        &request_digest,
        identity,
        now.unix_timestamp(),
    )?;
    Ok(operation_id)
}

#[allow(clippy::too_many_arguments)]
pub(super) fn stage_test_journal_operation(
    ledger: &mut SecureMeshMlsSecurityLedger,
    operation_id: &str,
    group_id: &[u8],
    base: Option<&SecureMeshMlsGroupMetadata>,
    expected: &SecureMeshMlsGroupMetadata,
    prepared: &PreparedMlsSecurityInputs,
    response: &Value,
    now: OffsetDateTime,
) -> Result<SecureMeshMlsOperationRecord> {
    match ledger.stage_operation(
        operation_id,
        response,
        group_id,
        base,
        expected,
        prepared,
        now.unix_timestamp(),
    ) {
        Ok(staged) => Ok(staged),
        Err(error) => {
            ledger.abort_empty_prepared_operation(operation_id)?;
            Err(error)
        }
    }
}

#[allow(clippy::too_many_arguments)]
pub(super) fn commit_test_journal_operation(
    ledger: &mut SecureMeshMlsSecurityLedger,
    operation_id: &str,
    group_id: &[u8],
    base: Option<&SecureMeshMlsGroupMetadata>,
    expected: &SecureMeshMlsGroupMetadata,
    prepared: &PreparedMlsSecurityInputs,
    response: &Value,
    now: OffsetDateTime,
) -> Result<()> {
    let staged = stage_test_journal_operation(
        ledger,
        operation_id,
        group_id,
        base,
        expected,
        prepared,
        response,
        now,
    )?;
    let committed =
        ledger.commit_operation_crypto(&staged.operation_id, expected, now.unix_timestamp())?;
    let reconciled = ledger.mark_operation_metadata_reconciled(
        &committed.operation_id,
        response,
        now.unix_timestamp(),
    )?;
    ledger.mark_operation_delivered(&reconciled.operation_id, now.unix_timestamp())?;
    Ok(())
}

#[allow(clippy::too_many_arguments)]
pub(super) fn process_test_product_commit(
    group: &mut SecureMeshMlsGroup,
    participant: &SecureMeshMlsParticipant,
    observing_identity: &DeviceTrustPublicIdentity,
    committer_identity: &DeviceTrustPublicIdentity,
    committer_trust_state: &DeviceTrustState,
    added_member_identity: Option<&DeviceTrustPublicIdentity>,
    removed_member_identity: Option<&DeviceTrustPublicIdentity>,
    trusted_roster: &BTreeMap<String, DeviceTrustPublicIdentity>,
    commit_message: &[u8],
    ledger: &mut SecureMeshMlsSecurityLedger,
    now: OffsetDateTime,
) -> Result<()> {
    let group_id = group.group_id_bytes()?;
    let base = group.public_metadata(observing_identity.fingerprint()?)?;
    let mut request_binding = Vec::new();
    request_binding.extend_from_slice(commit_message);
    request_binding.extend_from_slice(base.public_state_digest.as_bytes());
    request_binding.extend_from_slice(committer_identity.endpoint_id.as_bytes());
    let operation_id = begin_test_journal_operation(
        ledger,
        "secure_mesh.mls.commit.process",
        &request_binding,
        observing_identity,
        now,
    )?;
    let prepared = match process_product_commit_prepared(
        group,
        participant,
        observing_identity,
        committer_identity,
        committer_trust_state,
        added_member_identity,
        removed_member_identity,
        trusted_roster,
        commit_message,
        now,
    ) {
        Ok(prepared) => prepared,
        Err(error) => {
            ledger.abort_empty_prepared_operation(&operation_id)?;
            return Err(error);
        }
    };
    let expected = group.public_metadata(observing_identity.fingerprint()?)?;
    commit_test_journal_operation(
        ledger,
        &operation_id,
        &group_id,
        Some(&base),
        &expected,
        &prepared,
        &serde_json::json!({"ok": true}),
        now,
    )
}

pub(super) fn add_test_product_member(
    group: &mut SecureMeshMlsGroup,
    owner: &DeviceFixture,
    member: &DeviceFixture,
    member_key_package: &SecureMeshMlsKeyPackage,
    ledger: &mut SecureMeshMlsSecurityLedger,
    key_package_id: &str,
) -> SecureMeshMlsWelcome {
    add_test_product_member_with_times(
        group,
        owner,
        member,
        member_key_package,
        ledger,
        key_package_id,
        capability_now(),
        capability_now(),
    )
    .unwrap()
}

pub(super) fn add_test_product_member_with_times(
    group: &mut SecureMeshMlsGroup,
    owner: &DeviceFixture,
    member: &DeviceFixture,
    member_key_package: &SecureMeshMlsKeyPackage,
    ledger: &mut SecureMeshMlsSecurityLedger,
    key_package_id: &str,
    member_proof_issued_at: OffsetDateTime,
    accepted_at: OffsetDateTime,
) -> Result<SecureMeshMlsWelcome> {
    let member_proof = sign_mls_keypackage_capability_proof(
        &member.identity,
        &member.signing_key,
        &capability_evaluation(),
        member_key_package,
        member_proof_issued_at,
    )
    .unwrap();
    let group_id = group.group_id_bytes()?;
    let base = group.public_metadata(owner.identity.fingerprint()?)?;
    let mut request_binding = Vec::new();
    request_binding.extend_from_slice(key_package_id.as_bytes());
    request_binding.extend_from_slice(member_key_package.as_public_bytes());
    request_binding.extend_from_slice(base.public_state_digest.as_bytes());
    let operation_id = begin_test_journal_operation(
        ledger,
        "secure_mesh.mls.member.add",
        &request_binding,
        &owner.identity,
        accepted_at,
    )?;
    let member_directory_version = 1;
    let member_key_package_version = 1;
    let member_directory_authorization = authorized_member_add_directory(
        member,
        member_key_package,
        member_directory_version,
        member_key_package_version,
        accepted_at,
        DirectoryAuthorizationPurpose::MlsMemberAdd,
    );
    let (welcome, prepared) = match add_product_member_prepared(
        group,
        &owner.participant,
        &owner.identity,
        &owner.signing_key,
        &capability_evaluation(),
        &DeviceTrustState::Verified,
        member_key_package,
        &member.identity,
        &member_proof,
        &DeviceTrustState::Verified,
        &member_directory_authorization,
        member_directory_version,
        member_key_package_version,
        key_package_id,
        accepted_at,
    ) {
        Ok(result) => result,
        Err(error) => {
            ledger.abort_empty_prepared_operation(&operation_id)?;
            return Err(error);
        }
    };
    let expected = group.public_metadata(owner.identity.fingerprint()?)?;
    commit_test_journal_operation(
        ledger,
        &operation_id,
        &group_id,
        Some(&base),
        &expected,
        &prepared,
        &serde_json::json!({"ok": true, "group": null}),
        accepted_at,
    )?;
    Ok(welcome)
}

pub(super) fn join_test_product_group(
    member: &DeviceFixture,
    inviter: &DeviceFixture,
    invitation: &SecureMeshMlsExpectedInvitation,
    welcome: &SecureMeshMlsWelcome,
    ledger: &mut SecureMeshMlsSecurityLedger,
) -> Result<SecureMeshMlsGroup> {
    let trusted_roster = BTreeMap::from([
        (
            inviter.identity.endpoint_id.clone(),
            inviter.identity.clone(),
        ),
        (member.identity.endpoint_id.clone(), member.identity.clone()),
    ]);
    join_test_product_group_with_roster(
        member,
        inviter,
        invitation,
        welcome,
        &trusted_roster,
        ledger,
    )
}

pub(super) fn join_test_product_group_with_roster(
    member: &DeviceFixture,
    inviter: &DeviceFixture,
    invitation: &SecureMeshMlsExpectedInvitation,
    welcome: &SecureMeshMlsWelcome,
    trusted_roster: &BTreeMap<String, DeviceTrustPublicIdentity>,
    ledger: &mut SecureMeshMlsSecurityLedger,
) -> Result<SecureMeshMlsGroup> {
    let mut request_binding = Vec::new();
    request_binding.extend_from_slice(&welcome.welcome_message);
    request_binding.extend_from_slice(&invitation.group_id);
    let operation_id = begin_test_journal_operation(
        ledger,
        "secure_mesh.mls.group.join",
        &request_binding,
        &member.identity,
        capability_now(),
    )?;
    let (group, prepared) = match join_product_group_from_welcome_prepared(
        &member.participant,
        &member.identity,
        invitation,
        &inviter.identity,
        &DeviceTrustState::Verified,
        trusted_roster,
        welcome,
        capability_now(),
    ) {
        Ok(result) => result,
        Err(error) => {
            ledger.abort_empty_prepared_operation(&operation_id)?;
            return Err(error);
        }
    };
    let expected = group.public_metadata(member.identity.fingerprint()?)?;
    commit_test_journal_operation(
        ledger,
        &operation_id,
        &invitation.group_id,
        None,
        &expected,
        &prepared,
        &serde_json::json!({"ok": true}),
        capability_now(),
    )?;
    Ok(group)
}

pub(super) fn ledger_path(name: &str) -> std::path::PathBuf {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let mut path = std::env::temp_dir();
    path.push(format!(
        "lico-mls-kp-{name}-{}-{nonce}.sqlite3",
        std::process::id()
    ));
    path
}

pub(super) fn journal_metadata(
    group_id: &[u8],
    participant_endpoint_id: &str,
    epoch: u64,
    state_label: &str,
) -> SecureMeshMlsGroupMetadata {
    SecureMeshMlsGroupMetadata {
        group_id_hash: format!("sha256:{}", hex_sha256(group_id)),
        public_state_digest: format!("sha256:{}", hex_sha256(state_label.as_bytes())),
        epoch,
        member_count: usize::try_from(epoch).unwrap_or(1).max(1),
        own_leaf_index: 0,
        active: true,
        participant_endpoint_id: participant_endpoint_id.to_string(),
    }
}
