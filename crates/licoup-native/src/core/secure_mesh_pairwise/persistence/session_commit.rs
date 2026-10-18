use anyhow::{Context, Result, anyhow, ensure};
use rusqlite::params;

use super::super::{key_ratchet::SecureMeshPairwiseSession, support::require_text};
use super::secret_snapshot::PendingPairwiseSnapshot;
use super::{
    capability_proof::{PreparedCapabilityProofPair, consume_prepared_capability_proof_pair},
    pending_delivery::insert_pending_delivery,
    received_payload::{
        PreparedReceivedPayload, cleanup_prepared_received_payload, insert_received_payload,
    },
    restoration_validation::{replay_window_preserved, skipped_keys_not_reintroduced},
    store_model::{
        SecureMeshPairwiseDurableRecord, SecureMeshPairwiseDurableStore,
        SecureMeshPairwisePendingDelivery, SecureMeshPairwiseReceivedPayload,
    },
};
use crate::core::secure_mesh_capability_proof::SignedCapabilityProof;
use crate::core::secure_mesh_secret_store::SecretStoreAuthorizationSession;

impl SecureMeshPairwiseDurableStore {
    pub fn commit_session(
        &mut self,
        previous: &SecureMeshPairwiseDurableRecord,
        session: &SecureMeshPairwiseSession,
        updated_at: impl Into<String>,
    ) -> Result<SecureMeshPairwiseDurableRecord> {
        self.commit_session_with_optional_authorization(
            previous,
            session,
            updated_at.into(),
            None,
            None,
            None,
            None,
        )
    }

    pub fn commit_session_with_authorized_session(
        &mut self,
        previous: &SecureMeshPairwiseDurableRecord,
        session: &SecureMeshPairwiseSession,
        updated_at: impl Into<String>,
        secret_store_session: &SecretStoreAuthorizationSession,
    ) -> Result<SecureMeshPairwiseDurableRecord> {
        self.commit_session_with_optional_authorization(
            previous,
            session,
            updated_at.into(),
            Some(secret_store_session),
            None,
            None,
            None,
        )
    }

    pub fn commit_session_with_authorized_session_and_pending_delivery(
        &mut self,
        previous: &SecureMeshPairwiseDurableRecord,
        session: &SecureMeshPairwiseSession,
        pending_delivery: &SecureMeshPairwisePendingDelivery,
        updated_at: impl Into<String>,
        secret_store_session: &SecretStoreAuthorizationSession,
    ) -> Result<SecureMeshPairwiseDurableRecord> {
        self.commit_session_with_optional_authorization(
            previous,
            session,
            updated_at.into(),
            Some(secret_store_session),
            None,
            Some(pending_delivery),
            None,
        )
    }

    pub fn commit_session_with_authorized_session_and_received_payload(
        &mut self,
        previous: &SecureMeshPairwiseDurableRecord,
        session: &SecureMeshPairwiseSession,
        received_payload: &SecureMeshPairwiseReceivedPayload,
        updated_at: impl Into<String>,
        secret_store_session: &SecretStoreAuthorizationSession,
    ) -> Result<SecureMeshPairwiseDurableRecord> {
        self.commit_session_with_optional_authorization(
            previous,
            session,
            updated_at.into(),
            Some(secret_store_session),
            None,
            None,
            Some(received_payload),
        )
    }

    pub fn commit_session_with_authorized_session_and_capability_proofs(
        &mut self,
        previous: &SecureMeshPairwiseDurableRecord,
        session: &SecureMeshPairwiseSession,
        first_proof: &SignedCapabilityProof,
        second_proof: &SignedCapabilityProof,
        now_unix_seconds: i64,
        updated_at: impl Into<String>,
        secret_store_session: &SecretStoreAuthorizationSession,
    ) -> Result<SecureMeshPairwiseDurableRecord> {
        self.commit_session_with_optional_authorization(
            previous,
            session,
            updated_at.into(),
            Some(secret_store_session),
            Some((first_proof, second_proof, now_unix_seconds)),
            None,
            None,
        )
    }

    pub(super) fn commit_session_with_optional_authorization(
        &mut self,
        previous: &SecureMeshPairwiseDurableRecord,
        session: &SecureMeshPairwiseSession,
        updated_at: String,
        secret_store_session: Option<&SecretStoreAuthorizationSession>,
        capability_proofs: Option<(&SignedCapabilityProof, &SignedCapabilityProof, i64)>,
        pending_delivery: Option<&SecureMeshPairwisePendingDelivery>,
        received_payload: Option<&SecureMeshPairwiseReceivedPayload>,
    ) -> Result<SecureMeshPairwiseDurableRecord> {
        ensure!(
            previous.session_id == session.session_id
                && previous.local_endpoint_id == session.local_endpoint_id,
            "secure mesh pairwise durable commit subject mismatch"
        );
        ensure!(
            previous.revoked_at.is_none(),
            "secure mesh pairwise durable session is revoked"
        );
        ensure!(
            session.dh_epoch >= previous.dh_epoch,
            "secure mesh pairwise durable rollback detected"
        );
        let previous_public = self
            .read_public_snapshot(&previous.session_id, &previous.local_endpoint_id)?
            .ok_or_else(|| anyhow!("secure mesh pairwise durable previous snapshot is missing"))?;
        ensure!(
            session.dh_epoch > previous.dh_epoch
                || (session.sending_chain_index >= previous.sent_count
                    && session.receiving_chain_index >= previous.received_count),
            "secure mesh pairwise durable state regression detected"
        );
        let received_advance = session
            .receiving_chain_index
            .saturating_sub(previous.received_count);
        ensure!(
            replay_window_preserved(
                &previous_public.received_message_ids,
                &session.received_message_ids,
                received_advance,
            ),
            "secure mesh pairwise durable replay cache rollback detected"
        );
        ensure!(
            skipped_keys_not_reintroduced(&previous_public.skipped_keys, session, previous),
            "secure mesh pairwise durable skipped-key rollback detected"
        );
        ensure!(
            !previous_public.initiator_key_confirmed || session.initiator_key_confirmed,
            "secure mesh pairwise durable handshake confirmation rollback detected"
        );
        ensure!(
            previous_public.capability_binding.is_none()
                || session.capability_negotiation.is_some(),
            "secure mesh pairwise durable capability negotiation rollback detected"
        );
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
        let updated_at = require_text(updated_at, "updated_at")?;
        let previous_secret_handle = self.secret_snapshot_handle(
            &previous_public.secret_store_namespace,
            &previous_public.secret_store_key,
        )?;
        let pending = self.prepare_secret_bound_snapshot_with_optional_authorization(
            session,
            previous.state_version + 1,
            secret_store_session,
        )?;
        let prepared_received = match received_payload {
            Some(received_payload) => {
                let authorization = secret_store_session.ok_or_else(|| {
                    anyhow!("secure mesh received payload commit requires authorization")
                })?;
                match self.prepare_received_payload(
                    &session.session_id,
                    &session.local_endpoint_id,
                    received_payload,
                    authorization,
                ) {
                    Ok(prepared) => Some(prepared),
                    Err(error) => {
                        self.cleanup_pending_snapshot(&pending).context(
                            "secure mesh received payload preparation snapshot cleanup is incomplete",
                        )?;
                        return Err(error);
                    }
                }
            }
            None => None,
        };
        let tx = self
            .connection
            .transaction()
            .context("secure mesh pairwise durable commit transaction failed")?;
        let changed = tx.execute(
            r#"
            UPDATE secure_mesh_pairwise_sessions
            SET remote_endpoint_id = ?1,
                state_version = state_version + 1,
                dh_epoch = ?2,
                sent_count = ?3,
                received_count = ?4,
                snapshot_json = ?5,
                updated_at = ?6
            WHERE session_id = ?7
              AND local_endpoint_id = ?8
              AND state_version = ?9
              AND revoked_at IS NULL
            "#,
            params![
                session.remote_endpoint_id,
                session.dh_epoch as i64,
                session.sending_chain_index as i64,
                session.receiving_chain_index as i64,
                pending.public_json,
                updated_at,
                previous.session_id,
                previous.local_endpoint_id,
                previous.state_version as i64
            ],
        );
        let changed = match changed {
            Ok(changed) => changed,
            Err(error) => {
                drop(tx);
                self.cleanup_failed_commit_material(&pending, prepared_received.as_ref())
                    .context("secure mesh pairwise failed update snapshot cleanup is incomplete")?;
                return Err(error).context("secure mesh pairwise durable update failed");
            }
        };
        if changed != 1 {
            drop(tx);
            self.cleanup_failed_commit_material(&pending, prepared_received.as_ref())
                .context("secure mesh pairwise rejected update snapshot cleanup is incomplete")?;
            return Err(anyhow!(
                "secure mesh pairwise durable compare-and-swap failed"
            ));
        }
        if let Some(capability_proofs) = &capability_proofs {
            if let Err(error) = consume_prepared_capability_proof_pair(&tx, capability_proofs) {
                drop(tx);
                self.cleanup_failed_commit_material(&pending, prepared_received.as_ref())
                    .context(
                        "secure mesh pairwise failed proof update snapshot cleanup is incomplete",
                    )?;
                return Err(error);
            }
        }
        if let Some(pending_delivery) = pending_delivery {
            if let Err(error) = insert_pending_delivery(
                &tx,
                &session.session_id,
                &session.local_endpoint_id,
                pending_delivery,
            ) {
                drop(tx);
                self.cleanup_failed_commit_material(&pending, prepared_received.as_ref()).context(
                    "secure mesh pairwise failed delivery update snapshot cleanup is incomplete",
                )?;
                return Err(error);
            }
        }
        if let Some(prepared_received) = &prepared_received
            && let Err(error) = insert_received_payload(
                &tx,
                &session.session_id,
                &session.local_endpoint_id,
                prepared_received,
            )
        {
            drop(tx);
            self.cleanup_failed_commit_material(&pending, Some(prepared_received))
                .context(
                    "secure mesh pairwise failed received payload snapshot cleanup is incomplete",
                )?;
            return Err(error);
        }
        if let Err(error) = tx.commit() {
            self.cleanup_failed_commit_material(&pending, prepared_received.as_ref())
                .context("secure mesh pairwise failed commit snapshot cleanup is incomplete")?;
            return Err(error).context("secure mesh pairwise durable commit failed");
        }
        if previous_secret_handle != pending.secret_handle {
            self.delete_secret_or_enqueue_cleanup(
                &pending.secret_store_session,
                &previous_secret_handle,
            )
            .context("secure mesh pairwise superseded secret cleanup is incomplete")?;
        }
        self.read_record(&session.session_id, &session.local_endpoint_id)?
            .ok_or_else(|| anyhow!("secure mesh pairwise durable record disappeared after commit"))
    }

    fn cleanup_failed_commit_material(
        &self,
        pending: &PendingPairwiseSnapshot,
        received: Option<&PreparedReceivedPayload>,
    ) -> Result<()> {
        let received_cleanup = received
            .map(|received| cleanup_prepared_received_payload(self, received))
            .unwrap_or(Ok(()));
        let snapshot_cleanup = self.cleanup_pending_snapshot(pending);
        received_cleanup.and(snapshot_cleanup)
    }
}
