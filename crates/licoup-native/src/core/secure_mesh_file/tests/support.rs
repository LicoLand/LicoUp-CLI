pub(super) use super::super::constants::*;
pub(super) use super::super::key_proof::*;
pub(super) use super::super::primitives::*;
pub(super) use super::super::*;
pub(super) use crate::core::secure_mesh_crypto::{
    SecureMeshContentContext, SecureMeshPayloadKind, SecureMeshPlaintext,
};
pub(super) use crate::core::secure_mesh_pairwise::{
    SecureMeshPairwisePrivateKey, SecureMeshPairwiseSession,
};
pub(super) use crate::core::secure_mesh_pqxdh::SecureMeshMlKem1024PreKeySeed;
pub(super) use crate::core::secure_mesh_prekey::{
    SecureMeshPairwisePreKeyBundle, SecureMeshPreKeyKind, SecureMeshPreKeyValidationPolicy,
    authorize_test_pairwise_prekey_bundle, sign_prekey_record,
};
pub(super) use crate::core::secure_mesh_trust::{DeviceTrustPublicIdentity, DeviceTrustState};
pub(super) use base64::{Engine as _, engine::general_purpose};
pub(super) use ed25519_dalek::SigningKey;
pub(super) use rand_core::OsRng;
pub(super) use serde_json::{Value, json};
pub(super) use sha2::{Digest, Sha256};
pub(super) use std::collections::HashSet;
pub(super) use time::{Duration, OffsetDateTime, format_description::well_known::Rfc3339};

pub(super) fn manifest_fixture() -> SecureMeshFileManifest {
    SecureMeshFileManifest {
        file_id: "file_test".to_string(),
        file_name: "quarterly-plan.pdf".to_string(),
        mime_type: "application/pdf".to_string(),
        relative_path: "workspace/reports".to_string(),
        total_size: 24,
        chunk_size: 8,
        chunk_count: 3,
    }
}

pub(super) fn manifest_json(manifest: &SecureMeshFileManifest) -> Value {
    json!({
        "fileId": &manifest.file_id,
        "fileName": &manifest.file_name,
        "mimeType": &manifest.mime_type,
        "relativePath": &manifest.relative_path,
        "totalSize": manifest.total_size,
        "chunkSize": manifest.chunk_size,
        "chunkCount": manifest.chunk_count
    })
}

pub(super) fn encrypted_chunks_fixture(
    key: &FileRootKey,
    manifest: &SecureMeshFileManifest,
) -> Vec<EncryptedSecureMeshFileChunk> {
    (0..manifest.chunk_count)
        .map(|index| {
            let chunk = SecureMeshFileChunk {
                file_id: manifest.file_id.clone(),
                chunk_index: index,
                bytes: vec![index as u8; manifest.chunk_size as usize],
            };
            seal_file_chunk(
                key,
                &context_fixture(
                    &format!("chunk_{index}"),
                    &format!("msg_chunk_{index}"),
                    manifest,
                ),
                &chunk,
            )
            .unwrap()
        })
        .collect()
}

pub(super) fn context_fixture(
    envelope: &str,
    message: &str,
    manifest: &SecureMeshFileManifest,
) -> SecureMeshFileProtectionContext {
    pairwise_context_fixture(
        envelope,
        message,
        manifest,
        "desktop_gui:alpha",
        "mobile:beta",
        "file_session_test",
        &hash_bytes(format!("fixture-file-hash:{}", manifest.file_id).as_bytes()),
        1_800_000_000,
    )
}

#[allow(clippy::too_many_arguments)]
pub(super) fn pairwise_context_fixture(
    envelope: &str,
    message: &str,
    manifest: &SecureMeshFileManifest,
    sender_endpoint_id: &str,
    recipient_endpoint_id: &str,
    session_id: &str,
    file_hash: &str,
    expires_at_unix_seconds: u64,
) -> SecureMeshFileProtectionContext {
    SecureMeshFileProtectionContext::for_pairwise_device(
        SecureMeshContentContext::new(
            format!("env_{envelope}"),
            message,
            "mailbox_file",
            sender_endpoint_id,
            recipient_endpoint_id,
            session_id,
            "2026-01-01T00:00:00.000Z",
            "2026-01-01T00:10:00.000Z",
        ),
        manifest.file_id.clone(),
        manifest.chunk_count,
        file_hash,
        expires_at_unix_seconds,
    )
    .unwrap()
}

#[allow(clippy::too_many_arguments)]
pub(super) fn mls_context_fixture(
    envelope: &str,
    message: &str,
    manifest: &SecureMeshFileManifest,
    sender_endpoint_id: &str,
    recipient_endpoint_id: &str,
    session_id: &str,
    file_hash: &str,
    group_id: &str,
    epoch: u64,
    expires_at_unix_seconds: u64,
) -> SecureMeshFileProtectionContext {
    SecureMeshFileProtectionContext::for_mls_epoch(
        SecureMeshContentContext::new(
            format!("env_{envelope}"),
            message,
            "mailbox_file",
            sender_endpoint_id,
            recipient_endpoint_id,
            session_id,
            "2026-01-01T00:00:00.000Z",
            "2026-01-01T00:10:00.000Z",
        ),
        manifest.file_id.clone(),
        manifest.chunk_count,
        file_hash,
        group_id,
        epoch,
        expires_at_unix_seconds,
    )
    .unwrap()
}

pub(super) fn key_fixture() -> FileRootKey {
    FileRootKey::from_bytes([23; 32])
}

pub(super) struct PairwiseFileEndpoint {
    identity: DeviceTrustPublicIdentity,
    identity_secret: SecureMeshPairwisePrivateKey,
    signing_key: SigningKey,
}

pub(super) struct PairwiseFilePrekeys {
    signed_secret: SecureMeshPairwisePrivateKey,
    one_time_secret: SecureMeshPairwisePrivateKey,
    one_time_mlkem1024_prekey_seed: SecureMeshMlKem1024PreKeySeed,
    bundle: SecureMeshPairwisePreKeyBundle,
}

pub(super) fn pairwise_file_sessions() -> (SecureMeshPairwiseSession, SecureMeshPairwiseSession) {
    let alice = pairwise_file_endpoint("desktop_gui:file-wrap-alice");
    let bob = pairwise_file_endpoint("mobile:file-wrap-bob");
    let bob_prekeys = pairwise_file_prekeys(&bob);
    let bob_directory = authorize_test_pairwise_prekey_bundle(&bob_prekeys.bundle);
    let now = OffsetDateTime::parse("2026-06-26T00:00:01Z", &Rfc3339).unwrap();
    let (mut alice_session, intro) = SecureMeshPairwiseSession::initiate(
        &alice.identity,
        &alice.identity_secret,
        &alice.signing_key,
        &bob_prekeys.bundle,
        &bob_directory,
        &SecureMeshPreKeyValidationPolicy::default(),
        &crate::core::secure_mesh_pairwise::secure_mesh_pairwise_test_capability_evaluation()
            .unwrap(),
        now,
    )
    .unwrap();
    let (mut bob_session, accepted) = SecureMeshPairwiseSession::accept(
        &bob.identity,
        &bob.identity_secret,
        &bob.signing_key,
        &alice.identity,
        &bob_prekeys.signed_secret,
        Some(&bob_prekeys.one_time_secret),
        &bob_prekeys.one_time_mlkem1024_prekey_seed,
        &intro,
        &crate::core::secure_mesh_pairwise::secure_mesh_pairwise_test_capability_evaluation()
            .unwrap(),
        now,
        &mut crate::core::secure_mesh_session_negotiation::CapabilityProofReplayGuard::default(),
    )
    .unwrap();
    let finished = alice_session
        .complete_initiator_handshake(
            &alice.identity,
            &bob.identity,
            &accepted,
            now,
            &mut crate::core::secure_mesh_session_negotiation::CapabilityProofReplayGuard::default(
            ),
        )
        .unwrap();
    bob_session.complete_responder_handshake(&finished).unwrap();
    (alice_session, bob_session)
}

pub(super) fn pairwise_file_endpoint(endpoint_id: &str) -> PairwiseFileEndpoint {
    let identity_secret = SecureMeshPairwisePrivateKey::generate();
    let signing_key = SigningKey::generate(&mut OsRng);
    let identity = DeviceTrustPublicIdentity::new(
        endpoint_id,
        identity_secret.public_key(),
        signing_key.verifying_key().to_bytes(),
        1,
    )
    .unwrap();
    PairwiseFileEndpoint {
        identity,
        identity_secret,
        signing_key,
    }
}

pub(super) fn pairwise_file_prekeys(endpoint: &PairwiseFileEndpoint) -> PairwiseFilePrekeys {
    let signed_secret = SecureMeshPairwisePrivateKey::generate();
    let one_time_secret = SecureMeshPairwisePrivateKey::generate();
    let one_time_mlkem1024_prekey_seed = SecureMeshMlKem1024PreKeySeed::generate();
    let signed_prekey = sign_prekey_record(
        &endpoint.signing_key,
        &endpoint.identity,
        SecureMeshPreKeyKind::SignedPreKey,
        "file-wrap-spk-1",
        signed_secret.public_key(),
        "2026-06-26T00:00:00Z",
        "2026-07-26T00:00:00Z",
    )
    .unwrap();
    let one_time_prekey = sign_prekey_record(
        &endpoint.signing_key,
        &endpoint.identity,
        SecureMeshPreKeyKind::OneTimePreKey,
        "file-wrap-otpk-1",
        one_time_secret.public_key(),
        "2026-06-26T00:00:00Z",
        "2026-07-26T00:00:00Z",
    )
    .unwrap();
    let one_time_mlkem1024_prekey = sign_prekey_record(
        &endpoint.signing_key,
        &endpoint.identity,
        SecureMeshPreKeyKind::OneTimeMlKem1024PreKey,
        "file-wrap-pqotpk-1",
        one_time_mlkem1024_prekey_seed.public_key(),
        "2026-06-26T00:00:00Z",
        "2026-07-26T00:00:00Z",
    )
    .unwrap();
    PairwiseFilePrekeys {
        signed_secret,
        one_time_secret,
        one_time_mlkem1024_prekey_seed,
        bundle: SecureMeshPairwisePreKeyBundle {
            endpoint_identity: endpoint.identity.clone(),
            trust_state: DeviceTrustState::Verified,
            signed_prekey,
            one_time_prekey: Some(one_time_prekey),
            one_time_mlkem1024_prekey,
            prekey_publication_version: 1,
        },
    }
}

pub(super) fn pairwise_file_context(
    session: &SecureMeshPairwiseSession,
    envelope_id: &str,
    message_id: &str,
) -> SecureMeshContentContext {
    let created_at = OffsetDateTime::now_utc();
    let expires_at = created_at + Duration::minutes(10);
    SecureMeshContentContext::new(
        general_purpose::URL_SAFE_NO_PAD.encode(&Sha256::digest(envelope_id.as_bytes())[..24]),
        message_id,
        general_purpose::URL_SAFE_NO_PAD.encode(Sha256::digest(b"mailbox_file_key_wrap")),
        session.local_endpoint_id.clone(),
        session.remote_endpoint_id.clone(),
        session.session_id.clone(),
        created_at.format(&Rfc3339).unwrap(),
        expires_at.format(&Rfc3339).unwrap(),
    )
}

pub(super) fn pairwise_file_protection_context(
    session: &SecureMeshPairwiseSession,
    envelope_id: &str,
    message_id: &str,
    manifest: &SecureMeshFileManifest,
    file_hash: &str,
) -> SecureMeshFileProtectionContext {
    SecureMeshFileProtectionContext::for_pairwise_device(
        pairwise_file_context(session, envelope_id, message_id),
        manifest.file_id.clone(),
        manifest.chunk_count,
        file_hash,
        1_800_000_000,
    )
    .unwrap()
}

pub(super) struct EncryptedPairwiseFileFixture {
    root_key_bytes: [u8; FILE_ROOT_KEY_BYTES],
    wrap_secret_bytes: [u8; FILE_KEY_WRAP_SECRET_BYTES],
    pub(super) chunk: SecureMeshFileChunk,
    pub(super) encrypted_chunk: EncryptedSecureMeshFileChunk,
    pub(super) chunk_context: SecureMeshFileProtectionContext,
    key_context: SecureMeshFileProtectionContext,
}

pub(super) fn encrypted_pairwise_file_fixture(
    session: &SecureMeshPairwiseSession,
    label: &str,
    key_bytes: [u8; 32],
    bytes: &[u8],
) -> EncryptedPairwiseFileFixture {
    let key = FileRootKey::from_bytes(key_bytes);
    let manifest = SecureMeshFileManifest {
        file_id: format!("file-key-wrap-out-of-order-{label}"),
        file_name: format!("pairwise-wrapped-{label}.txt"),
        mime_type: "text/plain".to_string(),
        relative_path: format!("pairwise/wrapped/{label}"),
        total_size: bytes.len() as u64,
        chunk_size: bytes.len().try_into().unwrap(),
        chunk_count: 1,
    };
    let file_hash = hash_bytes(bytes);
    let manifest_context = pairwise_file_protection_context(
        session,
        &format!("env_file_manifest_out_of_order_{label}"),
        &format!("msg_file_manifest_out_of_order_{label}"),
        &manifest,
        &file_hash,
    );
    let encrypted_manifest = seal_file_manifest(&key, &manifest_context, &manifest).unwrap();
    assert_eq!(
        open_file_manifest(&key, &manifest_context, &encrypted_manifest).unwrap(),
        manifest
    );
    let chunk = SecureMeshFileChunk {
        file_id: manifest.file_id.clone(),
        chunk_index: 0,
        bytes: bytes.to_vec(),
    };
    let chunk_context = pairwise_file_protection_context(
        session,
        &format!("env_file_chunk_out_of_order_{label}"),
        &format!("msg_file_chunk_out_of_order_{label}"),
        &manifest,
        &file_hash,
    );
    let encrypted_chunk = seal_file_chunk(&key, &chunk_context, &chunk).unwrap();
    let key_context = pairwise_file_protection_context(
        session,
        &format!("env_file_key_out_of_order_{label}"),
        &format!("msg_file_key_out_of_order_{label}"),
        &manifest,
        &file_hash,
    );
    EncryptedPairwiseFileFixture {
        root_key_bytes: key_bytes,
        wrap_secret_bytes: [key_bytes[0].wrapping_add(64); FILE_KEY_WRAP_SECRET_BYTES],
        chunk,
        encrypted_chunk,
        chunk_context,
        key_context,
    }
}

pub(super) fn pairwise_file_key_envelope(
    session: &mut SecureMeshPairwiseSession,
    fixture: &EncryptedPairwiseFileFixture,
) -> licoup_protocol_bindings::licoarc_relay::LicoArcRelayEnvelope {
    let root_key = FileRootKey::from_bytes(fixture.root_key_bytes);
    let wrap_secret = FileKeyWrapSecret::from_bytes(fixture.wrap_secret_bytes);
    let file_key_envelope =
        seal_file_root_key_for_pairwise_device(&root_key, &wrap_secret, &fixture.key_context)
            .unwrap();
    let body = file_key_envelope.to_json().unwrap().into_bytes();
    session
        .seal_payload_envelope(
            fixture.key_context.content_context(),
            &SecureMeshPlaintext::new(SecureMeshPayloadKind::Command, body)
                .with_content_type(SECURE_MESH_FILE_KEY_ENVELOPE_CONTENT_TYPE),
        )
        .unwrap()
}

pub(super) fn recovered_file_root_key(
    body: &[u8],
    fixture: &EncryptedPairwiseFileFixture,
) -> FileRootKey {
    let envelope = FileKeyEnvelope::from_json(std::str::from_utf8(body).unwrap()).unwrap();
    open_file_root_key_for_pairwise_device(
        &envelope,
        &FileKeyWrapSecret::from_bytes(fixture.wrap_secret_bytes),
        &fixture.key_context,
        1_700_000_000,
    )
    .unwrap()
}
