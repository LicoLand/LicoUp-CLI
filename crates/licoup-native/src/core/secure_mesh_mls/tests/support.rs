pub(super) use std::collections::BTreeMap;
pub(super) use std::path::PathBuf;
pub(super) use std::sync::Arc;
pub(super) use std::time::{SystemTime, UNIX_EPOCH};

pub(super) use crate::core::secure_mesh_capability::{
    CapabilityEvidenceKind, capability_catalog, mandatory_protocol_facts,
};
pub(super) use crate::core::secure_mesh_capability_proof::{
    CapabilityProofRequest, CapabilityProofVerificationContext, sign_capability_proof,
    verify_capability_proof,
};
pub(super) use crate::core::secure_mesh_crypto::{
    SecureMeshContentContext, SecureMeshPayloadKind, SecureMeshPlaintext,
    open_private_context_payload,
};
pub(super) use crate::core::secure_mesh_mls_pq_epoch::{
    create_mlkem1024_epoch_extension, mlkem1024_member_id,
};
pub(super) use crate::core::secure_mesh_secret_store::{
    SecretStoreAuthorizationRequest, SecretStoreHandle, SecureMeshSecretStore,
};
pub(super) use crate::core::secure_mesh_session_negotiation::create_mls_capability_binding;
pub(super) use crate::core::secure_mesh_trust::DeviceTrustPublicIdentity;
pub(super) use crate::platform::secure_mesh_secret_store::EphemeralSecretStore;
pub(super) use anyhow::Result;
pub(super) use ed25519_dalek::SigningKey;
pub(super) use openmls::prelude::{
    MlsMessageBodyIn, MlsMessageIn, ProcessedMessageContent,
    tls_codec::{Deserialize as TlsDeserialize, Serialize as TlsSerialize},
};
pub(super) use rusqlite::{Connection, params};

pub(super) use super::super::capability_extension::{
    SecureMeshMlsCapabilityExtension, SecureMeshMlsMemberCapabilityProof,
    SecureMeshMlsRosterTransition, secure_mesh_mls_capability_extension_digest,
    secure_mesh_mls_group_context_extensions_with_pq,
};
pub(super) use super::super::codec::{deserialize_protocol_message, hash_bytes};
pub(super) use super::super::constants::{
    MLS_CAPABILITY_EXTENSION_SCHEMA_VERSION, MLS_EPOCH_SECRET_STORE_CLASS,
    MLS_PRIVATE_CONTEXT_PAYLOAD_MAGIC, MLS_PUBLIC_STATE_DIGEST_AUTHENTICATED_BACKFILL,
    MLS_RECOVERY_SECRET_STORE_CLASS, SECURE_MESH_MLS_APPLICATION_PUBLIC_AAD,
};
pub(super) use super::super::group_model::SecureMeshMlsGroup;
pub(super) use super::super::key_package::SecureMeshMlsKeyPackage;
pub(super) use super::super::participant::SecureMeshMlsParticipant;
pub(super) use super::super::private_context_codec::decode_mls_private_context_payload;
pub(super) use super::super::provider::SecureMeshOpenMlsProvider;
pub(super) use super::super::{SecureMeshMlsDurableStore, runtime_crypto_self_test};

pub(super) fn durable_store_path(test_name: &str) -> PathBuf {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let mut path = std::env::temp_dir();
    path.push(format!(
        "lico-secure-mesh-mls-{test_name}-{}-{nonce}.sqlite3",
        std::process::id()
    ));
    path
}

pub(super) fn test_secret_store() -> Arc<dyn SecureMeshSecretStore> {
    Arc::new(EphemeralSecretStore::new())
}

pub(super) fn activate_test_payload_capabilities(
    group: &mut SecureMeshMlsGroup,
    participant: &SecureMeshMlsParticipant,
    members: &[&SecureMeshMlsParticipant],
) -> Vec<u8> {
    let evaluation = capability_catalog()
        .unwrap()
        .evaluate(&mandatory_protocol_facts(CapabilityEvidenceKind::TestFixture).unwrap())
        .unwrap();
    let first_key = SigningKey::from_bytes(&[0x31; 32]);
    let second_key = SigningKey::from_bytes(&[0x32; 32]);
    let first_identity = DeviceTrustPublicIdentity::new(
        "mls:test-capability-first",
        [0x41; 32],
        first_key.verifying_key().to_bytes(),
        1,
    )
    .unwrap();
    let second_identity = DeviceTrustPublicIdentity::new(
        "mls:test-capability-second",
        [0x42; 32],
        second_key.verifying_key().to_bytes(),
        1,
    )
    .unwrap();
    let build_protocol_digest =
        crate::core::secure_mesh_capability_proof::encode_sha256_digest(&[0x51; 32]);
    let request = CapabilityProofRequest {
        build_protocol_digest: build_protocol_digest.clone(),
        policy_revision: 1,
        challenge: [0x61; 32],
        issued_at_unix_seconds: 1_900_000_000,
        expires_at_unix_seconds: 1_900_000_060,
    };
    let first_proof =
        sign_capability_proof(&first_identity, &first_key, &evaluation, &request).unwrap();
    let second_proof =
        sign_capability_proof(&second_identity, &second_key, &evaluation, &request).unwrap();
    let context = CapabilityProofVerificationContext {
        expected_build_protocol_digest: build_protocol_digest,
        expected_policy_revision: 1,
        expected_challenge: [0x61; 32],
        now_unix_seconds: 1_900_000_001,
    };
    let first_verified = verify_capability_proof(&first_identity, &first_proof, &context).unwrap();
    let second_verified =
        verify_capability_proof(&second_identity, &second_proof, &context).unwrap();
    let binding = create_mls_capability_binding(
        &first_verified,
        &second_verified,
        &crate::core::secure_mesh_capability_proof::encode_sha256_digest(&[0x71; 32]),
    )
    .unwrap();
    let previous_extension_digest =
        secure_mesh_mls_capability_extension_digest(&group.capability_extension().unwrap())
            .unwrap();
    let extension = SecureMeshMlsCapabilityExtension::Active {
        schema_version: MLS_CAPABILITY_EXTENSION_SCHEMA_VERSION,
        activated_at_epoch: group.epoch().saturating_add(1),
        previous_extension_digest: Some(previous_extension_digest),
        committer_endpoint_id: first_identity.endpoint_id.clone(),
        roster_transition: Box::new(SecureMeshMlsRosterTransition::MemberAdded {
            member_endpoint_id: second_identity.endpoint_id.clone(),
            pair_binding: binding.clone(),
        }),
        member_capability_proofs: BTreeMap::from([
            (
                first_identity.endpoint_id.clone(),
                SecureMeshMlsMemberCapabilityProof {
                    endpoint_id: first_identity.endpoint_id.clone(),
                    accepted_at_unix_seconds: request.issued_at_unix_seconds,
                    proof: first_proof,
                },
            ),
            (
                second_identity.endpoint_id.clone(),
                SecureMeshMlsMemberCapabilityProof {
                    endpoint_id: second_identity.endpoint_id.clone(),
                    accepted_at_unix_seconds: request.issued_at_unix_seconds,
                    proof: second_proof,
                },
            ),
        ]),
        group_negotiated_protocol_capabilities: binding.negotiated_protocol_capabilities.clone(),
    };
    let member_public_keys = members
        .iter()
        .map(|member| {
            Ok((
                mlkem1024_member_id(&member.credential_identity_bytes()?)?,
                member.provider.mlkem1024_seed.public_key(),
            ))
        })
        .collect::<Result<BTreeMap<_, _>>>()
        .unwrap();
    let (pq_epoch_extension, _) = create_mlkem1024_epoch_extension(
        group.group.group_id().as_slice(),
        group.epoch().saturating_add(1),
        None,
        &member_public_keys,
    )
    .unwrap();
    let (commit, _, _) = group
        .group
        .update_group_context_extensions(
            &participant.provider,
            secure_mesh_mls_group_context_extensions_with_pq(&extension, Some(&pq_epoch_extension))
                .unwrap(),
            &participant.signer,
        )
        .unwrap();
    let commit = commit.to_bytes().unwrap();
    group
        .group
        .merge_pending_commit(&participant.provider)
        .unwrap();
    group
        .refresh_authenticated_group_context(participant)
        .unwrap();
    assert_eq!(group.capability_extension().unwrap(), extension);
    commit
}

pub(super) fn process_test_payload_capability_commit(
    group: &mut SecureMeshMlsGroup,
    participant: &SecureMeshMlsParticipant,
    commit: &[u8],
) {
    group
        .process_commit_with_capability_verifier(
            participant,
            commit,
            true,
            |_, _, _| Ok(()),
            |_, _, _, _| Ok(()),
        )
        .unwrap();
}

pub(super) fn test_secret_store_handle(test_name: &str, secret_class: &str) -> SecretStoreHandle {
    SecretStoreHandle::new(
        format!("mls-test-{secret_class}-{test_name}"),
        "providerSnapshot",
    )
    .unwrap()
}

pub(super) fn content_context_fixture(
    message_id: &str,
    sender_endpoint_id: &str,
    recipient_endpoint_id: &str,
    session_id: String,
) -> SecureMeshContentContext {
    SecureMeshContentContext::new(
        format!("env_{message_id}"),
        message_id,
        format!("mailbox_{recipient_endpoint_id}"),
        sender_endpoint_id,
        recipient_endpoint_id,
        session_id,
        "2026-01-01T00:00:00.000Z",
        "2026-01-01T00:10:00.000Z",
    )
}

pub(super) fn active_payload_group_pair(
    group_id: &[u8],
) -> (
    SecureMeshMlsParticipant,
    SecureMeshMlsParticipant,
    SecureMeshMlsGroup,
    SecureMeshMlsGroup,
) {
    let alice = SecureMeshMlsParticipant::new(b"desktop_gui:alice".to_vec()).unwrap();
    let bob = SecureMeshMlsParticipant::new(b"mobile:bob".to_vec()).unwrap();
    let bob_key_package = bob.generate_key_package().unwrap();
    let mut alice_group = SecureMeshMlsGroup::create(&alice, group_id).unwrap();
    let welcome = alice_group.add_member(&alice, &bob_key_package).unwrap();
    let mut bob_group =
        SecureMeshMlsGroup::join_from_welcome(&bob, &welcome.welcome_message).unwrap();
    let capability_commit =
        activate_test_payload_capabilities(&mut alice_group, &alice, &[&alice, &bob]);
    process_test_payload_capability_commit(&mut bob_group, &bob, &capability_commit);
    (alice, bob, alice_group, bob_group)
}
