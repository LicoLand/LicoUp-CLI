use anyhow::{Context, Result, anyhow, ensure};
use base64::{Engine as _, engine::general_purpose};
use zeroize::Zeroizing;

use super::super::{
    key_ratchet::{
        SecureMeshPairwisePrivateKey, SecureMeshPairwiseRole, SecureMeshPairwiseSession,
        SkippedMessageKey,
    },
    session_negotiation::decode_fixed_base64url,
    support::{
        HANDSHAKE_HASH_LEN, MAX_ENCODED_SPARSE_PQ_RATCHET_BYTES, MAX_SKIPPED_KEYS,
        PAIRWISE_SECRET_STORE_CLASS, PAIRWISE_SNAPSHOT_SCHEMA_VERSION, decode_secret_32,
        encode_secret, require_text, validate_endpoint_id,
    },
};
use super::{
    public_snapshot::{PersistedPairwisePublicSession, PersistedSkippedMessageKeyPublic},
    secret_snapshot::{
        PairwiseSecretString, PersistedPairwiseSessionSecrets, PersistedSkippedMessageKeySecret,
    },
};
use crate::core::secure_mesh_capability_proof::signed_capability_proof_digest;
use crate::core::secure_mesh_session_negotiation::restore_verified_pairwise_session_negotiation;
use crate::core::secure_mesh_sparse_pq_ratchet::SecureMeshSparsePqRatchet;

impl SecureMeshPairwiseSession {
    pub(in crate::core::secure_mesh_pairwise) fn to_public_snapshot(
        &self,
        state_version: u64,
        secret_store_namespace: String,
        secret_store_key: String,
    ) -> PersistedPairwisePublicSession {
        PersistedPairwisePublicSession {
            schema_version: PAIRWISE_SNAPSHOT_SCHEMA_VERSION,
            state_version,
            secret_store_class: PAIRWISE_SECRET_STORE_CLASS.to_string(),
            secret_store_namespace,
            secret_store_key,
            session_id: self.session_id.clone(),
            local_endpoint_id: self.local_endpoint_id.clone(),
            remote_endpoint_id: self.remote_endpoint_id.clone(),
            role: self.role.as_str().to_string(),
            local_ratchet_public_key: encode_secret(&self.local_ratchet_public_key),
            remote_ratchet_public_key: encode_secret(&self.remote_ratchet_public_key),
            handshake_transcript_hash: general_purpose::URL_SAFE_NO_PAD
                .encode(self.handshake_transcript_hash),
            dh_epoch: self.dh_epoch,
            receiving_ratchet_epoch: self.receiving_ratchet_epoch,
            sending_chain_index: self.sending_chain_index,
            receiving_chain_index: self.receiving_chain_index,
            previous_chain_length: self.previous_chain_length,
            skipped_keys: self
                .skipped_keys
                .iter()
                .map(PersistedSkippedMessageKeyPublic::from)
                .collect(),
            received_message_ids: self.received_message_ids.clone(),
            pending_sending_ratchet: self.pending_sending_ratchet,
            initiator_key_confirmed: self.initiator_key_confirmed,
            local_capability_proof: self.local_capability_proof.clone(),
            capability_binding: self
                .capability_negotiation
                .as_ref()
                .map(|negotiation| negotiation.binding().clone()),
            capability_projection: self
                .capability_negotiation
                .as_ref()
                .map(|negotiation| negotiation.projection().clone()),
            revoked: self.revoked,
        }
    }

    pub(in crate::core::secure_mesh_pairwise) fn to_secret_snapshot(
        &self,
        state_version: u64,
        public_snapshot_digest: String,
    ) -> Result<PersistedPairwiseSessionSecrets> {
        let sparse_pq_ratchet = self.sparse_pq_ratchet.persist()?;
        let local_ratchet_secret = Zeroizing::new(self.local_ratchet_secret.to_bytes());
        Ok(PersistedPairwiseSessionSecrets {
            schema_version: PAIRWISE_SNAPSHOT_SCHEMA_VERSION,
            state_version,
            session_id: self.session_id.clone(),
            local_endpoint_id: self.local_endpoint_id.clone(),
            remote_endpoint_id: self.remote_endpoint_id.clone(),
            public_snapshot_digest,
            root_key: PairwiseSecretString::new(encode_secret(&self.root_key)),
            sending_chain_key: PairwiseSecretString::new(encode_secret(&self.sending_chain_key)),
            receiving_chain_key: PairwiseSecretString::new(encode_secret(
                &self.receiving_chain_key,
            )),
            sending_header_key: PairwiseSecretString::new(encode_secret(&self.sending_header_key)),
            receiving_header_key: PairwiseSecretString::new(encode_secret(
                &self.receiving_header_key,
            )),
            next_sending_header_key: PairwiseSecretString::new(encode_secret(
                &self.next_sending_header_key,
            )),
            next_receiving_header_key: PairwiseSecretString::new(encode_secret(
                &self.next_receiving_header_key,
            )),
            skipped_receiving_header_keys: self
                .skipped_receiving_header_keys
                .iter()
                .map(|key| PairwiseSecretString::new(encode_secret(key)))
                .collect(),
            local_ratchet_secret: PairwiseSecretString::new(encode_secret(&local_ratchet_secret)),
            sparse_pq_ratchet: PairwiseSecretString::new(
                general_purpose::URL_SAFE_NO_PAD.encode(sparse_pq_ratchet.as_slice()),
            ),
            skipped_keys: self
                .skipped_keys
                .iter()
                .map(PersistedSkippedMessageKeySecret::from)
                .collect(),
        })
    }

    pub(in crate::core::secure_mesh_pairwise) fn from_persisted_snapshots(
        public: PersistedPairwisePublicSession,
        secrets: PersistedPairwiseSessionSecrets,
    ) -> Result<Self> {
        ensure!(
            public.schema_version == PAIRWISE_SNAPSHOT_SCHEMA_VERSION
                && secrets.schema_version == PAIRWISE_SNAPSHOT_SCHEMA_VERSION,
            "secure mesh pairwise persisted snapshot schema is unsupported"
        );
        ensure!(
            public.secret_store_class == PAIRWISE_SECRET_STORE_CLASS,
            "secure mesh pairwise persisted snapshot secret class is unsupported"
        );
        ensure!(
            public.state_version == secrets.state_version
                && public.session_id == secrets.session_id
                && public.local_endpoint_id == secrets.local_endpoint_id
                && public.remote_endpoint_id == secrets.remote_endpoint_id,
            "secure mesh pairwise persisted public and secret snapshots are not bound"
        );
        ensure!(
            public.skipped_keys.len() == secrets.skipped_keys.len(),
            "secure mesh pairwise persisted skipped-key snapshot is inconsistent"
        );
        ensure!(
            secrets.skipped_receiving_header_keys.len() <= MAX_SKIPPED_KEYS,
            "secure mesh pairwise persisted skipped header-key limit exceeded"
        );
        validate_endpoint_id(&public.local_endpoint_id)?;
        validate_endpoint_id(&public.remote_endpoint_id)?;
        let _secret_store_key = require_text(public.secret_store_key, "secret store key")?;
        let _secret_store_namespace =
            require_text(public.secret_store_namespace, "secret store namespace")?;
        let local_ratchet_secret = SecureMeshPairwisePrivateKey::from_bytes(decode_secret_32(
            secrets.local_ratchet_secret.as_str(),
        )?);
        let role = SecureMeshPairwiseRole::from_str(&public.role)?;
        let handshake_transcript_hash = decode_fixed_base64url::<HANDSHAKE_HASH_LEN>(
            &public.handshake_transcript_hash,
            "persisted handshake transcript hash",
        )?;
        let capability_negotiation = match (public.capability_binding, public.capability_projection)
        {
            (Some(binding), Some(projection)) => {
                let base_transcript_digest =
                    crate::core::secure_mesh_capability_proof::encode_sha256_digest(
                        &handshake_transcript_hash,
                    );
                ensure!(
                    binding.base_transcript_digest == base_transcript_digest,
                    "secure mesh pairwise persisted capability transcript mismatch"
                );
                let local_proof_digest =
                    signed_capability_proof_digest(&public.local_capability_proof)?;
                ensure!(
                    binding
                        .capability_proof_digests
                        .contains(&local_proof_digest),
                    "secure mesh pairwise persisted local capability proof is unbound"
                );
                Some(restore_verified_pairwise_session_negotiation(
                    binding, projection,
                )?)
            }
            (None, None) => None,
            _ => {
                return Err(anyhow!(
                    "secure mesh pairwise persisted capability negotiation is incomplete"
                ));
            }
        };
        ensure!(
            public.revoked
                || ((role == SecureMeshPairwiseRole::Initiator
                    || capability_negotiation.is_some())
                    && (!public.initiator_key_confirmed || capability_negotiation.is_some())),
            "secure mesh pairwise persisted protected session lacks capability negotiation"
        );
        ensure!(
            public.receiving_ratchet_epoch <= public.dh_epoch
                && public.dh_epoch - public.receiving_ratchet_epoch <= 1,
            "secure mesh pairwise persisted ratchet epochs are inconsistent"
        );
        ensure!(
            secrets.sparse_pq_ratchet.as_str().len() <= MAX_ENCODED_SPARSE_PQ_RATCHET_BYTES,
            "secure mesh pairwise persisted sparse PQ ratchet exceeds the resource limit"
        );
        let sparse_pq_ratchet_bytes = Zeroizing::new(
            general_purpose::URL_SAFE_NO_PAD
                .decode(secrets.sparse_pq_ratchet.as_str())
                .context("secure mesh pairwise persisted sparse PQ ratchet is not base64url")?,
        );
        let canonical_sparse_pq_ratchet = Zeroizing::new(
            general_purpose::URL_SAFE_NO_PAD.encode(sparse_pq_ratchet_bytes.as_slice()),
        );
        ensure!(
            canonical_sparse_pq_ratchet.as_str() == secrets.sparse_pq_ratchet.as_str(),
            "secure mesh pairwise persisted sparse PQ ratchet encoding is non-canonical"
        );
        let sparse_pq_ratchet =
            SecureMeshSparsePqRatchet::restore(sparse_pq_ratchet_bytes.as_slice())?;
        Ok(Self {
            session_id: require_text(public.session_id, "session id")?,
            local_endpoint_id: public.local_endpoint_id,
            remote_endpoint_id: public.remote_endpoint_id,
            role,
            root_key: Zeroizing::new(decode_secret_32(secrets.root_key.as_str())?),
            sending_chain_key: Zeroizing::new(decode_secret_32(
                secrets.sending_chain_key.as_str(),
            )?),
            receiving_chain_key: Zeroizing::new(decode_secret_32(
                secrets.receiving_chain_key.as_str(),
            )?),
            sending_header_key: Zeroizing::new(decode_secret_32(
                secrets.sending_header_key.as_str(),
            )?),
            receiving_header_key: Zeroizing::new(decode_secret_32(
                secrets.receiving_header_key.as_str(),
            )?),
            next_sending_header_key: Zeroizing::new(decode_secret_32(
                secrets.next_sending_header_key.as_str(),
            )?),
            next_receiving_header_key: Zeroizing::new(decode_secret_32(
                secrets.next_receiving_header_key.as_str(),
            )?),
            skipped_receiving_header_keys: secrets
                .skipped_receiving_header_keys
                .iter()
                .map(|key| decode_secret_32(key.as_str()).map(Zeroizing::new))
                .collect::<Result<Vec<_>>>()?,
            local_ratchet_secret,
            local_ratchet_public_key: decode_secret_32(&public.local_ratchet_public_key)?,
            remote_ratchet_public_key: decode_secret_32(&public.remote_ratchet_public_key)?,
            handshake_transcript_hash,
            dh_epoch: public.dh_epoch,
            receiving_ratchet_epoch: public.receiving_ratchet_epoch,
            sending_chain_index: public.sending_chain_index,
            receiving_chain_index: public.receiving_chain_index,
            previous_chain_length: public.previous_chain_length,
            skipped_keys: public
                .skipped_keys
                .into_iter()
                .zip(secrets.skipped_keys.iter())
                .map(SkippedMessageKey::try_from)
                .collect::<Result<Vec<_>>>()?,
            received_message_ids: public.received_message_ids,
            pending_sending_ratchet: public.pending_sending_ratchet,
            initiator_key_confirmed: public.initiator_key_confirmed,
            local_capability_proof: public.local_capability_proof,
            capability_negotiation,
            sparse_pq_ratchet,
            revoked: public.revoked,
        })
    }
}
