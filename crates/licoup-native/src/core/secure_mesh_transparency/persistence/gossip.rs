//! Bounded peer-gossip observations and freshness binding.

use anyhow::{Result, anyhow, ensure};
use rusqlite::{OptionalExtension, Transaction, params};

use super::super::constants::{
    KT_PROTOCOL_MAX_GOSSIP_AGE_SECONDS, MAX_PERSISTED_GOSSIP_OBSERVATIONS,
};
use super::super::model::SecureMeshKtCachedCheckpoint;
use super::super::signature::{KtFreshnessPolicy, PinnedKtLogKey, SecureMeshSignedTreeHead};
use super::sql::{sql_to_u64, u64_to_sql};

pub(crate) fn persist_gossip_observation_transaction(
    transaction: &Transaction<'_>,
    pin: &PinnedKtLogKey,
    sth: &SecureMeshSignedTreeHead,
    observed_at_epoch_seconds: u64,
) -> Result<()> {
    ensure!(
        sth.log_id == pin.log_id() && sth.key_id == pin.key_id(),
        "secure mesh KT gossip observation authority binding is invalid"
    );
    transaction.execute(
        r#"
        INSERT INTO secure_mesh_kt_gossip_observations(
            log_id, tree_size, root_hash, map_root_hash,
            issued_at_epoch_seconds, observed_at_epoch_seconds
        ) VALUES(?1, ?2, ?3, ?4, ?5, ?6)
        ON CONFLICT(
            log_id, tree_size, root_hash, map_root_hash, issued_at_epoch_seconds
        ) DO UPDATE SET
            observed_at_epoch_seconds = MAX(
                observed_at_epoch_seconds,
                excluded.observed_at_epoch_seconds
            )
        "#,
        params![
            pin.log_id(),
            u64_to_sql(sth.tree_size)?,
            sth.root_hash,
            sth.map_root_hash,
            u64_to_sql(sth.issued_at_epoch_seconds)?,
            u64_to_sql(observed_at_epoch_seconds)?,
        ],
    )?;
    transaction.execute(
        r#"
        DELETE FROM secure_mesh_kt_gossip_observations
        WHERE log_id = ?1 AND rowid NOT IN (
            SELECT rowid FROM secure_mesh_kt_gossip_observations
            WHERE log_id = ?1
            ORDER BY tree_size DESC, observed_at_epoch_seconds DESC
            LIMIT ?2
        )
        "#,
        params![pin.log_id(), u64_to_sql(MAX_PERSISTED_GOSSIP_OBSERVATIONS)?],
    )?;
    Ok(())
}

pub(crate) fn require_fresh_gossip_checkpoint_transaction(
    transaction: &Transaction<'_>,
    pin: &PinnedKtLogKey,
    checkpoint: &SecureMeshKtCachedCheckpoint,
    freshness_policy: KtFreshnessPolicy,
    now_epoch_seconds: u64,
) -> Result<()> {
    require_fresh_gossip_binding_transaction(
        transaction,
        pin,
        checkpoint.tree_size,
        &checkpoint.root_hash,
        &checkpoint.map_root_hash,
        checkpoint.issued_at_epoch_seconds,
        freshness_policy,
        now_epoch_seconds,
    )
}

pub(crate) fn require_fresh_gossip_observation_transaction(
    transaction: &Transaction<'_>,
    pin: &PinnedKtLogKey,
    sth: &SecureMeshSignedTreeHead,
    freshness_policy: KtFreshnessPolicy,
    now_epoch_seconds: u64,
) -> Result<()> {
    require_fresh_gossip_binding_transaction(
        transaction,
        pin,
        sth.tree_size,
        &sth.root_hash,
        &sth.map_root_hash,
        sth.issued_at_epoch_seconds,
        freshness_policy,
        now_epoch_seconds,
    )
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn require_fresh_gossip_binding_transaction(
    transaction: &Transaction<'_>,
    pin: &PinnedKtLogKey,
    tree_size: u64,
    root_hash: &str,
    map_root_hash: &str,
    issued_at_epoch_seconds: u64,
    freshness_policy: KtFreshnessPolicy,
    now_epoch_seconds: u64,
) -> Result<()> {
    let observation = transaction
        .query_row(
            r#"
            SELECT issued_at_epoch_seconds, observed_at_epoch_seconds
            FROM secure_mesh_kt_gossip_observations
            WHERE log_id = ?1 AND tree_size = ?2
              AND root_hash = ?3 AND map_root_hash = ?4
              AND issued_at_epoch_seconds = ?5
            "#,
            params![
                pin.log_id(),
                u64_to_sql(tree_size)?,
                root_hash,
                map_root_hash,
                u64_to_sql(issued_at_epoch_seconds)?,
            ],
            |row| Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)?)),
        )
        .optional()?
        .ok_or_else(|| {
            anyhow!(
                "secure mesh KT fresh peer-gossip or witness observation is required before authorization"
            )
        })?;
    let observed_issued = sql_to_u64(observation.0, "gossip issue time")?;
    let observed_at = sql_to_u64(observation.1, "gossip observation time")?;
    let max_gossip_age_seconds = freshness_policy
        .max_sth_age_seconds
        .min(KT_PROTOCOL_MAX_GOSSIP_AGE_SECONDS);
    ensure!(
        observed_issued == issued_at_epoch_seconds,
        "secure mesh KT gossip observation signed-tree-head binding is invalid"
    );
    ensure!(
        observed_at <= now_epoch_seconds.saturating_add(freshness_policy.max_future_skew_seconds)
            && now_epoch_seconds <= observed_at.saturating_add(max_gossip_age_seconds),
        "secure mesh KT peer-gossip or witness observation is stale or from the future"
    );
    Ok(())
}
