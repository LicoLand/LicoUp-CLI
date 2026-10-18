//! Bounded checkpoint persistence and append-only transition enforcement.

use anyhow::{Result, anyhow, ensure};
use rusqlite::{Connection, OptionalExtension, Transaction, params};

use super::super::constants::MAX_PERSISTED_CHECKPOINTS;
use super::super::model::{SecureMeshKtCachedCheckpoint, SecureMeshKtConsistencyProof};
use super::super::proofs::{checkpoint_from_sth, verify_kt_consistency};
use super::super::signature::{KtFreshnessPolicy, PinnedKtLogKey, SecureMeshSignedTreeHead};
use super::sql::u64_to_sql;
use super::time_guard::persist_security_block;

pub(crate) enum CheckpointTransition {
    Accepted(SecureMeshKtCachedCheckpoint),
    SecurityBlock(&'static str),
}

pub(crate) fn latest_checkpoint_connection(
    connection: &Connection,
    log_id: &str,
) -> Result<Option<SecureMeshKtCachedCheckpoint>> {
    connection
        .query_row(
            "SELECT tree_size, root_hash, map_root_hash, issued_at_epoch_seconds FROM secure_mesh_kt_checkpoints WHERE log_id = ?1 ORDER BY tree_size DESC LIMIT 1",
            params![log_id],
            |row| {
                let tree_size = row.get::<_, i64>(0)?;
                let issued = row.get::<_, i64>(3)?;
                Ok(SecureMeshKtCachedCheckpoint {
                    tree_size: u64::try_from(tree_size).map_err(|error| {
                        rusqlite::Error::FromSqlConversionFailure(
                            0,
                            rusqlite::types::Type::Integer,
                            Box::new(error),
                        )
                    })?,
                    root_hash: row.get(1)?,
                    map_root_hash: row.get(2)?,
                    issued_at_epoch_seconds: u64::try_from(issued).map_err(|error| {
                        rusqlite::Error::FromSqlConversionFailure(
                            3,
                            rusqlite::types::Type::Integer,
                            Box::new(error),
                        )
                    })?,
                })
            },
        )
        .optional()
        .map_err(Into::into)
}

pub(crate) fn advance_checkpoint_transaction(
    transaction: &Transaction<'_>,
    pin: &PinnedKtLogKey,
    freshness_policy: KtFreshnessPolicy,
    sth: &SecureMeshSignedTreeHead,
    consistency: Option<&SecureMeshKtConsistencyProof>,
    now_epoch_seconds: u64,
) -> Result<CheckpointTransition> {
    let blocked = transaction.query_row(
        "SELECT blocked FROM secure_mesh_kt_guard WHERE singleton = 1",
        [],
        |row| row.get::<_, i64>(0),
    )? != 0;
    ensure!(
        !blocked,
        "secure mesh KT equivocation was previously persisted; authorization is blocked"
    );
    let latest = latest_checkpoint_connection(transaction, pin.log_id())?;
    if let Some(cached) = &latest {
        if sth.tree_size < cached.tree_size {
            persist_security_block(transaction, "tree_rollback")?;
            return Ok(CheckpointTransition::SecurityBlock("tree rollback"));
        }
        if sth.tree_size == cached.tree_size {
            if sth.root_hash != cached.root_hash || sth.map_root_hash != cached.map_root_hash {
                persist_security_block(transaction, "same_size_split_view")?;
                return Ok(CheckpointTransition::SecurityBlock("same-size split view"));
            }
            transaction.execute(
                "UPDATE secure_mesh_kt_checkpoints SET issued_at_epoch_seconds = MAX(issued_at_epoch_seconds, ?1) WHERE log_id = ?2 AND tree_size = ?3",
                params![u64_to_sql(sth.issued_at_epoch_seconds)?, pin.log_id(), u64_to_sql(sth.tree_size)?],
            )?;
            return Ok(CheckpointTransition::Accepted(checkpoint_from_sth(sth)));
        }

        let proof = consistency.ok_or_else(|| {
            anyhow!("secure mesh KT consistency proof is required for tree advance")
        })?;
        ensure!(
            proof.second_signed_tree_head == *sth,
            "secure mesh KT consistency proof targets a different signed tree head"
        );
        verify_kt_consistency(proof, pin, freshness_policy, now_epoch_seconds, cached)?;
    }

    transaction.execute(
        "INSERT INTO secure_mesh_kt_checkpoints(log_id, tree_size, root_hash, map_root_hash, issued_at_epoch_seconds, key_id) VALUES(?1, ?2, ?3, ?4, ?5, ?6)",
        params![
            pin.log_id(),
            u64_to_sql(sth.tree_size)?,
            sth.root_hash,
            sth.map_root_hash,
            u64_to_sql(sth.issued_at_epoch_seconds)?,
            pin.key_id(),
        ],
    )?;
    transaction.execute(
        "DELETE FROM secure_mesh_kt_checkpoints
         WHERE log_id = ?1 AND tree_size NOT IN (
             SELECT tree_size FROM secure_mesh_kt_checkpoints
             WHERE log_id = ?1 ORDER BY tree_size DESC LIMIT ?2
         )",
        params![pin.log_id(), u64_to_sql(MAX_PERSISTED_CHECKPOINTS)?],
    )?;
    Ok(CheckpointTransition::Accepted(checkpoint_from_sth(sth)))
}
