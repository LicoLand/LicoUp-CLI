use anyhow::{Result, anyhow, ensure};
use base64::{Engine as _, engine::general_purpose};
use ed25519_dalek::SigningKey;
use rand_core::{OsRng, RngCore};
use time::OffsetDateTime;
use zeroize::Zeroizing;

use super::super::key_ratchet::{
    SecureMeshPairwisePrivateKey, SecureMeshPairwiseRole, SecureMeshPairwiseSession,
};
use super::super::support::{
    HANDSHAKE_HASH_LEN, PUBLIC_KEY_LEN, SECURE_MESH_PAIRWISE_CIPHER_SUITE, parse_key_bytes,
    validate_endpoint_id,
};
use super::capability_binding::{capability_proof_request, capability_verification_context};
use super::input_validation::{ensure_intro, ensure_local_identity_key_material};
use super::key_schedule::{
    derive_capability_bound_initial_keys, derive_initial_keys,
    derive_pqxdh_classical_initiator_secret, derive_pqxdh_classical_responder_secret,
};
use super::transcript_codec::{
    SecureMeshPairwiseSessionAccepted, SecureMeshPairwiseSessionFinished,
    SecureMeshPairwiseSessionIntro, accept_signature_payload, decode_fixed_base64url,
    derive_session_id, handshake_transcript_hash, initiator_finished_key_confirmation,
    intro_signature_payload, pairwise_key_confirmation, sign_pairwise_transcript,
    verify_initiator_finished_key_confirmation, verify_pairwise_key_confirmation,
    verify_pairwise_transcript_signature,
};
use crate::core::secure_mesh::SECURE_MESH_PROTOCOL_VERSION;
use crate::core::secure_mesh_capability::CapabilityEvaluation;
use crate::core::secure_mesh_capability_proof::{
    ClientCapabilityProjection, SignedCapabilityProof, sign_capability_proof,
    signed_capability_proof_challenge,
};
use crate::core::secure_mesh_directory::AuthorizedDirectoryLeaf;
use crate::core::secure_mesh_pqxdh::{
    SecureMeshMlKem1024PreKeySeed, decapsulate_ml_kem_1024, derive_triple_ratchet_initial_secrets,
    encapsulate_ml_kem_1024,
};
use crate::core::secure_mesh_prekey::{
    SecureMeshPairwisePreKeyBundle, SecureMeshPreKeyValidationPolicy,
    validate_pairwise_prekey_bundle,
};
use crate::core::secure_mesh_session_negotiation::{
    CapabilityProofPeer, CapabilityProofReplayGuard, NegotiatedCapabilityBinding,
    VerifiedSessionNegotiation, accept_pairwise_capability_binding,
    create_pairwise_capability_binding,
};
use crate::core::secure_mesh_sparse_pq_ratchet::SecureMeshSparsePqRatchet;
use crate::core::secure_mesh_trust::DeviceTrustPublicIdentity;

impl SecureMeshPairwiseSession {
    pub fn initiate(
        local_identity: &DeviceTrustPublicIdentity,
        local_identity_secret: &SecureMeshPairwisePrivateKey,
        local_signing_key: &SigningKey,
        remote_bundle: &SecureMeshPairwisePreKeyBundle,
        remote_directory_authorization: &AuthorizedDirectoryLeaf,
        policy: &SecureMeshPreKeyValidationPolicy,
        capability_evaluation: &CapabilityEvaluation,
        now: OffsetDateTime,
    ) -> Result<(Self, SecureMeshPairwiseSessionIntro)> {
        validate_endpoint_id(&local_identity.endpoint_id)?;
        ensure_local_identity_key_material(
            local_identity,
            local_identity_secret,
            local_signing_key,
        )?;
        let validation = validate_pairwise_prekey_bundle(
            remote_bundle,
            remote_directory_authorization,
            policy,
            now,
        )?;
        let local_ephemeral = SecureMeshPairwisePrivateKey::generate();
        let local_ratchet_secret = SecureMeshPairwisePrivateKey::generate();
        let classical_secret = derive_pqxdh_classical_initiator_secret(
            local_identity,
            local_identity_secret,
            &local_ephemeral,
            remote_bundle,
        )?;
        let mlkem1024 =
            encapsulate_ml_kem_1024(&remote_bundle.one_time_mlkem1024_prekey.public_key)?;
        let session_id = derive_session_id(
            local_identity,
            &remote_bundle.endpoint_identity,
            &local_ephemeral.public_key(),
            &validation.signed_prekey_id,
            &remote_bundle.signed_prekey.public_key,
            validation.one_time_prekey_id.as_deref(),
            remote_bundle
                .one_time_prekey
                .as_ref()
                .map(|record| record.public_key.as_slice()),
            &validation.one_time_mlkem1024_prekey_id,
            &remote_bundle.one_time_mlkem1024_prekey.public_key,
            &mlkem1024.ciphertext,
            &validation.directory_authorization_digest,
        )?;
        let triple_ratchet_secrets = derive_triple_ratchet_initial_secrets(
            classical_secret.as_slice(),
            mlkem1024.shared_secret(),
            &local_identity.identity_public_key,
            &remote_bundle.endpoint_identity.identity_public_key,
            session_id.as_bytes(),
        )?;
        let keys = derive_initial_keys(
            triple_ratchet_secrets.ec_secret(),
            &session_id,
            &local_identity.endpoint_id,
            &remote_bundle.endpoint_identity.endpoint_id,
        )?;
        let sparse_pq_ratchet =
            SecureMeshSparsePqRatchet::new_initiator(triple_ratchet_secrets.scka_secret())?;
        let local_ratchet_public_key = local_ratchet_secret.public_key();
        let mut capability_challenge = [0u8; 32];
        OsRng.fill_bytes(&mut capability_challenge);
        let local_capability_proof = sign_capability_proof(
            local_identity,
            local_signing_key,
            capability_evaluation,
            &capability_proof_request(capability_challenge, now)?,
        )?;
        let mut intro = SecureMeshPairwiseSessionIntro {
            protocol_version: SECURE_MESH_PROTOCOL_VERSION.to_string(),
            cipher_suite: SECURE_MESH_PAIRWISE_CIPHER_SUITE.to_string(),
            session_id: session_id.clone(),
            initiator_endpoint_id: local_identity.endpoint_id.clone(),
            responder_endpoint_id: remote_bundle.endpoint_identity.endpoint_id.clone(),
            initiator_identity_public_key: local_identity.identity_public_key.to_vec(),
            initiator_ephemeral_public_key: local_ephemeral.public_key().to_vec(),
            initiator_initial_ratchet_public_key: local_ratchet_public_key.to_vec(),
            responder_signed_prekey_id: validation.signed_prekey_id,
            responder_one_time_prekey_id: validation.one_time_prekey_id,
            responder_one_time_mlkem1024_prekey_id: validation.one_time_mlkem1024_prekey_id,
            mlkem1024_ciphertext: mlkem1024.ciphertext,
            directory_authorization_digest: validation.directory_authorization_digest,
            initiator_capability_proof: local_capability_proof.clone(),
            initiator_signature: String::new(),
        };
        intro.initiator_signature =
            sign_pairwise_transcript(local_signing_key, &intro_signature_payload(&intro)?);
        let handshake_transcript_hash = handshake_transcript_hash(&intro)?;
        let session = Self {
            session_id: session_id.clone(),
            local_endpoint_id: local_identity.endpoint_id.clone(),
            remote_endpoint_id: remote_bundle.endpoint_identity.endpoint_id.clone(),
            role: SecureMeshPairwiseRole::Initiator,
            root_key: Zeroizing::new(keys.root_key),
            sending_chain_key: Zeroizing::new(keys.initiator_chain_key),
            receiving_chain_key: Zeroizing::new(keys.responder_chain_key),
            sending_header_key: Zeroizing::new(keys.initiator_header_key),
            receiving_header_key: Zeroizing::new(keys.responder_header_key),
            next_sending_header_key: Zeroizing::new(keys.initiator_next_header_key),
            next_receiving_header_key: Zeroizing::new(keys.responder_next_header_key),
            skipped_receiving_header_keys: Vec::new(),
            local_ratchet_secret,
            local_ratchet_public_key,
            remote_ratchet_public_key: [0u8; PUBLIC_KEY_LEN],
            handshake_transcript_hash,
            dh_epoch: 0,
            receiving_ratchet_epoch: 0,
            sending_chain_index: 0,
            receiving_chain_index: 0,
            previous_chain_length: 0,
            skipped_keys: Vec::new(),
            received_message_ids: Vec::new(),
            pending_sending_ratchet: false,
            initiator_key_confirmed: false,
            local_capability_proof,
            capability_negotiation: None,
            sparse_pq_ratchet,
            revoked: false,
        };
        Ok((session, intro))
    }

    pub fn accept(
        local_identity: &DeviceTrustPublicIdentity,
        local_identity_secret: &SecureMeshPairwisePrivateKey,
        local_signing_key: &SigningKey,
        expected_initiator_identity: &DeviceTrustPublicIdentity,
        local_signed_prekey_secret: &SecureMeshPairwisePrivateKey,
        local_one_time_prekey_secret: Option<&SecureMeshPairwisePrivateKey>,
        local_one_time_mlkem1024_prekey_seed: &SecureMeshMlKem1024PreKeySeed,
        intro: &SecureMeshPairwiseSessionIntro,
        capability_evaluation: &CapabilityEvaluation,
        now: OffsetDateTime,
        capability_replay_guard: &mut CapabilityProofReplayGuard,
    ) -> Result<(Self, SecureMeshPairwiseSessionAccepted)> {
        ensure_intro(intro)?;
        ensure_local_identity_key_material(
            local_identity,
            local_identity_secret,
            local_signing_key,
        )?;
        ensure!(
            intro.responder_endpoint_id == local_identity.endpoint_id,
            "secure mesh pairwise intro responder mismatch"
        );
        ensure!(
            intro.initiator_endpoint_id == expected_initiator_identity.endpoint_id
                && intro.initiator_identity_public_key
                    == expected_initiator_identity.identity_public_key,
            "secure mesh pairwise intro initiator identity mismatch"
        );
        verify_pairwise_transcript_signature(
            expected_initiator_identity,
            &intro_signature_payload(intro)?,
            &intro.initiator_signature,
            "intro",
        )?;
        let local_one_time_prekey_public_key =
            local_one_time_prekey_secret.map(SecureMeshPairwisePrivateKey::public_key);
        let local_one_time_mlkem1024_prekey_public_key =
            local_one_time_mlkem1024_prekey_seed.public_key();
        let expected_session_id = derive_session_id(
            expected_initiator_identity,
            local_identity,
            &intro.initiator_ephemeral_public_key,
            &intro.responder_signed_prekey_id,
            &local_signed_prekey_secret.public_key(),
            intro.responder_one_time_prekey_id.as_deref(),
            local_one_time_prekey_public_key
                .as_ref()
                .map(|public_key| public_key.as_slice()),
            &intro.responder_one_time_mlkem1024_prekey_id,
            &local_one_time_mlkem1024_prekey_public_key,
            &intro.mlkem1024_ciphertext,
            &intro.directory_authorization_digest,
        )?;
        ensure!(
            intro.session_id == expected_session_id,
            "secure mesh pairwise intro session transcript mismatch"
        );
        let classical_secret = derive_pqxdh_classical_responder_secret(
            local_identity_secret,
            local_signed_prekey_secret,
            local_one_time_prekey_secret,
            intro,
        )?;
        let mlkem1024_shared_secret = decapsulate_ml_kem_1024(
            local_one_time_mlkem1024_prekey_seed,
            &local_one_time_mlkem1024_prekey_public_key,
            &intro.mlkem1024_ciphertext,
        )?;
        let triple_ratchet_secrets = derive_triple_ratchet_initial_secrets(
            classical_secret.as_slice(),
            &mlkem1024_shared_secret,
            &expected_initiator_identity.identity_public_key,
            &local_identity.identity_public_key,
            intro.session_id.as_bytes(),
        )?;
        let initial_keys = derive_initial_keys(
            triple_ratchet_secrets.ec_secret(),
            &intro.session_id,
            &intro.initiator_endpoint_id,
            &intro.responder_endpoint_id,
        )?;
        let sparse_pq_ratchet =
            SecureMeshSparsePqRatchet::new_responder(triple_ratchet_secrets.scka_secret())?;
        let local_ratchet_secret = SecureMeshPairwisePrivateKey::generate();
        let local_ratchet_public_key = local_ratchet_secret.public_key();
        let remote_ratchet_public_key = parse_key_bytes(
            &intro.initiator_initial_ratchet_public_key,
            "initiator ratchet public key",
        )?;
        let handshake_transcript_hash = handshake_transcript_hash(intro)?;
        let capability_challenge =
            signed_capability_proof_challenge(&intro.initiator_capability_proof)?;
        let local_capability_proof = sign_capability_proof(
            local_identity,
            local_signing_key,
            capability_evaluation,
            &capability_proof_request(capability_challenge, now)?,
        )?;
        let verification_context = capability_verification_context(capability_challenge, now)?;
        let initiator_verified =
            crate::core::secure_mesh_capability_proof::verify_capability_proof(
                expected_initiator_identity,
                &intro.initiator_capability_proof,
                &verification_context,
            )?;
        let responder_verified =
            crate::core::secure_mesh_capability_proof::verify_capability_proof(
                local_identity,
                &local_capability_proof,
                &verification_context,
            )?;
        let base_transcript_digest =
            crate::core::secure_mesh_capability_proof::encode_sha256_digest(
                &handshake_transcript_hash,
            );
        let capability_binding = create_pairwise_capability_binding(
            &initiator_verified,
            &responder_verified,
            &base_transcript_digest,
        )?;
        let capability_negotiation = accept_pairwise_capability_binding(
            CapabilityProofPeer {
                identity: local_identity,
                proof: &local_capability_proof,
                verification_context: &verification_context,
            },
            CapabilityProofPeer {
                identity: expected_initiator_identity,
                proof: &intro.initiator_capability_proof,
                verification_context: &verification_context,
            },
            &base_transcript_digest,
            &capability_binding,
            capability_replay_guard,
        )?;
        let keys = derive_capability_bound_initial_keys(
            &initial_keys.root_key,
            &capability_binding.transcript_digest,
            &intro.session_id,
            &intro.initiator_endpoint_id,
            &intro.responder_endpoint_id,
        )?;
        let mut accepted = SecureMeshPairwiseSessionAccepted {
            protocol_version: SECURE_MESH_PROTOCOL_VERSION.to_string(),
            cipher_suite: SECURE_MESH_PAIRWISE_CIPHER_SUITE.to_string(),
            session_id: intro.session_id.clone(),
            responder_endpoint_id: local_identity.endpoint_id.clone(),
            responder_initial_ratchet_public_key: local_ratchet_public_key.to_vec(),
            handshake_transcript_hash: general_purpose::URL_SAFE_NO_PAD
                .encode(handshake_transcript_hash),
            responder_capability_proof: local_capability_proof.clone(),
            capability_binding,
            responder_signature: String::new(),
            key_confirmation: String::new(),
        };
        accepted.responder_signature =
            sign_pairwise_transcript(local_signing_key, &accept_signature_payload(&accepted)?);
        accepted.key_confirmation = pairwise_key_confirmation(&keys.root_key, &accepted)?;
        let session = Self {
            session_id: intro.session_id.clone(),
            local_endpoint_id: local_identity.endpoint_id.clone(),
            remote_endpoint_id: intro.initiator_endpoint_id.clone(),
            role: SecureMeshPairwiseRole::Responder,
            root_key: Zeroizing::new(keys.root_key),
            sending_chain_key: Zeroizing::new(keys.responder_chain_key),
            receiving_chain_key: Zeroizing::new(keys.initiator_chain_key),
            sending_header_key: Zeroizing::new(keys.responder_header_key),
            receiving_header_key: Zeroizing::new(keys.initiator_header_key),
            next_sending_header_key: Zeroizing::new(keys.responder_next_header_key),
            next_receiving_header_key: Zeroizing::new(keys.initiator_next_header_key),
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
            initiator_key_confirmed: false,
            local_capability_proof,
            capability_negotiation: Some(capability_negotiation),
            sparse_pq_ratchet,
            revoked: false,
        };
        Ok((session, accepted))
    }

    pub fn complete_initiator_handshake(
        &mut self,
        local_identity: &DeviceTrustPublicIdentity,
        expected_responder_identity: &DeviceTrustPublicIdentity,
        accepted: &SecureMeshPairwiseSessionAccepted,
        now: OffsetDateTime,
        capability_replay_guard: &mut CapabilityProofReplayGuard,
    ) -> Result<SecureMeshPairwiseSessionFinished> {
        ensure!(
            self.role == SecureMeshPairwiseRole::Initiator,
            "secure mesh pairwise accept can only complete an initiator session"
        );
        ensure!(
            accepted.protocol_version == SECURE_MESH_PROTOCOL_VERSION
                && accepted.cipher_suite == SECURE_MESH_PAIRWISE_CIPHER_SUITE,
            "secure mesh pairwise accept protocol is unsupported"
        );
        ensure!(
            accepted.session_id == self.session_id
                && accepted.responder_endpoint_id == self.remote_endpoint_id
                && local_identity.endpoint_id == self.local_endpoint_id
                && expected_responder_identity.endpoint_id == self.remote_endpoint_id,
            "secure mesh pairwise accept subject mismatch"
        );
        ensure!(
            self.capability_negotiation.is_none(),
            "secure mesh pairwise capability negotiation is already complete"
        );
        let accepted_handshake_hash = decode_fixed_base64url::<HANDSHAKE_HASH_LEN>(
            &accepted.handshake_transcript_hash,
            "accept handshake transcript hash",
        )?;
        ensure!(
            accepted_handshake_hash == self.handshake_transcript_hash,
            "secure mesh pairwise accept handshake transcript mismatch"
        );
        verify_pairwise_transcript_signature(
            expected_responder_identity,
            &accept_signature_payload(accepted)?,
            &accepted.responder_signature,
            "accept",
        )?;
        let remote_ratchet_public_key = parse_key_bytes(
            &accepted.responder_initial_ratchet_public_key,
            "responder ratchet public key",
        )?;
        ensure!(
            remote_ratchet_public_key != [0u8; PUBLIC_KEY_LEN],
            "secure mesh pairwise responder ratchet public key is invalid"
        );
        let capability_challenge = signed_capability_proof_challenge(&self.local_capability_proof)?;
        let verification_context = capability_verification_context(capability_challenge, now)?;
        let base_transcript_digest =
            crate::core::secure_mesh_capability_proof::encode_sha256_digest(
                &self.handshake_transcript_hash,
            );
        let mut verification_guard = CapabilityProofReplayGuard::default();
        let verified_without_consumption = accept_pairwise_capability_binding(
            CapabilityProofPeer {
                identity: local_identity,
                proof: &self.local_capability_proof,
                verification_context: &verification_context,
            },
            CapabilityProofPeer {
                identity: expected_responder_identity,
                proof: &accepted.responder_capability_proof,
                verification_context: &verification_context,
            },
            &base_transcript_digest,
            &accepted.capability_binding,
            &mut verification_guard,
        )?;
        let bound_keys = derive_capability_bound_initial_keys(
            &self.root_key,
            &verified_without_consumption.binding().transcript_digest,
            &self.session_id,
            &self.local_endpoint_id,
            &self.remote_endpoint_id,
        )?;
        verify_pairwise_key_confirmation(&bound_keys.root_key, accepted)?;
        let capability_negotiation = accept_pairwise_capability_binding(
            CapabilityProofPeer {
                identity: local_identity,
                proof: &self.local_capability_proof,
                verification_context: &verification_context,
            },
            CapabilityProofPeer {
                identity: expected_responder_identity,
                proof: &accepted.responder_capability_proof,
                verification_context: &verification_context,
            },
            &base_transcript_digest,
            &accepted.capability_binding,
            capability_replay_guard,
        )?;
        *self.root_key = bound_keys.root_key;
        *self.sending_chain_key = bound_keys.initiator_chain_key;
        *self.receiving_chain_key = bound_keys.responder_chain_key;
        *self.sending_header_key = bound_keys.initiator_header_key;
        *self.receiving_header_key = bound_keys.responder_header_key;
        *self.next_sending_header_key = bound_keys.initiator_next_header_key;
        *self.next_receiving_header_key = bound_keys.responder_next_header_key;
        self.remote_ratchet_public_key = remote_ratchet_public_key;
        self.capability_negotiation = Some(capability_negotiation);
        self.initiator_key_confirmed = true;
        // The first protected message performs a fresh DH step for post-compromise recovery.
        // Initiator authentication itself is explicit and cannot be inferred from app data.
        self.pending_sending_ratchet = true;
        let mut finished = SecureMeshPairwiseSessionFinished {
            protocol_version: SECURE_MESH_PROTOCOL_VERSION.to_string(),
            cipher_suite: SECURE_MESH_PAIRWISE_CIPHER_SUITE.to_string(),
            session_id: self.session_id.clone(),
            initiator_endpoint_id: self.local_endpoint_id.clone(),
            responder_endpoint_id: self.remote_endpoint_id.clone(),
            handshake_transcript_hash: accepted.handshake_transcript_hash.clone(),
            capability_transcript_digest: accepted.capability_binding.transcript_digest.clone(),
            key_confirmation: String::new(),
        };
        finished.key_confirmation = initiator_finished_key_confirmation(&self.root_key, &finished)?;
        Ok(finished)
    }

    pub fn complete_responder_handshake(
        &mut self,
        finished: &SecureMeshPairwiseSessionFinished,
    ) -> Result<()> {
        ensure!(
            self.role == SecureMeshPairwiseRole::Responder,
            "secure mesh pairwise finished can only complete a responder session"
        );
        ensure!(
            !self.initiator_key_confirmed,
            "secure mesh pairwise initiator finished is already complete"
        );
        ensure!(
            finished.protocol_version == SECURE_MESH_PROTOCOL_VERSION
                && finished.cipher_suite == SECURE_MESH_PAIRWISE_CIPHER_SUITE,
            "secure mesh pairwise finished protocol is unsupported"
        );
        ensure!(
            finished.session_id == self.session_id
                && finished.initiator_endpoint_id == self.remote_endpoint_id
                && finished.responder_endpoint_id == self.local_endpoint_id,
            "secure mesh pairwise finished subject mismatch"
        );
        ensure!(
            decode_fixed_base64url::<HANDSHAKE_HASH_LEN>(
                &finished.handshake_transcript_hash,
                "finished handshake transcript hash",
            )? == self.handshake_transcript_hash,
            "secure mesh pairwise finished handshake transcript mismatch"
        );
        let binding = self
            .capability_binding()
            .ok_or_else(|| anyhow!("secure mesh pairwise capability negotiation is incomplete"))?;
        ensure!(
            finished.capability_transcript_digest == binding.transcript_digest,
            "secure mesh pairwise finished capability transcript mismatch"
        );
        verify_initiator_finished_key_confirmation(&self.root_key, finished)?;
        self.initiator_key_confirmed = true;
        Ok(())
    }

    pub fn capability_projection(&self) -> Option<&ClientCapabilityProjection> {
        self.capability_negotiation
            .as_ref()
            .map(VerifiedSessionNegotiation::projection)
    }

    pub fn capability_binding(&self) -> Option<&NegotiatedCapabilityBinding> {
        self.capability_negotiation
            .as_ref()
            .map(VerifiedSessionNegotiation::binding)
    }

    pub(crate) fn local_capability_proof(&self) -> &SignedCapabilityProof {
        &self.local_capability_proof
    }

    pub fn handshake_confirmed(&self) -> bool {
        self.initiator_key_confirmed && self.capability_negotiation.is_some() && !self.revoked
    }

    pub(in crate::core::secure_mesh_pairwise) fn require_capability_negotiation(
        &self,
    ) -> Result<()> {
        ensure!(
            self.capability_negotiation.is_some(),
            "secure mesh pairwise capability negotiation is incomplete"
        );
        Ok(())
    }
}
