use anyhow::{Context, Result, ensure};
use rusqlite::{Transaction, params};

use super::super::support::{
    MAX_PERSISTED_CAPABILITY_PROOF_USES, sha256_hex, validate_endpoint_id,
};
use super::{
    replay_watermark::advance_pairwise_replay_time_watermark,
    store_model::SecureMeshPairwiseDurableStore,
};
use crate::core::secure_mesh_capability_proof::{
    SignedCapabilityProof, signed_capability_proof_digest,
};

pub(super) struct PreparedCapabilityProofPair {
    local_scope_hash: String,
    first_digest: String,
    second_digest: String,
    first_expiry: i64,
    second_expiry: i64,
    now_unix_seconds: i64,
}

impl PreparedCapabilityProofPair {
    pub(super) fn new(
        secret_store_namespace: &str,
        local_endpoint_id: &str,
        first: &SignedCapabilityProof,
        second: &SignedCapabilityProof,
        now_unix_seconds: i64,
    ) -> Result<Self> {
        validate_endpoint_id(local_endpoint_id)?;
        let first_digest = signed_capability_proof_digest(first)?;
        let second_digest = signed_capability_proof_digest(second)?;
        ensure!(
            first_digest != second_digest,
            "secure mesh durable capability replay ledger requires distinct proofs"
        );
        let first_expiry = first.claims.expires_at_unix_seconds;
        let second_expiry = second.claims.expires_at_unix_seconds;
        ensure!(
            first_expiry >= now_unix_seconds && second_expiry >= now_unix_seconds,
            "secure mesh durable capability replay ledger rejected expired proof"
        );
        Ok(Self {
            local_scope_hash: sha256_hex(
                format!("{secret_store_namespace}:{local_endpoint_id}").as_bytes(),
            ),
            first_digest,
            second_digest,
            first_expiry,
            second_expiry,
            now_unix_seconds,
        })
    }
}

pub(super) fn consume_prepared_capability_proof_pair(
    tx: &Transaction<'_>,
    pair: &PreparedCapabilityProofPair,
) -> Result<()> {
    let effective_now_unix_seconds =
        advance_pairwise_replay_time_watermark(tx, pair.now_unix_seconds)?;
    ensure!(
        pair.first_expiry >= effective_now_unix_seconds
            && pair.second_expiry >= effective_now_unix_seconds,
        "secure mesh durable capability replay ledger rejected proof revived by clock rollback"
    );
    tx.execute(
        "DELETE FROM secure_mesh_pairwise_capability_proof_uses WHERE expires_at_unix_seconds < ?1",
        params![effective_now_unix_seconds],
    )
    .context("secure mesh durable capability replay expiry pruning failed")?;
    let existing_count: i64 = tx
        .query_row(
            r#"
            SELECT COUNT(*)
            FROM secure_mesh_pairwise_capability_proof_uses
            WHERE local_endpoint_scope_hash = ?1
              AND proof_digest IN (?2, ?3)
            "#,
            params![pair.local_scope_hash, pair.first_digest, pair.second_digest],
            |row| row.get(0),
        )
        .context("secure mesh durable capability replay lookup failed")?;
    ensure!(
        existing_count == 0,
        "secure mesh capability proof replay rejected"
    );
    let unexpired_count: i64 = tx
        .query_row(
            r#"
            SELECT COUNT(*)
            FROM secure_mesh_pairwise_capability_proof_uses
            WHERE local_endpoint_scope_hash = ?1
            "#,
            params![pair.local_scope_hash],
            |row| row.get(0),
        )
        .context("secure mesh durable capability replay capacity lookup failed")?;
    ensure!(
        usize::try_from(unexpired_count)
            .unwrap_or(usize::MAX)
            .saturating_add(2)
            <= MAX_PERSISTED_CAPABILITY_PROOF_USES,
        "secure mesh capability proof replay guard is at capacity"
    );
    for (digest, expiry) in [
        (pair.first_digest.as_str(), pair.first_expiry),
        (pair.second_digest.as_str(), pair.second_expiry),
    ] {
        tx.execute(
            r#"
            INSERT INTO secure_mesh_pairwise_capability_proof_uses (
                local_endpoint_scope_hash,
                proof_digest,
                expires_at_unix_seconds,
                consumed_at_unix_seconds
            ) VALUES (?1, ?2, ?3, ?4)
            "#,
            params![
                pair.local_scope_hash,
                digest,
                expiry,
                effective_now_unix_seconds
            ],
        )
        .context("secure mesh durable capability proof consumption failed")?;
    }
    Ok(())
}

impl SecureMeshPairwiseDurableStore {
    #[cfg(test)]
    pub fn consume_capability_proof_pair(
        &mut self,
        local_endpoint_id: &str,
        first: &SignedCapabilityProof,
        second: &SignedCapabilityProof,
        now_unix_seconds: i64,
    ) -> Result<()> {
        let prepared = PreparedCapabilityProofPair::new(
            &self.secret_store_namespace,
            local_endpoint_id,
            first,
            second,
            now_unix_seconds,
        )?;
        let tx = self
            .connection
            .transaction()
            .context("secure mesh durable capability replay transaction failed")?;
        consume_prepared_capability_proof_pair(&tx, &prepared)?;
        tx.commit()
            .context("secure mesh durable capability replay commit failed")?;
        Ok(())
    }
}
