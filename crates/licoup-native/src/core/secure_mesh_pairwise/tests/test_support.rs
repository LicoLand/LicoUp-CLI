pub(super) use super::super::codec::*;
pub(super) use super::super::key_ratchet::*;
pub(super) use super::super::manager_fanout::*;
pub(super) use super::super::persistence::*;
pub(super) use super::super::runtime_self_test::*;
pub(super) use super::super::session_negotiation::*;
pub(super) use super::super::support::*;
pub(super) use crate::core::secure_mesh::{
    SECURE_MESH_PROTOCOL_BUILD_REVISION, SECURE_MESH_PROTOCOL_VERSION,
};
pub(super) use crate::core::secure_mesh_capability_proof::{
    CapabilityProofRequest, CapabilityProofVerificationContext, SignedCapabilityProof,
    sign_capability_proof,
};
pub(super) use crate::core::secure_mesh_command::{
    SecureCommandEvaluationContext, SecureCommandLocalExecutor, SecureCommandPayload,
    SecureCommandReplayLedger, evaluate_secure_command, execute_evaluated_secure_command,
};
pub(super) use crate::core::secure_mesh_crypto::{
    SecureMeshContentContext, SecureMeshPayloadKind, SecureMeshPlaintext,
};
pub(super) use crate::core::secure_mesh_pqxdh::{
    ML_KEM_1024_CIPHERTEXT_BYTES, ML_KEM_1024_KEY_GENERATION_SEED_BYTES,
    SecureMeshMlKem1024PreKeySeed, decapsulate_ml_kem_1024, derive_triple_ratchet_initial_secrets,
    encapsulate_ml_kem_1024,
};
pub(super) use crate::core::secure_mesh_prekey::{
    SecureMeshPairwisePreKeyBundle, SecureMeshPreKeyKind, SecureMeshPreKeyValidationPolicy,
    authorize_test_pairwise_prekey_bundle, sign_prekey_record,
};
pub(super) use crate::core::secure_mesh_secret_store::SecretBytes;
pub(super) use crate::core::secure_mesh_secret_store::{
    SecretStoreAuthorizationRequest, SecretStoreAuthorizationSession, SecretStoreHandle,
    SecureMeshSecretStore,
};
pub(super) use crate::core::secure_mesh_session_negotiation::{
    CapabilityProofPeer, CapabilityProofReplayGuard, VerifiedSessionNegotiation,
    accept_pairwise_capability_binding, create_pairwise_capability_binding,
};
pub(super) use crate::core::secure_mesh_sparse_pq_ratchet::SecureMeshSparsePqRatchet;
pub(super) use crate::core::secure_mesh_trust::{DeviceTrustPublicIdentity, DeviceTrustState};
pub(super) use crate::platform::secure_mesh_secret_store::EphemeralSecretStore;
pub(super) use anyhow::{Result, anyhow, ensure};
pub(super) use base64::{Engine as _, engine::general_purpose};
pub(super) use ed25519_dalek::SigningKey;
pub(super) use licoup_protocol_bindings::licoarc_relay::LicoArcRelayEnvelope;
pub(super) use rand_core::OsRng;
pub(super) use rusqlite::{Connection as TestConnection, params};
pub(super) use serde_json::{Value, json};
pub(super) use sha2::{Digest, Sha256};
pub(super) use std::path::{Path, PathBuf};
pub(super) use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
pub(super) use std::time::{SystemTime, UNIX_EPOCH};
pub(super) use time::{Duration, OffsetDateTime, format_description::well_known::Rfc3339};
pub(super) use zeroize::Zeroizing;

pub(super) struct EndpointFixture {
    pub(super) identity: DeviceTrustPublicIdentity,
    pub(super) identity_secret: SecureMeshPairwisePrivateKey,
    pub(super) signing_key: SigningKey,
}

pub(super) struct PrekeyFixture {
    pub(super) signed_secret: SecureMeshPairwisePrivateKey,
    pub(super) one_time_secret: SecureMeshPairwisePrivateKey,
    pub(super) one_time_mlkem1024_seed: SecureMeshMlKem1024PreKeySeed,
    pub(super) bundle: SecureMeshPairwisePreKeyBundle,
}

pub(super) struct HandshakeFixture {
    pub(super) alice: EndpointFixture,
    pub(super) bob: EndpointFixture,
    pub(super) bob_prekeys: PrekeyFixture,
    pub(super) alice_session: SecureMeshPairwiseSession,
    pub(super) intro: SecureMeshPairwiseSessionIntro,
    pub(super) bob_session: SecureMeshPairwiseSession,
    pub(super) accepted: SecureMeshPairwiseSessionAccepted,
}

pub(super) fn endpoint(endpoint_id: &str) -> EndpointFixture {
    let identity_secret = SecureMeshPairwisePrivateKey::generate();
    let signing_key = SigningKey::generate(&mut OsRng);
    let identity = DeviceTrustPublicIdentity::new(
        endpoint_id,
        identity_secret.public_key(),
        signing_key.verifying_key().to_bytes(),
        1,
    )
    .unwrap();
    EndpointFixture {
        identity,
        identity_secret,
        signing_key,
    }
}

pub(super) fn prekeys(endpoint: &EndpointFixture) -> PrekeyFixture {
    let signed_secret = SecureMeshPairwisePrivateKey::generate();
    let one_time_secret = SecureMeshPairwisePrivateKey::generate();
    let one_time_mlkem1024_seed = SecureMeshMlKem1024PreKeySeed::generate();
    let signed_prekey = sign_prekey_record(
        &endpoint.signing_key,
        &endpoint.identity,
        SecureMeshPreKeyKind::SignedPreKey,
        "spk-1",
        signed_secret.public_key(),
        "2026-06-26T00:00:00Z",
        "2026-07-26T00:00:00Z",
    )
    .unwrap();
    let one_time_prekey = sign_prekey_record(
        &endpoint.signing_key,
        &endpoint.identity,
        SecureMeshPreKeyKind::OneTimePreKey,
        "otpk-1",
        one_time_secret.public_key(),
        "2026-06-26T00:00:00Z",
        "2026-07-26T00:00:00Z",
    )
    .unwrap();
    let one_time_mlkem1024_prekey = sign_prekey_record(
        &endpoint.signing_key,
        &endpoint.identity,
        SecureMeshPreKeyKind::OneTimeMlKem1024PreKey,
        "pqotpk-1",
        one_time_mlkem1024_seed.public_key(),
        "2026-06-26T00:00:00Z",
        "2026-07-26T00:00:00Z",
    )
    .unwrap();
    PrekeyFixture {
        signed_secret,
        one_time_secret,
        one_time_mlkem1024_seed,
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

pub(super) fn pairwise_sessions() -> (SecureMeshPairwiseSession, SecureMeshPairwiseSession) {
    pairwise_sessions_between("desktop_gui:alice", "mobile:bob")
}

pub(super) fn handshake_now() -> OffsetDateTime {
    OffsetDateTime::parse(
        "2026-06-26T00:00:01Z",
        &time::format_description::well_known::Rfc3339,
    )
    .unwrap()
}

pub(super) fn handshake_fixture() -> HandshakeFixture {
    let alice = endpoint("desktop_gui:alice");
    let bob = endpoint("mobile:bob");
    let bob_prekeys = prekeys(&bob);
    let bob_directory = authorize_test_pairwise_prekey_bundle(&bob_prekeys.bundle);
    let now = handshake_now();
    let (alice_session, intro) = SecureMeshPairwiseSession::initiate(
        &alice.identity,
        &alice.identity_secret,
        &alice.signing_key,
        &bob_prekeys.bundle,
        &bob_directory,
        &SecureMeshPreKeyValidationPolicy::default(),
        &secure_mesh_pairwise_test_capability_evaluation().unwrap(),
        now,
    )
    .unwrap();
    let (bob_session, accepted) = SecureMeshPairwiseSession::accept(
        &bob.identity,
        &bob.identity_secret,
        &bob.signing_key,
        &alice.identity,
        &bob_prekeys.signed_secret,
        Some(&bob_prekeys.one_time_secret),
        &bob_prekeys.one_time_mlkem1024_seed,
        &intro,
        &secure_mesh_pairwise_test_capability_evaluation().unwrap(),
        now,
        &mut CapabilityProofReplayGuard::default(),
    )
    .unwrap();
    HandshakeFixture {
        alice,
        bob,
        bob_prekeys,
        alice_session,
        intro,
        bob_session,
        accepted,
    }
}

pub(super) fn pairwise_sessions_between(
    initiator_endpoint_id: &str,
    responder_endpoint_id: &str,
) -> (SecureMeshPairwiseSession, SecureMeshPairwiseSession) {
    let alice = endpoint(initiator_endpoint_id);
    let bob = endpoint(responder_endpoint_id);
    let bob_prekeys = prekeys(&bob);
    let bob_directory = authorize_test_pairwise_prekey_bundle(&bob_prekeys.bundle);
    let now = handshake_now();
    let (mut alice_session, intro) = SecureMeshPairwiseSession::initiate(
        &alice.identity,
        &alice.identity_secret,
        &alice.signing_key,
        &bob_prekeys.bundle,
        &bob_directory,
        &SecureMeshPreKeyValidationPolicy::default(),
        &secure_mesh_pairwise_test_capability_evaluation().unwrap(),
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
        &bob_prekeys.one_time_mlkem1024_seed,
        &intro,
        &secure_mesh_pairwise_test_capability_evaluation().unwrap(),
        now,
        &mut CapabilityProofReplayGuard::default(),
    )
    .unwrap();
    let finished = alice_session
        .complete_initiator_handshake(
            &alice.identity,
            &bob.identity,
            &accepted,
            now,
            &mut CapabilityProofReplayGuard::default(),
        )
        .unwrap();
    bob_session.complete_responder_handshake(&finished).unwrap();
    assert_eq!(alice_session.session_id, bob_session.session_id);
    (alice_session, bob_session)
}

pub(super) fn fixed_pairwise_key(seed: u8) -> SecureMeshPairwisePrivateKey {
    SecureMeshPairwisePrivateKey::from_bytes([seed; PUBLIC_KEY_LEN])
}

pub(super) fn deterministic_test_capability_negotiation(
    local_endpoint_id: &str,
    remote_endpoint_id: &str,
    handshake_transcript_hash: &[u8; HANDSHAKE_HASH_LEN],
) -> (SignedCapabilityProof, VerifiedSessionNegotiation) {
    fn deterministic_identity(
        endpoint_id: &str,
        label: &[u8],
    ) -> (DeviceTrustPublicIdentity, SigningKey) {
        let mut signing_material = Vec::from(label);
        signing_material.extend_from_slice(endpoint_id.as_bytes());
        let signing_seed: [u8; 32] = Sha256::digest(&signing_material).into();
        let signing_key = SigningKey::from_bytes(&signing_seed);
        let mut identity_material = Vec::from(b"pairwise-test-identity".as_slice());
        identity_material.extend_from_slice(endpoint_id.as_bytes());
        let identity_public_key: [u8; 32] = Sha256::digest(identity_material).into();
        (
            DeviceTrustPublicIdentity::new(
                endpoint_id,
                identity_public_key,
                signing_key.verifying_key().to_bytes(),
                1,
            )
            .unwrap(),
            signing_key,
        )
    }

    let (local_identity, local_signing_key) =
        deterministic_identity(local_endpoint_id, b"pairwise-test-local-signing");
    let (remote_identity, remote_signing_key) =
        deterministic_identity(remote_endpoint_id, b"pairwise-test-remote-signing");
    let evaluation = secure_mesh_pairwise_test_capability_evaluation().unwrap();
    let now = handshake_now();
    let challenge = [0x42; 32];
    let request = capability_proof_request(challenge, now).unwrap();
    let local_proof =
        sign_capability_proof(&local_identity, &local_signing_key, &evaluation, &request).unwrap();
    let remote_proof =
        sign_capability_proof(&remote_identity, &remote_signing_key, &evaluation, &request)
            .unwrap();
    let context = capability_verification_context(challenge, now).unwrap();
    let local_verified = crate::core::secure_mesh_capability_proof::verify_capability_proof(
        &local_identity,
        &local_proof,
        &context,
    )
    .unwrap();
    let remote_verified = crate::core::secure_mesh_capability_proof::verify_capability_proof(
        &remote_identity,
        &remote_proof,
        &context,
    )
    .unwrap();
    let base_transcript_digest =
        crate::core::secure_mesh_capability_proof::encode_sha256_digest(handshake_transcript_hash);
    let binding = create_pairwise_capability_binding(
        &local_verified,
        &remote_verified,
        &base_transcript_digest,
    )
    .unwrap();
    let mut replay_guard = CapabilityProofReplayGuard::default();
    let negotiation = accept_pairwise_capability_binding(
        CapabilityProofPeer {
            identity: &local_identity,
            proof: &local_proof,
            verification_context: &context,
        },
        CapabilityProofPeer {
            identity: &remote_identity,
            proof: &remote_proof,
            verification_context: &context,
        },
        &base_transcript_digest,
        &binding,
        &mut replay_guard,
    )
    .unwrap();
    (local_proof, negotiation)
}

pub(super) fn deterministic_pairwise_session(
    session_id: &str,
    local_endpoint_id: &str,
    remote_endpoint_id: &str,
    initiator_endpoint_id: &str,
    responder_endpoint_id: &str,
    shared_secret: [u8; PUBLIC_KEY_LEN],
    local_ratchet_secret: SecureMeshPairwisePrivateKey,
    remote_ratchet_public_key: [u8; PUBLIC_KEY_LEN],
) -> Result<SecureMeshPairwiseSession> {
    validate_endpoint_id(local_endpoint_id)?;
    validate_endpoint_id(remote_endpoint_id)?;
    ensure!(
        local_endpoint_id != remote_endpoint_id,
        "secure mesh pairwise deterministic vector requires distinct endpoints"
    );
    ensure!(
        (local_endpoint_id == initiator_endpoint_id && remote_endpoint_id == responder_endpoint_id)
            || (local_endpoint_id == responder_endpoint_id
                && remote_endpoint_id == initiator_endpoint_id),
        "secure mesh pairwise deterministic vector endpoint order mismatch"
    );
    let role = if local_endpoint_id == initiator_endpoint_id {
        SecureMeshPairwiseRole::Initiator
    } else {
        SecureMeshPairwiseRole::Responder
    };
    let keys = derive_initial_keys(
        &shared_secret,
        session_id,
        initiator_endpoint_id,
        responder_endpoint_id,
    )?;
    let local_ratchet_public_key = local_ratchet_secret.public_key();
    let (sending_chain_key, receiving_chain_key) = if role == SecureMeshPairwiseRole::Initiator {
        (keys.initiator_chain_key, keys.responder_chain_key)
    } else {
        (keys.responder_chain_key, keys.initiator_chain_key)
    };
    let (
        sending_header_key,
        receiving_header_key,
        next_sending_header_key,
        next_receiving_header_key,
    ) = if role == SecureMeshPairwiseRole::Initiator {
        (
            keys.initiator_header_key,
            keys.responder_header_key,
            keys.initiator_next_header_key,
            keys.responder_next_header_key,
        )
    } else {
        (
            keys.responder_header_key,
            keys.initiator_header_key,
            keys.responder_next_header_key,
            keys.initiator_next_header_key,
        )
    };
    let handshake_transcript_hash: [u8; HANDSHAKE_HASH_LEN] =
        Sha256::digest(session_id.as_bytes()).into();
    let (local_capability_proof, capability_negotiation) =
        deterministic_test_capability_negotiation(
            local_endpoint_id,
            remote_endpoint_id,
            &handshake_transcript_hash,
        );
    let sparse_pq_seed: [u8; 32] = Sha256::digest(
        [
            b"licomesh.secure-mesh.test.sparse-pq.v1".as_slice(),
            shared_secret.as_slice(),
        ]
        .concat(),
    )
    .into();
    let sparse_pq_ratchet = match role {
        SecureMeshPairwiseRole::Initiator => {
            SecureMeshSparsePqRatchet::new_initiator(&sparse_pq_seed)?
        }
        SecureMeshPairwiseRole::Responder => {
            SecureMeshSparsePqRatchet::new_responder(&sparse_pq_seed)?
        }
    };
    Ok(SecureMeshPairwiseSession {
        session_id: session_id.to_string(),
        local_endpoint_id: local_endpoint_id.to_string(),
        remote_endpoint_id: remote_endpoint_id.to_string(),
        role,
        root_key: Zeroizing::new(keys.root_key),
        sending_chain_key: Zeroizing::new(sending_chain_key),
        receiving_chain_key: Zeroizing::new(receiving_chain_key),
        sending_header_key: Zeroizing::new(sending_header_key),
        receiving_header_key: Zeroizing::new(receiving_header_key),
        next_sending_header_key: Zeroizing::new(next_sending_header_key),
        next_receiving_header_key: Zeroizing::new(next_receiving_header_key),
        skipped_receiving_header_keys: Vec::new(),
        local_ratchet_secret,
        local_ratchet_public_key,
        remote_ratchet_public_key,
        handshake_transcript_hash,
        dh_epoch: 0,
        receiving_ratchet_epoch: 0,
        sending_chain_index: 0,
        receiving_chain_index: 0,
        previous_chain_length: 0,
        skipped_keys: Vec::new(),
        received_message_ids: Vec::new(),
        pending_sending_ratchet: false,
        initiator_key_confirmed: true,
        local_capability_proof,
        capability_negotiation: Some(capability_negotiation),
        sparse_pq_ratchet,
        revoked: false,
    })
}

pub(super) fn fixed_signing_key(seed: u8) -> SigningKey {
    SigningKey::from_bytes(&[seed; 32])
}

pub(super) fn fixed_endpoint(
    endpoint_id: &str,
    identity_seed: u8,
    signing_seed: u8,
) -> EndpointFixture {
    let identity_secret = fixed_pairwise_key(identity_seed);
    let signing_key = fixed_signing_key(signing_seed);
    let identity = DeviceTrustPublicIdentity::new(
        endpoint_id,
        identity_secret.public_key(),
        signing_key.verifying_key().to_bytes(),
        1,
    )
    .unwrap();
    EndpointFixture {
        identity,
        identity_secret,
        signing_key,
    }
}

pub(super) fn fixed_prekeys(
    endpoint: &EndpointFixture,
    signed_seed: u8,
    one_time_seed: u8,
) -> PrekeyFixture {
    let signed_secret = fixed_pairwise_key(signed_seed);
    let one_time_secret = fixed_pairwise_key(one_time_seed);
    let one_time_mlkem1024_seed = SecureMeshMlKem1024PreKeySeed::from_bytes(
        [signed_seed.wrapping_add(one_time_seed); ML_KEM_1024_KEY_GENERATION_SEED_BYTES],
    );
    let signed_prekey = sign_prekey_record(
        &endpoint.signing_key,
        &endpoint.identity,
        SecureMeshPreKeyKind::SignedPreKey,
        "spk-vector",
        signed_secret.public_key(),
        "2026-06-26T00:00:00Z",
        "2026-07-26T00:00:00Z",
    )
    .unwrap();
    let one_time_prekey = sign_prekey_record(
        &endpoint.signing_key,
        &endpoint.identity,
        SecureMeshPreKeyKind::OneTimePreKey,
        "otpk-vector",
        one_time_secret.public_key(),
        "2026-06-26T00:00:00Z",
        "2026-07-26T00:00:00Z",
    )
    .unwrap();
    let one_time_mlkem1024_prekey = sign_prekey_record(
        &endpoint.signing_key,
        &endpoint.identity,
        SecureMeshPreKeyKind::OneTimeMlKem1024PreKey,
        "pqotpk-vector",
        one_time_mlkem1024_seed.public_key(),
        "2026-06-26T00:00:00Z",
        "2026-07-26T00:00:00Z",
    )
    .unwrap();
    PrekeyFixture {
        signed_secret,
        one_time_secret,
        one_time_mlkem1024_seed,
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

pub(super) fn durable_store_path(test_name: &str) -> PathBuf {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let mut path = std::env::temp_dir();
    path.push(format!(
        "lico-secure-mesh-pairwise-{test_name}-{}-{nonce}.sqlite3",
        std::process::id()
    ));
    path
}

pub(super) fn test_secret_store() -> Arc<dyn SecureMeshSecretStore> {
    Arc::new(EphemeralSecretStore::new())
}

pub(super) struct FailOnceDeleteSecretStore {
    pub(super) inner: EphemeralSecretStore,
    pub(super) fail_next_delete: AtomicBool,
}

impl FailOnceDeleteSecretStore {
    pub(super) fn new() -> Self {
        Self {
            inner: EphemeralSecretStore::new(),
            fail_next_delete: AtomicBool::new(true),
        }
    }
}

impl SecureMeshSecretStore for FailOnceDeleteSecretStore {
    fn backend(&self) -> &'static str {
        self.inner.backend()
    }

    fn supported(&self) -> bool {
        self.inner.supported()
    }

    fn capability_facts(&self) -> Result<Vec<crate::core::secure_mesh_capability::CapabilityFact>> {
        self.inner.capability_facts()
    }

    fn begin_authorized_session(
        &self,
        request: &SecretStoreAuthorizationRequest,
    ) -> Result<SecretStoreAuthorizationSession> {
        self.inner.begin_authorized_session(request)
    }

    fn set_secret(&self, handle: &SecretStoreHandle, secret: SecretBytes) -> Result<()> {
        self.inner.set_secret(handle, secret)
    }

    fn get_secret(&self, handle: &SecretStoreHandle) -> Result<Option<SecretBytes>> {
        self.inner.get_secret(handle)
    }

    fn delete_secret(&self, handle: &SecretStoreHandle) -> Result<()> {
        if self.fail_next_delete.swap(false, Ordering::SeqCst) {
            return Err(anyhow!("injected secret deletion failure"));
        }
        self.inner.delete_secret(handle)
    }
}

pub(super) fn durable_store_namespace(test_name: &str) -> String {
    format!("pairwise-test-{test_name}")
}

pub(super) fn open_test_durable_store(
    store_path: &Path,
    secret_store: Arc<dyn SecureMeshSecretStore>,
    test_name: &str,
) -> SecureMeshPairwiseDurableStore {
    SecureMeshPairwiseDurableStore::open_with_secret_store(
        store_path,
        secret_store,
        durable_store_namespace(test_name),
    )
    .unwrap()
}

pub(super) fn stored_snapshot_json(
    store_path: &Path,
    session_id: &str,
    local_endpoint_id: &str,
) -> String {
    TestConnection::open(store_path)
        .unwrap()
        .query_row(
            r#"
                SELECT snapshot_json
                FROM secure_mesh_pairwise_sessions
                WHERE session_id = ?1 AND local_endpoint_id = ?2
                "#,
            params![session_id, local_endpoint_id],
            |row| row.get(0),
        )
        .unwrap()
}

pub(super) fn payload_context(
    session: &SecureMeshPairwiseSession,
    message_id: &str,
    sender: &str,
    recipient: &str,
) -> SecureMeshContentContext {
    payload_context_with_mailbox(
        session,
        message_id,
        "mailbox-pairwise-payload",
        sender,
        recipient,
    )
}

pub(super) fn payload_context_with_mailbox(
    session: &SecureMeshPairwiseSession,
    message_id: &str,
    mailbox_id: &str,
    sender: &str,
    recipient: &str,
) -> SecureMeshContentContext {
    let created_at = OffsetDateTime::now_utc();
    let expires_at = created_at + Duration::minutes(10);
    SecureMeshContentContext::new(
        relay_delivery_id(message_id),
        message_id,
        relay_mailbox_token(mailbox_id),
        sender,
        recipient,
        session.session_id.clone(),
        created_at.format(&Rfc3339).unwrap(),
        expires_at.format(&Rfc3339).unwrap(),
    )
}

pub(super) fn relay_delivery_id(label: &str) -> String {
    general_purpose::URL_SAFE_NO_PAD.encode(&Sha256::digest(label.as_bytes())[..24])
}

pub(super) fn relay_mailbox_token(label: &str) -> String {
    general_purpose::URL_SAFE_NO_PAD.encode(Sha256::digest(label.as_bytes()))
}

#[derive(Default)]
pub(super) struct OpaquePairwiseRelay {
    pub(super) pending: Vec<LicoArcRelayEnvelope>,
    pub(super) acked_delivery_ids: Vec<String>,
}

impl OpaquePairwiseRelay {
    pub(super) fn send(&mut self, envelope: LicoArcRelayEnvelope, forbidden_plaintext: &str) {
        assert_eq!(
            envelope.contract_version(),
            licoup_protocol_bindings::licoarc_relay::LICOARC_RELAY_CONTRACT_VERSION
        );
        assert!(!envelope.envelope_id().contains(forbidden_plaintext));
        assert!(!envelope.mailbox_id().contains(forbidden_plaintext));
        assert!(!envelope.ciphertext().contains(forbidden_plaintext));
        self.pending.push(envelope);
    }

    pub(super) fn sync(&self, mailbox_token: &str) -> Vec<LicoArcRelayEnvelope> {
        let mailbox_token = if mailbox_token.len() == 43 {
            mailbox_token.to_string()
        } else {
            relay_mailbox_token(mailbox_token)
        };
        self.pending
            .iter()
            .filter(|envelope| envelope.mailbox_id() == mailbox_token)
            .cloned()
            .collect()
    }

    pub(super) fn ack(&mut self, message_label: &str) -> bool {
        let delivery_id = relay_delivery_id(message_label);
        let before = self.pending.len();
        self.pending
            .retain(|envelope| envelope.envelope_id() != delivery_id);
        let idempotent = before == self.pending.len();
        if !self.acked_delivery_ids.iter().any(|id| id == &delivery_id) {
            self.acked_delivery_ids.push(delivery_id);
        }
        idempotent
    }

    pub(super) fn queue_len(&self) -> usize {
        self.pending.len()
    }
}

#[derive(Default)]
pub(super) struct PcRelayExecutor {
    pub(super) calls: usize,
}

impl SecureCommandLocalExecutor for PcRelayExecutor {
    fn execute_secure_command(&mut self, payload: &SecureCommandPayload) -> Result<Value> {
        self.calls += 1;
        assert_eq!(payload.command_kind, "agent.message.send");
        Ok(json!({
            "accepted": true,
            "message": payload.body().get("message").and_then(Value::as_str).unwrap_or_default(),
        }))
    }
}

pub(super) fn pc_pc_command_fixture(
    command_id: &str,
    idempotency_key: &str,
    message: &str,
) -> Value {
    json!({
        "schema": crate::core::secure_mesh::SECURE_MESH_COMMAND_PROTOCOL_VERSION,
        "commandId": command_id,
        "commandKind": "agent.message.send",
        "senderIdentity": {
            "endpointId": "desktop_sidecar:pc-a",
            "identityFingerprint": "fingerprint-pc-a",
            "trustState": "verified",
            "endpointKind": "desktop_sidecar"
        },
        "targetBinding": {
            "targetEndpointId": "desktop_sidecar:pc-b",
            "targetAgentId": "agent-pc-b",
            "workspaceId": "workspace-a"
        },
        "riskClass": "safe_write",
        "requiresUserConfirmation": false,
        "idempotencyKey": idempotency_key,
        "createdAt": "2026-06-26T00:00:00Z",
        "expiresAt": "2026-06-26T00:10:00Z",
        "body": {"message": message}
    })
}

pub(super) fn pc_pc_command_context_fixture() -> Value {
    json!({
        "localEndpointId": "desktop_sidecar:pc-b",
        "senderEndpointId": "desktop_sidecar:pc-a",
        "senderIdentityFingerprint": "fingerprint-pc-a",
        "senderTrustState": "verified",
        "senderEndpointKind": "desktop_sidecar",
        "senderRosterActive": true,
        "targetRosterActive": true,
        "sessionOrEpochValid": true,
        "userConfirmed": false,
        "allowedWorkspaceIds": ["workspace-a"],
        "allowedAgentIds": ["agent-pc-b"],
        "now": "2026-06-26T00:01:00Z"
    })
}

pub(super) fn command_fixture_for_endpoints(
    command_id: &str,
    idempotency_key: &str,
    sender_endpoint_id: &str,
    sender_endpoint_kind: &str,
    sender_fingerprint: &str,
    target_endpoint_id: &str,
    target_agent_id: &str,
    workspace_id: &str,
    message: &str,
) -> Value {
    json!({
        "schema": crate::core::secure_mesh::SECURE_MESH_COMMAND_PROTOCOL_VERSION,
        "commandId": command_id,
        "commandKind": "agent.message.send",
        "senderIdentity": {
            "endpointId": sender_endpoint_id,
            "identityFingerprint": sender_fingerprint,
            "trustState": "verified",
            "endpointKind": sender_endpoint_kind
        },
        "targetBinding": {
            "targetEndpointId": target_endpoint_id,
            "targetAgentId": target_agent_id,
            "workspaceId": workspace_id
        },
        "riskClass": "safe_write",
        "requiresUserConfirmation": false,
        "idempotencyKey": idempotency_key,
        "createdAt": "2026-06-26T00:00:00Z",
        "expiresAt": "2026-06-26T00:10:00Z",
        "body": {"message": message}
    })
}

pub(super) fn command_context_for_endpoints(
    local_endpoint_id: &str,
    sender_endpoint_id: &str,
    sender_endpoint_kind: &str,
    sender_fingerprint: &str,
    target_agent_id: &str,
    workspace_id: &str,
) -> Value {
    json!({
        "localEndpointId": local_endpoint_id,
        "senderEndpointId": sender_endpoint_id,
        "senderIdentityFingerprint": sender_fingerprint,
        "senderTrustState": "verified",
        "senderEndpointKind": sender_endpoint_kind,
        "senderRosterActive": true,
        "targetRosterActive": true,
        "sessionOrEpochValid": true,
        "userConfirmed": false,
        "allowedWorkspaceIds": [workspace_id],
        "allowedAgentIds": [target_agent_id],
        "now": "2026-06-26T00:01:00Z"
    })
}

pub(super) fn assert_relay_envelope_hides(
    envelope: &LicoArcRelayEnvelope,
    forbidden_plaintext: &[&str],
) {
    for forbidden in forbidden_plaintext {
        assert!(
            !envelope.envelope_id().contains(forbidden),
            "envelope id leaked {forbidden}"
        );
        assert!(
            !envelope.mailbox_id().contains(forbidden),
            "mailbox id leaked {forbidden}"
        );
        assert!(
            !envelope.ciphertext().contains(forbidden),
            "ciphertext leaked {forbidden}"
        );
    }
}

pub(super) struct CommandRelayScenario<'a> {
    pub(super) label: &'a str,
    pub(super) sender_endpoint_id: &'a str,
    pub(super) sender_endpoint_kind: &'a str,
    pub(super) recipient_endpoint_id: &'a str,
    pub(super) target_agent_id: &'a str,
    pub(super) workspace_id: &'a str,
    pub(super) sender_mailbox_id: &'a str,
    pub(super) recipient_mailbox_id: &'a str,
    pub(super) canary: &'a str,
}

pub(super) fn assert_pairwise_command_result_relay_round_trip(scenario: CommandRelayScenario<'_>) {
    let (mut sender_session, mut recipient_session) =
        pairwise_sessions_between(scenario.sender_endpoint_id, scenario.recipient_endpoint_id);
    let mut relay = OpaquePairwiseRelay::default();
    let sender_fingerprint = format!("fingerprint-{}-sender", scenario.label);
    let command_id = format!("cmd-{}-1", scenario.label);
    let idempotency_key = format!("idem-{}-1", scenario.label);
    let command_message_id = format!("msg-{}-command", scenario.label);
    let result_message_id = format!("msg-{}-result", scenario.label);
    let command = command_fixture_for_endpoints(
        &command_id,
        &idempotency_key,
        scenario.sender_endpoint_id,
        scenario.sender_endpoint_kind,
        &sender_fingerprint,
        scenario.recipient_endpoint_id,
        scenario.target_agent_id,
        scenario.workspace_id,
        scenario.canary,
    );
    let command_context = payload_context_with_mailbox(
        &sender_session,
        &command_message_id,
        scenario.recipient_mailbox_id,
        scenario.sender_endpoint_id,
        scenario.recipient_endpoint_id,
    );
    let command_plaintext = SecureMeshPlaintext::new(
        SecureMeshPayloadKind::Command,
        serde_json::to_vec(&command).unwrap(),
    )
    .with_content_type("application/json");
    let command_envelope = sender_session
        .seal_payload_envelope(&command_context, &command_plaintext)
        .unwrap();
    assert_relay_envelope_hides(
        &command_envelope,
        &[
            scenario.canary,
            scenario.sender_endpoint_id,
            scenario.recipient_endpoint_id,
            scenario.target_agent_id,
            "agent.message.send",
        ],
    );
    relay.send(command_envelope, scenario.canary);
    let synced_for_recipient = relay.sync(scenario.recipient_mailbox_id);
    assert_eq!(synced_for_recipient.len(), 1);
    let opened_command = recipient_session
        .open_payload_envelope(&synced_for_recipient[0], SecureMeshPayloadKind::Command)
        .unwrap();
    let command_value: Value = serde_json::from_slice(&opened_command.body).unwrap();
    assert_eq!(command_value["body"]["message"], scenario.canary);
    assert_eq!(
        command_value["targetBinding"]["targetEndpointId"],
        scenario.recipient_endpoint_id
    );

    let command_payload = SecureCommandPayload::from_value(&command_value).unwrap();
    let command_gate = SecureCommandEvaluationContext::from_value(&command_context_for_endpoints(
        scenario.recipient_endpoint_id,
        scenario.sender_endpoint_id,
        scenario.sender_endpoint_kind,
        &sender_fingerprint,
        scenario.target_agent_id,
        scenario.workspace_id,
    ))
    .unwrap();
    let mut ledger = SecureCommandReplayLedger::default();
    let evaluation = evaluate_secure_command(&command_payload, &command_gate, &mut ledger).unwrap();
    assert!(evaluation.accepted);
    assert!(evaluation.should_execute);
    let mut executor = PcRelayExecutor::default();
    let execution = execute_evaluated_secure_command(
        &command_payload,
        &evaluation,
        &mut executor,
        "2026-06-26T00:02:00Z",
    )
    .unwrap();
    assert_eq!(executor.calls, 1);
    assert!(!relay.ack(&command_message_id));
    assert!(relay.ack(&command_message_id));
    assert_eq!(relay.queue_len(), 0);

    let result = execution.result().unwrap();
    let result_context = payload_context_with_mailbox(
        &recipient_session,
        &result_message_id,
        scenario.sender_mailbox_id,
        scenario.recipient_endpoint_id,
        scenario.sender_endpoint_id,
    );
    let result_plaintext =
        SecureMeshPlaintext::new(SecureMeshPayloadKind::ResultPayload, result.output.clone())
            .with_content_type("application/json");
    let result_envelope = recipient_session
        .seal_payload_envelope(&result_context, &result_plaintext)
        .unwrap();
    assert_relay_envelope_hides(
        &result_envelope,
        &[
            scenario.canary,
            scenario.sender_endpoint_id,
            scenario.recipient_endpoint_id,
            scenario.target_agent_id,
            "agent.message.send",
        ],
    );
    relay.send(result_envelope, scenario.canary);
    let synced_for_sender = relay.sync(scenario.sender_mailbox_id);
    assert_eq!(synced_for_sender.len(), 1);
    let opened_result = sender_session
        .open_payload_envelope(&synced_for_sender[0], SecureMeshPayloadKind::ResultPayload)
        .unwrap();
    let result_value: Value = serde_json::from_slice(&opened_result.body).unwrap();
    assert_eq!(result_value["ok"], true);
    assert_eq!(result_value["commandKind"], "agent.message.send");
    assert_eq!(result_value["output"]["message"], scenario.canary);
    assert!(!relay.ack(&result_message_id));
    assert!(relay.sync(scenario.sender_mailbox_id).is_empty());
    assert_eq!(relay.queue_len(), 0);
}
