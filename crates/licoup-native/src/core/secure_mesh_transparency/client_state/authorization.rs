//! Purpose-bound directory inclusion and absence authorization flows.

use anyhow::{Result, anyhow, bail, ensure};
use rusqlite::{OptionalExtension, TransactionBehavior, params};

use super::super::json_codec::{hex_encode, validate_hex_hash, validate_text};
use super::super::model::{
    DirectoryComponentCommitments, SecureMeshKtAuthorizationReceipt, SecureMeshKtCachedCheckpoint,
    SecureMeshKtConsistencyProof, SecureMeshKtInclusionProof, SecureMeshKtMapProof,
};
use super::super::persistence::{
    CheckpointTransition, advance_checkpoint_transaction, advance_durable_time_watermark,
    authenticated_sth_temporal_block_reason, enforce_directory_latest_transaction,
    latest_checkpoint_connection, persist_directory_authorization_transaction,
    persist_security_block, persist_security_block_connection,
    require_fresh_gossip_checkpoint_transaction, require_fresh_gossip_observation_transaction,
    sql_to_u64, verify_authenticated_sth_freshness_or_block,
};
use super::super::proofs::{map_root_log_leaf_hash, verify_kt_inclusion};
use super::super::signature::VerifiedKtFreshness;
use super::super::sparse_map::{verify_kt_map_inclusion, verify_kt_non_inclusion};
use super::SecureMeshKtClientState;

impl SecureMeshKtClientState {
    pub(crate) fn authorize_hashed_directory_view(
        &mut self,
        stable_label: &str,
        purpose: &str,
        version: u64,
        revoked: bool,
        expected_leaf_hash: &str,
        components: DirectoryComponentCommitments<'_>,
        inclusion: &SecureMeshKtInclusionProof,
        map_proof: &SecureMeshKtMapProof,
        consistency: Option<&SecureMeshKtConsistencyProof>,
        now_epoch_seconds: u64,
    ) -> Result<(SecureMeshKtCachedCheckpoint, VerifiedKtFreshness)> {
        validate_hex_hash("stable_label", stable_label)?;
        validate_text("authorization_purpose", purpose)?;
        validate_hex_hash("expected_leaf_hash", expected_leaf_hash)?;
        validate_text("identity_fingerprint", components.identity_fingerprint)?;
        validate_hex_hash("identity_key_digest", components.identity_key_digest)?;
        validate_hex_hash("signed_prekey_digest", components.signed_prekey_digest)?;
        validate_hex_hash("one_time_prekey_digest", components.one_time_prekey_digest)?;
        validate_hex_hash("mls_key_package_digest", components.mls_key_package_digest)?;
        let pin = self.pin()?.clone();
        let effective_now_epoch_seconds =
            advance_durable_time_watermark(&mut self.connection, now_epoch_seconds)?;
        verify_authenticated_sth_freshness_or_block(
            &mut self.connection,
            &pin,
            self.freshness_policy,
            &inclusion.signed_tree_head,
            effective_now_epoch_seconds,
        )?;
        let freshness = verify_kt_inclusion(
            inclusion,
            &pin,
            self.freshness_policy,
            effective_now_epoch_seconds,
        )?;
        let expected_map_root_log_leaf = hex_encode(&map_root_log_leaf_hash(
            &inclusion.signed_tree_head.map_root_hash,
        )?);
        ensure!(
            inclusion.leaf_hash == expected_map_root_log_leaf,
            "secure mesh KT append-log inclusion does not commit the authenticated map root"
        );
        ensure!(
            inclusion.signed_tree_head == map_proof.signed_tree_head,
            "secure mesh KT log and map proofs do not share one signed tree head"
        );
        verify_kt_map_inclusion(
            map_proof,
            stable_label,
            expected_leaf_hash,
            version,
            revoked,
            &pin,
            self.freshness_policy,
            effective_now_epoch_seconds,
        )?;

        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let checkpoint = match advance_checkpoint_transaction(
            &transaction,
            &pin,
            self.freshness_policy,
            &inclusion.signed_tree_head,
            consistency,
            effective_now_epoch_seconds,
        )? {
            CheckpointTransition::Accepted(checkpoint) => checkpoint,
            CheckpointTransition::SecurityBlock(reason) => {
                transaction.commit()?;
                bail!("secure mesh KT security block persisted: {reason}")
            }
        };
        require_fresh_gossip_observation_transaction(
            &transaction,
            &pin,
            &inclusion.signed_tree_head,
            self.freshness_policy,
            effective_now_epoch_seconds,
        )?;
        if let Some(reason) = enforce_directory_latest_transaction(
            &transaction,
            &pin,
            stable_label,
            version,
            expected_leaf_hash,
            revoked,
            &components,
            checkpoint.tree_size,
        )? {
            transaction.commit()?;
            bail!("secure mesh KT security block persisted: {reason}")
        }
        persist_directory_authorization_transaction(
            &transaction,
            &pin,
            stable_label,
            purpose,
            version,
            expected_leaf_hash,
            revoked,
            inclusion,
            map_proof,
            &freshness,
        )?;
        transaction.commit()?;
        Ok((checkpoint, freshness))
    }

    pub fn require_current_directory_authorization(
        &mut self,
        stable_label: &str,
        purpose: &str,
        now_epoch_seconds: u64,
    ) -> Result<SecureMeshKtAuthorizationReceipt> {
        validate_hex_hash("stable_label", stable_label)?;
        validate_text("authorization_purpose", purpose)?;
        let pin = self.pin()?.clone();
        let effective_now_epoch_seconds =
            advance_durable_time_watermark(&mut self.connection, now_epoch_seconds)?;
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Deferred)?;
        let blocked = transaction.query_row(
            "SELECT blocked FROM secure_mesh_kt_guard WHERE singleton = 1",
            [],
            |row| row.get::<_, i64>(0),
        )? != 0;
        ensure!(
            !blocked,
            "secure mesh KT equivocation was previously persisted; authorization is blocked"
        );
        let latest = latest_checkpoint_connection(&transaction, pin.log_id())?
            .ok_or_else(|| anyhow!("secure mesh KT current checkpoint is unavailable"))?;
        let persisted = transaction
            .query_row(
                "SELECT directory_version, leaf_hash, revoked, tree_size, root_hash, map_root_hash, issued_at_epoch_seconds, observed_at_epoch_seconds, inclusion_json, map_proof_json
                 FROM secure_mesh_kt_directory_authorizations
                 WHERE log_id = ?1 AND stable_label = ?2 AND purpose = ?3",
                params![pin.log_id(), stable_label, purpose],
                |row| {
                    Ok((
                        row.get::<_, i64>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, i64>(2)? != 0,
                        row.get::<_, i64>(3)?,
                        row.get::<_, String>(4)?,
                        row.get::<_, String>(5)?,
                        row.get::<_, i64>(6)?,
                        row.get::<_, i64>(7)?,
                        row.get::<_, String>(8)?,
                        row.get::<_, String>(9)?,
                    ))
                },
            )
            .optional()?
            .ok_or_else(|| anyhow!("secure mesh KT purpose-bound authorization is missing"))?;
        let directory_version = sql_to_u64(persisted.0, "directory version")?;
        let tree_size = sql_to_u64(persisted.3, "authorization tree size")?;
        let issued_at_epoch_seconds = sql_to_u64(persisted.6, "authorization issue time")?;
        let observed_at_epoch_seconds = sql_to_u64(persisted.7, "authorization observation time")?;
        let inclusion: SecureMeshKtInclusionProof = serde_json::from_str(&persisted.8)
            .map_err(|_| anyhow!("secure mesh KT persisted inclusion proof is invalid"))?;
        let map_proof: SecureMeshKtMapProof = serde_json::from_str(&persisted.9)
            .map_err(|_| anyhow!("secure mesh KT persisted map proof is invalid"))?;

        ensure!(
            tree_size == latest.tree_size
                && persisted.4 == latest.root_hash
                && persisted.5 == latest.map_root_hash,
            "secure mesh KT label authorization does not match the current accepted checkpoint"
        );
        let latest_directory = transaction
            .query_row(
                "SELECT version, leaf_hash, revoked, tree_size,
                        identity_fingerprint, identity_rotation_epoch, identity_key_digest,
                        pairwise_prekey_version, signed_prekey_digest, one_time_prekey_digest,
                        mls_key_package_version, mls_key_package_digest
                 FROM secure_mesh_kt_directory_latest
                 WHERE log_id = ?1 AND stable_label = ?2",
                params![pin.log_id(), stable_label],
                |row| {
                    Ok((
                        row.get::<_, i64>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, i64>(2)? != 0,
                        row.get::<_, i64>(3)?,
                        row.get::<_, String>(4)?,
                        row.get::<_, i64>(5)?,
                        row.get::<_, String>(6)?,
                        row.get::<_, i64>(7)?,
                        row.get::<_, String>(8)?,
                        row.get::<_, String>(9)?,
                        row.get::<_, i64>(10)?,
                        row.get::<_, String>(11)?,
                    ))
                },
            )
            .optional()?
            .ok_or_else(|| anyhow!("secure mesh KT directory label is unavailable"))?;
        ensure!(
            sql_to_u64(latest_directory.0, "directory version")? == directory_version
                && latest_directory.1 == persisted.1
                && latest_directory.2 == persisted.2
                && sql_to_u64(latest_directory.3, "directory tree size")? == tree_size,
            "secure mesh KT purpose authorization is not bound to the latest directory claim"
        );
        ensure!(
            inclusion.signed_tree_head == map_proof.signed_tree_head
                && inclusion.signed_tree_head.tree_size == tree_size
                && inclusion.signed_tree_head.root_hash == persisted.4
                && inclusion.signed_tree_head.map_root_hash == persisted.5
                && inclusion.signed_tree_head.issued_at_epoch_seconds == issued_at_epoch_seconds,
            "secure mesh KT persisted authorization STH binding is invalid"
        );
        inclusion.signed_tree_head.verify_authenticity(&pin)?;
        if let Some(reason) = authenticated_sth_temporal_block_reason(
            &inclusion.signed_tree_head,
            self.freshness_policy,
            effective_now_epoch_seconds,
        ) {
            drop(transaction);
            persist_security_block_connection(&mut self.connection, reason)?;
            bail!("secure mesh KT terminal freshness block persisted: {reason}");
        }
        let freshness = verify_kt_inclusion(
            &inclusion,
            &pin,
            self.freshness_policy,
            effective_now_epoch_seconds,
        )?;
        ensure!(
            inclusion.leaf_hash
                == hex_encode(&map_root_log_leaf_hash(
                    &inclusion.signed_tree_head.map_root_hash,
                )?),
            "secure mesh KT persisted append-log proof does not commit the map root"
        );
        verify_kt_map_inclusion(
            &map_proof,
            stable_label,
            &persisted.1,
            directory_version,
            persisted.2,
            &pin,
            self.freshness_policy,
            effective_now_epoch_seconds,
        )?;
        ensure!(
            observed_at_epoch_seconds
                <= effective_now_epoch_seconds
                    .saturating_add(self.freshness_policy.max_future_skew_seconds)
                && effective_now_epoch_seconds
                    <= observed_at_epoch_seconds
                        .saturating_add(self.freshness_policy.max_sth_age_seconds),
            "secure mesh KT purpose authorization observation is stale or from the future"
        );
        ensure!(
            freshness.issued_at_epoch_seconds == issued_at_epoch_seconds,
            "secure mesh KT persisted authorization freshness binding is invalid"
        );
        require_fresh_gossip_checkpoint_transaction(
            &transaction,
            &pin,
            &latest,
            self.freshness_policy,
            effective_now_epoch_seconds,
        )?;
        transaction.commit()?;
        Ok(SecureMeshKtAuthorizationReceipt {
            stable_label: stable_label.to_string(),
            purpose: purpose.to_string(),
            directory_version,
            leaf_hash: persisted.1,
            revoked: persisted.2,
            tree_size,
            root_hash: persisted.4,
            map_root_hash: persisted.5,
            issued_at_epoch_seconds,
            observed_at_epoch_seconds,
            validated_at_epoch_seconds: effective_now_epoch_seconds,
            expires_at_epoch_seconds: issued_at_epoch_seconds
                .saturating_add(self.freshness_policy.max_sth_age_seconds),
            identity_fingerprint: latest_directory.4,
            identity_rotation_epoch: sql_to_u64(
                latest_directory.5,
                "directory identity rotation epoch",
            )?,
            identity_key_digest: latest_directory.6,
            pairwise_prekey_version: sql_to_u64(
                latest_directory.7,
                "directory pairwise prekey version",
            )?,
            signed_prekey_digest: latest_directory.8,
            one_time_prekey_digest: latest_directory.9,
            mls_key_package_version: sql_to_u64(
                latest_directory.10,
                "directory MLS KeyPackage version",
            )?,
            mls_key_package_digest: latest_directory.11,
        })
    }

    pub(crate) fn authorize_absence_view(
        &mut self,
        stable_label: &str,
        map_root_inclusion: &SecureMeshKtInclusionProof,
        map_proof: &SecureMeshKtMapProof,
        consistency: Option<&SecureMeshKtConsistencyProof>,
        now_epoch_seconds: u64,
    ) -> Result<(SecureMeshKtCachedCheckpoint, VerifiedKtFreshness)> {
        let pin = self.pin()?.clone();
        let effective_now_epoch_seconds =
            advance_durable_time_watermark(&mut self.connection, now_epoch_seconds)?;
        verify_authenticated_sth_freshness_or_block(
            &mut self.connection,
            &pin,
            self.freshness_policy,
            &map_root_inclusion.signed_tree_head,
            effective_now_epoch_seconds,
        )?;
        verify_kt_inclusion(
            map_root_inclusion,
            &pin,
            self.freshness_policy,
            effective_now_epoch_seconds,
        )?;
        ensure!(
            map_root_inclusion.signed_tree_head == map_proof.signed_tree_head,
            "secure mesh KT absence log and map proofs do not share one signed tree head"
        );
        ensure!(
            map_root_inclusion.leaf_hash
                == hex_encode(&map_root_log_leaf_hash(
                    &map_proof.signed_tree_head.map_root_hash,
                )?),
            "secure mesh KT absence append-log inclusion does not commit the authenticated map root"
        );
        let freshness = verify_kt_non_inclusion(
            map_proof,
            stable_label,
            &pin,
            self.freshness_policy,
            effective_now_epoch_seconds,
        )?;
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let checkpoint = match advance_checkpoint_transaction(
            &transaction,
            &pin,
            self.freshness_policy,
            &map_proof.signed_tree_head,
            consistency,
            effective_now_epoch_seconds,
        )? {
            CheckpointTransition::Accepted(checkpoint) => checkpoint,
            CheckpointTransition::SecurityBlock(reason) => {
                transaction.commit()?;
                bail!("secure mesh KT security block persisted: {reason}")
            }
        };
        require_fresh_gossip_observation_transaction(
            &transaction,
            &pin,
            &map_proof.signed_tree_head,
            self.freshness_policy,
            effective_now_epoch_seconds,
        )?;
        let previously_present = transaction
            .query_row(
                "SELECT 1 FROM secure_mesh_kt_directory_latest WHERE log_id = ?1 AND stable_label = ?2",
                params![pin.log_id(), stable_label],
                |_| Ok(()),
            )
            .optional()?
            .is_some();
        if previously_present {
            persist_security_block(&transaction, "directory_present_to_absent")?;
            transaction.commit()?;
            bail!(
                "secure mesh KT security block persisted: previously present directory label became absent"
            )
        }
        transaction.commit()?;
        Ok((checkpoint, freshness))
    }
}
