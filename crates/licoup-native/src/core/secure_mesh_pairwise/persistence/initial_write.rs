use anyhow::{Context, Result, anyhow, ensure};
use rusqlite::{OptionalExtension, params};

use super::super::{key_ratchet::SecureMeshPairwiseSession, support::require_text};
use super::{
    capability_proof::{PreparedCapabilityProofPair, consume_prepared_capability_proof_pair},
    local_prekey::{PreparedLocalPreKeyUse, consume_local_prekey_use},
    remote_prekey::{PreparedRemotePreKeyUse, consume_remote_prekey_use},
    store_model::{
        SecureMeshLocalPreKeyUse, SecureMeshPairwiseDurableRecord, SecureMeshPairwiseDurableStore,
        SecureMeshRemotePreKeyUse,
    },
};
use crate::core::secure_mesh_capability_proof::SignedCapabilityProof;

impl SecureMeshPairwiseDurableStore {
    pub fn upsert_initial(
        &mut self,
        session: &SecureMeshPairwiseSession,
        updated_at: impl Into<String>,
    ) -> Result<SecureMeshPairwiseDurableRecord> {
        self.upsert_initial_with_security_claims(session, updated_at.into(), None, None, None)
    }

    pub fn upsert_initial_with_local_prekey_claim(
        &mut self,
        session: &SecureMeshPairwiseSession,
        local_prekey_use: &SecureMeshLocalPreKeyUse,
        updated_at: impl Into<String>,
    ) -> Result<SecureMeshPairwiseDurableRecord> {
        self.upsert_initial_with_security_claims(
            session,
            updated_at.into(),
            Some(local_prekey_use),
            None,
            None,
        )
    }

    pub fn upsert_initial_with_local_prekey_claim_and_capability_proofs(
        &mut self,
        session: &SecureMeshPairwiseSession,
        local_prekey_use: &SecureMeshLocalPreKeyUse,
        first_proof: &SignedCapabilityProof,
        second_proof: &SignedCapabilityProof,
        now_unix_seconds: i64,
        updated_at: impl Into<String>,
    ) -> Result<SecureMeshPairwiseDurableRecord> {
        self.upsert_initial_with_security_claims(
            session,
            updated_at.into(),
            Some(local_prekey_use),
            None,
            Some((first_proof, second_proof, now_unix_seconds)),
        )
    }

    pub fn upsert_initial_with_remote_prekey_claim(
        &mut self,
        session: &SecureMeshPairwiseSession,
        remote_prekey_use: &SecureMeshRemotePreKeyUse,
        updated_at: impl Into<String>,
    ) -> Result<SecureMeshPairwiseDurableRecord> {
        self.upsert_initial_with_security_claims(
            session,
            updated_at.into(),
            None,
            Some(remote_prekey_use),
            None,
        )
    }

    pub(super) fn upsert_initial_with_security_claims(
        &mut self,
        session: &SecureMeshPairwiseSession,
        updated_at: String,
        local_prekey_use: Option<&SecureMeshLocalPreKeyUse>,
        remote_prekey_use: Option<&SecureMeshRemotePreKeyUse>,
        capability_proofs: Option<(&SignedCapabilityProof, &SignedCapabilityProof, i64)>,
    ) -> Result<SecureMeshPairwiseDurableRecord> {
        let updated_at = require_text(updated_at.into(), "updated_at")?;
        let local_prekey_claim = local_prekey_use
            .map(|prekey_use| PreparedLocalPreKeyUse::new(prekey_use, session, updated_at.clone()))
            .transpose()?;
        let remote_prekey_claim = remote_prekey_use
            .map(|prekey_use| PreparedRemotePreKeyUse::new(prekey_use, updated_at.clone()))
            .transpose()?;
        if let Some(remote_prekey_claim) = &remote_prekey_claim {
            ensure!(
                remote_prekey_claim.session_id == session.session_id
                    && remote_prekey_claim.local_endpoint_id == session.local_endpoint_id
                    && remote_prekey_claim.remote_endpoint_id == session.remote_endpoint_id,
                "secure mesh pairwise remote prekey claim session binding mismatch"
            );
        }
        let capability_proofs = capability_proofs
            .map(|(first, second, now)| {
                PreparedCapabilityProofPair::new(
                    &self.secret_store_namespace,
                    &session.local_endpoint_id,
                    first,
                    second,
                    now,
                )
            })
            .transpose()?;
        let existing: Option<i64> = self
            .connection
            .query_row(
                "SELECT 1 FROM secure_mesh_pairwise_sessions WHERE session_id = ?1 AND local_endpoint_id = ?2",
                params![session.session_id, session.local_endpoint_id],
                |row| row.get(0),
            )
            .optional()
            .context("secure mesh pairwise initial durable existence check failed")?;
        ensure!(
            existing.is_none(),
            "secure mesh pairwise durable record already exists"
        );
        let pending = self.prepare_secret_bound_snapshot(session, 1)?;
        let tx = self
            .connection
            .transaction()
            .context("secure mesh pairwise initial durable transaction failed")?;
        if let Some(local_prekey_claim) = &local_prekey_claim {
            if let Err(error) = consume_local_prekey_use(&tx, local_prekey_claim) {
                drop(tx);
                self.cleanup_pending_snapshot(&pending).context(
                    "secure mesh pairwise failed initial snapshot cleanup is incomplete",
                )?;
                return Err(error);
            }
        }
        if let Some(remote_prekey_claim) = &remote_prekey_claim {
            if let Err(error) = consume_remote_prekey_use(&tx, remote_prekey_claim) {
                drop(tx);
                self.cleanup_pending_snapshot(&pending).context(
                    "secure mesh pairwise failed remote prekey snapshot cleanup is incomplete",
                )?;
                return Err(error);
            }
        }
        if let Some(capability_proofs) = &capability_proofs {
            if let Err(error) = consume_prepared_capability_proof_pair(&tx, capability_proofs) {
                drop(tx);
                self.cleanup_pending_snapshot(&pending).context(
                    "secure mesh pairwise failed capability snapshot cleanup is incomplete",
                )?;
                return Err(error);
            }
        }
        let insert_result = tx.execute(
            r#"
            INSERT INTO secure_mesh_pairwise_sessions (
                session_id,
                local_endpoint_id,
                remote_endpoint_id,
                state_version,
                dh_epoch,
                sent_count,
                received_count,
                revoked_at,
                snapshot_json,
                updated_at
            ) VALUES (?1, ?2, ?3, 1, ?4, ?5, ?6, NULL, ?7, ?8)
            "#,
            params![
                session.session_id,
                session.local_endpoint_id,
                session.remote_endpoint_id,
                session.dh_epoch as i64,
                session.sending_chain_index as i64,
                session.receiving_chain_index as i64,
                pending.public_json,
                updated_at
            ],
        );
        if let Err(error) = insert_result {
            drop(tx);
            self.cleanup_pending_snapshot(&pending).context(
                "secure mesh pairwise failed initial insert snapshot cleanup is incomplete",
            )?;
            return Err(error).context("secure mesh pairwise initial durable insert failed");
        }
        if let Err(error) = tx.commit() {
            self.cleanup_pending_snapshot(&pending).context(
                "secure mesh pairwise failed initial commit snapshot cleanup is incomplete",
            )?;
            return Err(error).context("secure mesh pairwise initial durable commit failed");
        }
        self.read_record(&session.session_id, &session.local_endpoint_id)?
            .ok_or_else(|| anyhow!("secure mesh pairwise durable record disappeared after insert"))
    }
}
