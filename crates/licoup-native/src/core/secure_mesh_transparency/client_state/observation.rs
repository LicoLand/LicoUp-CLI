//! Signed-tree-head observation and gossip validation flows.

use anyhow::{Result, anyhow, bail, ensure};
use rusqlite::TransactionBehavior;

use super::super::constants::SECURE_MESH_KT_GOSSIP_CONTENT_TYPE;
use super::super::model::{
    SecureMeshKtCachedCheckpoint, SecureMeshKtConsistencyProof, SecureMeshKtGossipPayload,
};
use super::super::persistence::{
    CheckpointTransition, advance_checkpoint_transaction, advance_durable_time_watermark,
    latest_checkpoint_connection, persist_gossip_observation_transaction,
    verify_authenticated_sth_freshness_or_block,
};
use super::super::signature::SecureMeshSignedTreeHead;
use super::SecureMeshKtClientState;

impl SecureMeshKtClientState {
    /// Verify and atomically persist an STH learned through gossip/monitoring.
    pub fn observe_peer_gossip_sth(
        &mut self,
        gossip: &SecureMeshKtGossipPayload,
        now_epoch_seconds: u64,
    ) -> Result<SecureMeshKtCachedCheckpoint> {
        ensure!(
            gossip.content_type == SECURE_MESH_KT_GOSSIP_CONTENT_TYPE,
            "secure mesh KT gossip content type is unsupported"
        );
        let checkpoint = self.observe_tree_head(
            &gossip.signed_tree_head,
            gossip.consistency_proof.as_ref(),
            now_epoch_seconds,
        )?;
        let pin = self.pin()?.clone();
        let effective_now_epoch_seconds =
            advance_durable_time_watermark(&mut self.connection, now_epoch_seconds)?;
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        persist_gossip_observation_transaction(
            &transaction,
            &pin,
            &gossip.signed_tree_head,
            effective_now_epoch_seconds,
        )?;
        transaction.commit()?;
        Ok(checkpoint)
    }

    /// Verify an outgoing gossip payload against the already accepted local checkpoint without
    /// counting the local echo as independent peer/witness evidence.
    pub fn validate_outgoing_gossip_sth(
        &mut self,
        gossip: &SecureMeshKtGossipPayload,
        now_epoch_seconds: u64,
    ) -> Result<SecureMeshKtCachedCheckpoint> {
        ensure!(
            gossip.content_type == SECURE_MESH_KT_GOSSIP_CONTENT_TYPE,
            "secure mesh KT gossip content type is unsupported"
        );
        ensure!(
            gossip.consistency_proof.is_none(),
            "secure mesh KT outgoing current-checkpoint gossip must not carry a transition proof"
        );
        let pin = self.pin()?.clone();
        let effective_now_epoch_seconds =
            advance_durable_time_watermark(&mut self.connection, now_epoch_seconds)?;
        verify_authenticated_sth_freshness_or_block(
            &mut self.connection,
            &pin,
            self.freshness_policy,
            &gossip.signed_tree_head,
            effective_now_epoch_seconds,
        )?;
        let latest = latest_checkpoint_connection(&self.connection, pin.log_id())?
            .ok_or_else(|| anyhow!("secure mesh KT outgoing gossip checkpoint is unavailable"))?;
        ensure!(
            latest.tree_size == gossip.signed_tree_head.tree_size
                && latest.root_hash == gossip.signed_tree_head.root_hash
                && latest.map_root_hash == gossip.signed_tree_head.map_root_hash
                && latest.issued_at_epoch_seconds
                    == gossip.signed_tree_head.issued_at_epoch_seconds,
            "secure mesh KT outgoing gossip does not match the accepted local checkpoint"
        );
        Ok(latest)
    }

    pub fn observe_tree_head(
        &mut self,
        sth: &SecureMeshSignedTreeHead,
        consistency: Option<&SecureMeshKtConsistencyProof>,
        now_epoch_seconds: u64,
    ) -> Result<SecureMeshKtCachedCheckpoint> {
        let pin = self.pin()?.clone();
        let effective_now_epoch_seconds =
            advance_durable_time_watermark(&mut self.connection, now_epoch_seconds)?;
        verify_authenticated_sth_freshness_or_block(
            &mut self.connection,
            &pin,
            self.freshness_policy,
            sth,
            effective_now_epoch_seconds,
        )?;
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let transition = advance_checkpoint_transaction(
            &transaction,
            &pin,
            self.freshness_policy,
            sth,
            consistency,
            effective_now_epoch_seconds,
        )?;
        match transition {
            CheckpointTransition::Accepted(checkpoint) => {
                transaction.commit()?;
                Ok(checkpoint)
            }
            CheckpointTransition::SecurityBlock(reason) => {
                transaction.commit()?;
                bail!("secure mesh KT security block persisted: {reason}")
            }
        }
    }
}
