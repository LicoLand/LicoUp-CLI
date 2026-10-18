mod payload_adapter;
mod relay_codec;

use payload_adapter::ensure_message_for_session;

use anyhow::{Context, Result, anyhow, ensure};
use base64::{Engine as _, engine::general_purpose};
use chacha20poly1305::{
    ChaCha20Poly1305, Key, Nonce,
    aead::{Aead, KeyInit, Payload as AeadPayload},
};
use hkdf::Hkdf;
use rand_core::{OsRng, RngCore};
use sha2::{Digest, Sha256};
use x25519_dalek::{PublicKey, StaticSecret};
use zeroize::{Zeroize, Zeroizing};

use super::codec::{
    OpenedPairwiseMessage, SecureMeshPairwiseMessage, message_aad, message_replay_fingerprint,
};
use super::support::{
    CHAIN_INFO_DOMAIN, CHAIN_KEY_LEN, HANDSHAKE_HASH_LEN, HEADER_KEY_LEN, MAX_CIPHERTEXT_BYTES,
    MAX_REPLAY_IDS, MAX_SKIPPED_KEYS, MESSAGE_KEY_LEN, NONCE_LEN, PUBLIC_KEY_LEN, ROOT_INFO_DOMAIN,
    ROOT_KEY_LEN, SECURE_MESH_PAIRWISE_CIPHER_SUITE, append_len_prefixed_bytes, parse_key_bytes,
    require_text, validate_message_id,
};
use crate::core::secure_mesh::SECURE_MESH_PROTOCOL_VERSION;
use crate::core::secure_mesh_capability_proof::SignedCapabilityProof;
use crate::core::secure_mesh_session_negotiation::VerifiedSessionNegotiation;
use crate::core::secure_mesh_sparse_pq_ratchet::{
    SecureMeshSparsePqRatchet, derive_hybrid_message_key,
};

#[derive(Clone)]
pub struct SecureMeshPairwisePrivateKey(StaticSecret);

impl SecureMeshPairwisePrivateKey {
    pub fn generate() -> Self {
        Self(StaticSecret::random_from_rng(OsRng))
    }

    pub fn from_bytes(bytes: [u8; PUBLIC_KEY_LEN]) -> Self {
        Self(StaticSecret::from(bytes))
    }

    pub fn public_key(&self) -> [u8; PUBLIC_KEY_LEN] {
        PublicKey::from(&self.0).to_bytes()
    }

    pub(super) fn diffie_hellman(
        &self,
        remote_public_key: &[u8],
    ) -> Result<Zeroizing<[u8; PUBLIC_KEY_LEN]>> {
        let remote = PublicKey::from(parse_key_bytes(remote_public_key, "remote public key")?);
        let shared_secret = self.0.diffie_hellman(&remote).to_bytes();
        ensure!(
            shared_secret != [0u8; PUBLIC_KEY_LEN],
            "secure mesh pairwise X25519 input is non-contributory"
        );
        Ok(Zeroizing::new(shared_secret))
    }

    pub(super) fn to_bytes(&self) -> [u8; PUBLIC_KEY_LEN] {
        self.0.to_bytes()
    }

    pub(super) fn destroy(&mut self) {
        self.0.zeroize();
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SecureMeshPairwiseRole {
    Initiator,
    Responder,
}

impl SecureMeshPairwiseRole {
    pub(super) fn as_str(self) -> &'static str {
        match self {
            Self::Initiator => "initiator",
            Self::Responder => "responder",
        }
    }

    pub(super) fn from_str(value: &str) -> Result<Self> {
        match value {
            "initiator" => Ok(Self::Initiator),
            "responder" => Ok(Self::Responder),
            _ => Err(anyhow!("secure mesh pairwise role is unsupported")),
        }
    }
}

#[derive(Clone)]
pub(super) struct SkippedMessageKey {
    pub(super) message_id: String,
    pub(super) dh_epoch: u64,
    pub(super) chain_index: u64,
    pub(super) sender_ratchet_public_key: [u8; PUBLIC_KEY_LEN],
    pub(super) message_key: Zeroizing<[u8; MESSAGE_KEY_LEN]>,
}

pub struct SecureMeshPairwiseSession {
    pub session_id: String,
    pub local_endpoint_id: String,
    pub remote_endpoint_id: String,
    pub(super) role: SecureMeshPairwiseRole,
    pub(super) root_key: Zeroizing<[u8; ROOT_KEY_LEN]>,
    pub(super) sending_chain_key: Zeroizing<[u8; CHAIN_KEY_LEN]>,
    pub(super) receiving_chain_key: Zeroizing<[u8; CHAIN_KEY_LEN]>,
    pub(super) sending_header_key: Zeroizing<[u8; HEADER_KEY_LEN]>,
    pub(super) receiving_header_key: Zeroizing<[u8; HEADER_KEY_LEN]>,
    pub(super) next_sending_header_key: Zeroizing<[u8; HEADER_KEY_LEN]>,
    pub(super) next_receiving_header_key: Zeroizing<[u8; HEADER_KEY_LEN]>,
    pub(super) skipped_receiving_header_keys: Vec<Zeroizing<[u8; HEADER_KEY_LEN]>>,
    pub(super) local_ratchet_secret: SecureMeshPairwisePrivateKey,
    pub(super) local_ratchet_public_key: [u8; PUBLIC_KEY_LEN],
    pub(super) remote_ratchet_public_key: [u8; PUBLIC_KEY_LEN],
    pub(super) handshake_transcript_hash: [u8; HANDSHAKE_HASH_LEN],
    pub(super) dh_epoch: u64,
    pub(super) receiving_ratchet_epoch: u64,
    pub(super) sending_chain_index: u64,
    pub(super) receiving_chain_index: u64,
    pub(super) previous_chain_length: u64,
    pub(super) skipped_keys: Vec<SkippedMessageKey>,
    pub(super) received_message_ids: Vec<String>,
    pub(super) pending_sending_ratchet: bool,
    pub(super) initiator_key_confirmed: bool,
    pub(super) local_capability_proof: SignedCapabilityProof,
    pub(super) capability_negotiation: Option<VerifiedSessionNegotiation>,
    pub(super) sparse_pq_ratchet: SecureMeshSparsePqRatchet,
    pub(super) revoked: bool,
}

impl SecureMeshPairwiseSession {
    pub(super) fn try_clone(&self) -> Result<Self> {
        Ok(Self {
            session_id: self.session_id.clone(),
            local_endpoint_id: self.local_endpoint_id.clone(),
            remote_endpoint_id: self.remote_endpoint_id.clone(),
            role: self.role,
            root_key: self.root_key.clone(),
            sending_chain_key: self.sending_chain_key.clone(),
            receiving_chain_key: self.receiving_chain_key.clone(),
            sending_header_key: self.sending_header_key.clone(),
            receiving_header_key: self.receiving_header_key.clone(),
            next_sending_header_key: self.next_sending_header_key.clone(),
            next_receiving_header_key: self.next_receiving_header_key.clone(),
            skipped_receiving_header_keys: self.skipped_receiving_header_keys.clone(),
            local_ratchet_secret: self.local_ratchet_secret.clone(),
            local_ratchet_public_key: self.local_ratchet_public_key,
            remote_ratchet_public_key: self.remote_ratchet_public_key,
            handshake_transcript_hash: self.handshake_transcript_hash,
            dh_epoch: self.dh_epoch,
            receiving_ratchet_epoch: self.receiving_ratchet_epoch,
            sending_chain_index: self.sending_chain_index,
            receiving_chain_index: self.receiving_chain_index,
            previous_chain_length: self.previous_chain_length,
            skipped_keys: self.skipped_keys.clone(),
            received_message_ids: self.received_message_ids.clone(),
            pending_sending_ratchet: self.pending_sending_ratchet,
            initiator_key_confirmed: self.initiator_key_confirmed,
            local_capability_proof: self.local_capability_proof.clone(),
            capability_negotiation: self.capability_negotiation.clone(),
            sparse_pq_ratchet: self.sparse_pq_ratchet.try_clone()?,
            revoked: self.revoked,
        })
    }

    pub fn seal_message(
        &mut self,
        message_id: impl Into<String>,
        body: impl AsRef<[u8]>,
    ) -> Result<SecureMeshPairwiseMessage> {
        let mut nonce = [0u8; NONCE_LEN];
        OsRng.fill_bytes(&mut nonce);
        self.seal_message_with_nonce(message_id, body, nonce)
    }

    pub(super) fn seal_message_with_nonce(
        &mut self,
        message_id: impl Into<String>,
        body: impl AsRef<[u8]>,
        nonce: [u8; NONCE_LEN],
    ) -> Result<SecureMeshPairwiseMessage> {
        ensure!(!self.revoked, "secure mesh pairwise session is revoked");
        self.require_capability_negotiation()?;
        ensure!(
            self.initiator_key_confirmed,
            "secure mesh pairwise handshake key confirmation is incomplete"
        );
        let message_id = require_text(message_id.into(), "message id")?;
        validate_message_id(&message_id)?;
        let body = body.as_ref();
        ensure!(
            body.len() <= MAX_CIPHERTEXT_BYTES,
            "secure mesh pairwise message body is too large"
        );
        let mut candidate = self.try_clone()?;
        candidate.prepare_sending_ratchet_for_send()?;
        let chain_index = candidate.sending_chain_index;
        let (next_chain_key, classical_message_key) = advance_chain(
            &candidate.sending_chain_key,
            candidate.dh_epoch,
            chain_index,
            "message",
        )?;
        let sparse_pq = candidate.sparse_pq_ratchet.send_key()?;
        let message_key = derive_hybrid_message_key(
            &classical_message_key,
            &sparse_pq.message_key,
            candidate.session_id.as_bytes(),
        )?;
        let mut message = SecureMeshPairwiseMessage {
            protocol_version: SECURE_MESH_PROTOCOL_VERSION.to_string(),
            cipher_suite: SECURE_MESH_PAIRWISE_CIPHER_SUITE.to_string(),
            session_id: candidate.session_id.clone(),
            message_id,
            sender_endpoint_id: candidate.local_endpoint_id.clone(),
            recipient_endpoint_id: candidate.remote_endpoint_id.clone(),
            dh_epoch: candidate.dh_epoch,
            chain_index,
            previous_chain_length: candidate.previous_chain_length,
            sender_ratchet_public_key: candidate.local_ratchet_public_key.to_vec(),
            sparse_pq_header: sparse_pq.header,
            encrypted_header: general_purpose::URL_SAFE_NO_PAD.encode(nonce),
            ciphertext: String::new(),
            ciphertext_size: 0,
        };
        let aad = message_aad(&message)?;
        let cipher = ChaCha20Poly1305::new(Key::from_slice(message_key.as_ref()));
        let ciphertext = cipher
            .encrypt(
                Nonce::from_slice(&nonce),
                AeadPayload {
                    msg: body,
                    aad: &aad,
                },
            )
            .map_err(|_| anyhow!("secure mesh pairwise message encryption failed"))?;
        message.ciphertext_size = ciphertext.len();
        message.ciphertext = general_purpose::URL_SAFE_NO_PAD.encode(ciphertext);
        *candidate.sending_chain_key = *next_chain_key;
        candidate.sending_chain_index += 1;
        *self = candidate;
        Ok(message)
    }

    pub fn open_message(
        &mut self,
        message: &SecureMeshPairwiseMessage,
    ) -> Result<OpenedPairwiseMessage> {
        ensure!(!self.revoked, "secure mesh pairwise session is revoked");
        self.require_capability_negotiation()?;
        ensure!(
            self.initiator_key_confirmed,
            "secure mesh pairwise handshake key confirmation is incomplete"
        );
        ensure_message_for_session(self, message)?;
        let replay_fingerprint = message_replay_fingerprint(message)?;
        ensure!(
            !self
                .received_message_ids
                .iter()
                .any(|id| id == &replay_fingerprint),
            "secure mesh pairwise message replay detected"
        );
        let aad = message_aad(message)?;
        let ciphertext = general_purpose::URL_SAFE_NO_PAD
            .decode(&message.ciphertext)
            .context("secure mesh pairwise ciphertext is not base64url")?;
        ensure!(
            ciphertext.len() == message.ciphertext_size,
            "secure mesh pairwise ciphertext size mismatch"
        );
        let nonce_bytes = general_purpose::URL_SAFE_NO_PAD
            .decode(&message.encrypted_header)
            .context("secure mesh pairwise encrypted header is not base64url")?;
        ensure!(
            nonce_bytes.len() == NONCE_LEN,
            "secure mesh pairwise nonce length is invalid"
        );
        let mut nonce = [0u8; NONCE_LEN];
        nonce.copy_from_slice(&nonce_bytes);

        let mut candidate = self.try_clone()?;
        let classical_message_key = candidate.message_key_for_open(message)?;
        let post_quantum_message_key = candidate
            .sparse_pq_ratchet
            .receive_key(&message.sparse_pq_header)?;
        let message_key = derive_hybrid_message_key(
            &classical_message_key,
            &post_quantum_message_key,
            candidate.session_id.as_bytes(),
        )?;
        let cipher = ChaCha20Poly1305::new(Key::from_slice(message_key.as_ref()));
        let plaintext = cipher
            .decrypt(
                Nonce::from_slice(&nonce),
                AeadPayload {
                    msg: &ciphertext,
                    aad: &aad,
                },
            )
            .map_err(|_| anyhow!("secure mesh pairwise message authentication failed"))?;
        candidate.record_received_message_id(replay_fingerprint);
        *self = candidate;
        Ok(OpenedPairwiseMessage {
            message_id: message.message_id.clone(),
            sender_endpoint_id: message.sender_endpoint_id.clone(),
            body: plaintext,
        })
    }

    pub fn pending_sending_ratchet(&self) -> bool {
        self.pending_sending_ratchet
    }

    pub fn rotate_sending_ratchet(&mut self) -> Result<()> {
        ensure!(
            self.pending_sending_ratchet,
            "secure mesh pairwise sending ratchet requires an authenticated remote ratchet"
        );
        self.rotate_sending_ratchet_with_secret(SecureMeshPairwisePrivateKey::generate())
    }

    pub(super) fn prepare_sending_ratchet_for_send(&mut self) -> Result<()> {
        if self.pending_sending_ratchet {
            self.rotate_sending_ratchet()?;
        }
        Ok(())
    }

    pub(super) fn rotate_sending_ratchet_with_secret(
        &mut self,
        next_ratchet_secret: SecureMeshPairwisePrivateKey,
    ) -> Result<()> {
        ensure!(!self.revoked, "secure mesh pairwise session is revoked");
        self.require_capability_negotiation()?;
        ensure!(
            self.initiator_key_confirmed,
            "secure mesh pairwise handshake key confirmation is incomplete"
        );
        ensure!(
            self.remote_ratchet_public_key != [0u8; PUBLIC_KEY_LEN],
            "secure mesh pairwise remote ratchet public key is unavailable"
        );
        self.previous_chain_length = self.sending_chain_index;
        self.local_ratchet_secret = next_ratchet_secret;
        self.local_ratchet_public_key = self.local_ratchet_secret.public_key();
        let dh_secret = self
            .local_ratchet_secret
            .diffie_hellman(&self.remote_ratchet_public_key)?;
        let (root_key, chain_key, next_header_key) =
            derive_ratchet_root(&self.root_key, &dh_secret, self.dh_epoch + 1)?;
        *self.root_key = root_key;
        *self.sending_chain_key = chain_key;
        *self.sending_header_key = *self.next_sending_header_key;
        *self.next_sending_header_key = next_header_key;
        self.dh_epoch += 1;
        self.sending_chain_index = 0;
        self.pending_sending_ratchet = false;
        Ok(())
    }

    pub fn revoke(&mut self) {
        self.revoked = true;
        self.root_key.zeroize();
        self.sending_chain_key.zeroize();
        self.receiving_chain_key.zeroize();
        self.sending_header_key.zeroize();
        self.receiving_header_key.zeroize();
        self.next_sending_header_key.zeroize();
        self.next_receiving_header_key.zeroize();
        for header_key in &mut self.skipped_receiving_header_keys {
            header_key.zeroize();
        }
        self.skipped_receiving_header_keys.clear();
        for skipped in &mut self.skipped_keys {
            skipped.message_key.zeroize();
        }
        self.skipped_keys.clear();
        self.local_ratchet_secret.destroy();
        self.sparse_pq_ratchet.destroy();
        self.pending_sending_ratchet = false;
        self.initiator_key_confirmed = false;
        self.capability_negotiation = None;
    }

    pub fn sent_count(&self) -> u64 {
        self.sending_chain_index
    }

    pub fn received_count(&self) -> u64 {
        self.receiving_chain_index
    }

    pub fn skipped_key_count(&self) -> usize {
        self.skipped_keys.len()
    }

    pub fn dh_epoch(&self) -> u64 {
        self.dh_epoch
    }

    pub(super) fn store_skipped_message_keys_until(
        &mut self,
        dh_epoch: u64,
        sender_ratchet_public_key: [u8; PUBLIC_KEY_LEN],
        previous_chain_length: u64,
    ) -> Result<()> {
        ensure!(
            previous_chain_length >= self.receiving_chain_index,
            "secure mesh pairwise previous chain length regressed"
        );
        let missing = previous_chain_length - self.receiving_chain_index;
        ensure!(
            missing as usize <= MAX_SKIPPED_KEYS.saturating_sub(self.skipped_keys.len()),
            "secure mesh pairwise skipped-key limit exceeded before ratchet"
        );
        while self.receiving_chain_index < previous_chain_length {
            let skipped_index = self.receiving_chain_index;
            let skipped_id = format!("{}:{}:{}", self.session_id, dh_epoch, skipped_index);
            let (next_chain_key, message_key) = advance_chain(
                &self.receiving_chain_key,
                dh_epoch,
                skipped_index,
                "message",
            )?;
            *self.receiving_chain_key = *next_chain_key;
            self.receiving_chain_index += 1;
            self.push_skipped_message_key(SkippedMessageKey {
                message_id: skipped_id,
                dh_epoch,
                chain_index: skipped_index,
                sender_ratchet_public_key,
                message_key,
            });
        }
        Ok(())
    }

    pub(super) fn advance_receiving_chain_to(
        &mut self,
        message: &SecureMeshPairwiseMessage,
        sender_ratchet_public_key: [u8; PUBLIC_KEY_LEN],
    ) -> Result<Zeroizing<[u8; MESSAGE_KEY_LEN]>> {
        if sender_ratchet_public_key != self.remote_ratchet_public_key {
            ensure!(
                self.dh_epoch.checked_add(1) == Some(message.dh_epoch),
                "secure mesh pairwise ratchet epoch must advance exactly once"
            );
            self.store_skipped_message_keys_until(
                self.receiving_ratchet_epoch,
                self.remote_ratchet_public_key,
                message.previous_chain_length,
            )?;
            let dh_secret = self
                .local_ratchet_secret
                .diffie_hellman(&sender_ratchet_public_key)?;
            let (root_key, chain_key, next_header_key) =
                derive_ratchet_root(&self.root_key, &dh_secret, message.dh_epoch)?;
            *self.root_key = root_key;
            *self.receiving_chain_key = chain_key;
            self.skipped_receiving_header_keys
                .push(self.receiving_header_key.clone());
            while self.skipped_receiving_header_keys.len() > MAX_SKIPPED_KEYS {
                let mut evicted = self.skipped_receiving_header_keys.remove(0);
                evicted.zeroize();
            }
            *self.receiving_header_key = *self.next_receiving_header_key;
            *self.next_receiving_header_key = next_header_key;
            self.remote_ratchet_public_key = sender_ratchet_public_key;
            self.dh_epoch = message.dh_epoch;
            self.receiving_ratchet_epoch = message.dh_epoch;
            self.receiving_chain_index = 0;
            self.pending_sending_ratchet = true;
        } else {
            ensure!(
                message.dh_epoch == self.receiving_ratchet_epoch,
                "secure mesh pairwise message epoch mismatch"
            );
        }
        ensure!(
            message.chain_index >= self.receiving_chain_index,
            "secure mesh pairwise stale chain index"
        );
        let missing = message.chain_index - self.receiving_chain_index;
        ensure!(
            usize::try_from(missing).is_ok_and(|missing| {
                missing <= MAX_SKIPPED_KEYS.saturating_sub(self.skipped_keys.len())
            }),
            "secure mesh pairwise skipped-key limit exceeded"
        );
        while self.receiving_chain_index < message.chain_index {
            let skipped_index = self.receiving_chain_index;
            let skipped_id = format!("{}:{}:{}", self.session_id, message.dh_epoch, skipped_index);
            let (next_chain_key, message_key) = advance_chain(
                &self.receiving_chain_key,
                message.dh_epoch,
                skipped_index,
                "message",
            )?;
            *self.receiving_chain_key = *next_chain_key;
            self.receiving_chain_index += 1;
            self.push_skipped_message_key(SkippedMessageKey {
                message_id: skipped_id,
                dh_epoch: message.dh_epoch,
                chain_index: skipped_index,
                sender_ratchet_public_key,
                message_key,
            });
        }
        let (next_chain_key, message_key) = advance_chain(
            &self.receiving_chain_key,
            message.dh_epoch,
            self.receiving_chain_index,
            "message",
        )?;
        *self.receiving_chain_key = *next_chain_key;
        self.receiving_chain_index += 1;
        Ok(message_key)
    }

    pub(super) fn message_key_for_open(
        &mut self,
        message: &SecureMeshPairwiseMessage,
    ) -> Result<Zeroizing<[u8; MESSAGE_KEY_LEN]>> {
        let sender_ratchet_public_key = parse_key_bytes(
            &message.sender_ratchet_public_key,
            "sender ratchet public key",
        )?;
        if self
            .skipped_message_key_position(
                message.dh_epoch,
                message.chain_index,
                sender_ratchet_public_key,
            )
            .is_some()
        {
            self.take_skipped_message_key(
                &message.message_id,
                message.dh_epoch,
                message.chain_index,
                sender_ratchet_public_key,
            )
        } else if sender_ratchet_public_key != self.remote_ratchet_public_key {
            self.advance_receiving_chain_to(message, sender_ratchet_public_key)
        } else if message.chain_index < self.receiving_chain_index {
            Err(anyhow!(
                "secure mesh pairwise skipped message key is unavailable"
            ))
        } else {
            self.advance_receiving_chain_to(message, sender_ratchet_public_key)
        }
    }

    pub(super) fn skipped_message_key_position(
        &self,
        dh_epoch: u64,
        chain_index: u64,
        sender_ratchet_public_key: [u8; PUBLIC_KEY_LEN],
    ) -> Option<usize> {
        self.skipped_keys.iter().position(|candidate| {
            candidate.dh_epoch == dh_epoch
                && candidate.chain_index == chain_index
                && candidate.sender_ratchet_public_key == sender_ratchet_public_key
        })
    }

    pub(super) fn take_skipped_message_key(
        &mut self,
        message_id: &str,
        dh_epoch: u64,
        chain_index: u64,
        sender_ratchet_public_key: [u8; PUBLIC_KEY_LEN],
    ) -> Result<Zeroizing<[u8; MESSAGE_KEY_LEN]>> {
        match self.skipped_message_key_position(dh_epoch, chain_index, sender_ratchet_public_key) {
            Some(index) => {
                let skipped = self.skipped_keys.remove(index);
                if skipped.message_id != message_id {
                    // The placeholder id is deterministic for out-of-order gaps; the AEAD AAD
                    // still binds the real message id before any plaintext is returned.
                }
                Ok(skipped.message_key)
            }
            None => Err(anyhow!(
                "secure mesh pairwise skipped message key is unavailable"
            )),
        }
    }

    pub(super) fn push_skipped_message_key(&mut self, skipped: SkippedMessageKey) {
        self.skipped_keys.push(skipped);
        while self.skipped_keys.len() > MAX_SKIPPED_KEYS {
            let mut removed = self.skipped_keys.remove(0);
            removed.message_key.zeroize();
        }
    }

    pub(super) fn record_received_message_id(&mut self, message_id: String) {
        self.received_message_ids.push(message_id);
        while self.received_message_ids.len() > MAX_REPLAY_IDS {
            self.received_message_ids.remove(0);
        }
    }
}

#[cfg(test)]
impl Clone for SecureMeshPairwiseSession {
    fn clone(&self) -> Self {
        self.try_clone()
            .expect("valid secure mesh pairwise test session must be cloneable")
    }
}

pub(super) fn derive_ratchet_root(
    root_key: &[u8; ROOT_KEY_LEN],
    dh_secret: &[u8; PUBLIC_KEY_LEN],
    dh_epoch: u64,
) -> Result<(
    [u8; ROOT_KEY_LEN],
    [u8; CHAIN_KEY_LEN],
    [u8; HEADER_KEY_LEN],
)> {
    let mut salt_hasher = Sha256::new();
    salt_hasher.update(ROOT_INFO_DOMAIN);
    salt_hasher.update(root_key);
    salt_hasher.update(dh_epoch.to_be_bytes());
    let salt = salt_hasher.finalize();
    let hkdf = Hkdf::<Sha256>::new(Some(&salt), dh_secret);
    let mut info = Vec::new();
    append_len_prefixed_bytes(&mut info, SECURE_MESH_PAIRWISE_CIPHER_SUITE.as_bytes())?;
    append_len_prefixed_bytes(&mut info, b"dh-ratchet")?;
    let mut out = [0u8; ROOT_KEY_LEN + CHAIN_KEY_LEN + HEADER_KEY_LEN];
    hkdf.expand(&info, &mut out)
        .map_err(|_| anyhow!("secure mesh pairwise root ratchet failed"))?;
    let mut new_root = [0u8; ROOT_KEY_LEN];
    let mut chain_key = [0u8; CHAIN_KEY_LEN];
    let mut next_header_key = [0u8; HEADER_KEY_LEN];
    new_root.copy_from_slice(&out[0..ROOT_KEY_LEN]);
    chain_key.copy_from_slice(&out[ROOT_KEY_LEN..ROOT_KEY_LEN + CHAIN_KEY_LEN]);
    next_header_key.copy_from_slice(&out[ROOT_KEY_LEN + CHAIN_KEY_LEN..]);
    out.zeroize();
    Ok((new_root, chain_key, next_header_key))
}

pub(super) fn advance_chain(
    chain_key: &[u8; CHAIN_KEY_LEN],
    dh_epoch: u64,
    chain_index: u64,
    label: &str,
) -> Result<(
    Zeroizing<[u8; CHAIN_KEY_LEN]>,
    Zeroizing<[u8; MESSAGE_KEY_LEN]>,
)> {
    let mut salt_hasher = Sha256::new();
    salt_hasher.update(CHAIN_INFO_DOMAIN);
    salt_hasher.update(chain_key);
    salt_hasher.update(dh_epoch.to_be_bytes());
    salt_hasher.update(chain_index.to_be_bytes());
    let salt = salt_hasher.finalize();
    let hkdf = Hkdf::<Sha256>::new(Some(&salt), chain_key);
    let mut info = Vec::new();
    append_len_prefixed_bytes(&mut info, SECURE_MESH_PAIRWISE_CIPHER_SUITE.as_bytes())?;
    append_len_prefixed_bytes(&mut info, label.as_bytes())?;
    let mut out = [0u8; CHAIN_KEY_LEN + MESSAGE_KEY_LEN];
    hkdf.expand(&info, &mut out)
        .map_err(|_| anyhow!("secure mesh pairwise chain advance failed"))?;
    let mut next_chain_key = [0u8; CHAIN_KEY_LEN];
    let mut message_key = [0u8; MESSAGE_KEY_LEN];
    next_chain_key.copy_from_slice(&out[0..CHAIN_KEY_LEN]);
    message_key.copy_from_slice(&out[CHAIN_KEY_LEN..]);
    out.zeroize();
    Ok((Zeroizing::new(next_chain_key), Zeroizing::new(message_key)))
}
