//! Durable clock watermark and terminal authenticated-freshness guards.

use anyhow::{Result, bail, ensure};
use rusqlite::{Connection, Transaction, TransactionBehavior, params};

use super::super::constants::KT_JSON_SAFE_INTEGER_MAX;
use super::super::signature::{
    KtFreshnessPolicy, PinnedKtLogKey, SecureMeshSignedTreeHead, VerifiedKtFreshness,
};
use super::sql::{sql_to_u64, u64_to_sql};

pub(crate) fn persist_security_block(
    transaction: &Transaction<'_>,
    reason_code: &str,
) -> Result<()> {
    transaction.execute(
        "UPDATE secure_mesh_kt_guard SET blocked = 1, reason_code = ?1 WHERE singleton = 1",
        params![reason_code],
    )?;
    Ok(())
}

pub(crate) fn advance_durable_time_watermark(
    connection: &mut Connection,
    now_epoch_seconds: u64,
) -> Result<u64> {
    let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
    if now_epoch_seconds > KT_JSON_SAFE_INTEGER_MAX {
        persist_security_block(&transaction, "local_clock_out_of_range")?;
        transaction.commit()?;
        bail!(
            "secure mesh KT terminal freshness block persisted: local clock is outside the supported range"
        );
    }
    let (blocked, reason): (i64, Option<String>) = transaction.query_row(
        "SELECT blocked, reason_code FROM secure_mesh_kt_guard WHERE singleton = 1",
        [],
        |row| Ok((row.get(0)?, row.get(1)?)),
    )?;
    ensure!(
        blocked == 0,
        "secure mesh KT security block was previously persisted ({})",
        reason.as_deref().unwrap_or("unspecified")
    );
    let persisted: i64 = transaction.query_row(
        "SELECT max_observed_epoch_seconds FROM secure_mesh_kt_time_guard WHERE singleton = 1",
        [],
        |row| row.get(0),
    )?;
    let persisted = sql_to_u64(persisted, "time watermark")?;
    let effective = persisted.max(now_epoch_seconds);
    transaction.execute(
        "UPDATE secure_mesh_kt_time_guard SET max_observed_epoch_seconds = ?1 WHERE singleton = 1",
        params![u64_to_sql(effective)?],
    )?;
    transaction.commit()?;
    Ok(effective)
}

pub(crate) fn authenticated_sth_temporal_block_reason(
    sth: &SecureMeshSignedTreeHead,
    freshness_policy: KtFreshnessPolicy,
    now_epoch_seconds: u64,
) -> Option<&'static str> {
    if sth.issued_at_epoch_seconds
        > now_epoch_seconds.saturating_add(freshness_policy.max_future_skew_seconds)
    {
        return Some("authenticated_sth_from_future");
    }
    if now_epoch_seconds
        > sth
            .issued_at_epoch_seconds
            .saturating_add(freshness_policy.max_sth_age_seconds)
    {
        return Some("authenticated_sth_expired");
    }
    None
}

pub(crate) fn verify_authenticated_sth_freshness_or_block(
    connection: &mut Connection,
    pin: &PinnedKtLogKey,
    freshness_policy: KtFreshnessPolicy,
    sth: &SecureMeshSignedTreeHead,
    now_epoch_seconds: u64,
) -> Result<VerifiedKtFreshness> {
    sth.verify_authenticity(pin)?;
    if let Some(reason) =
        authenticated_sth_temporal_block_reason(sth, freshness_policy, now_epoch_seconds)
    {
        persist_security_block_connection(connection, reason)?;
        bail!("secure mesh KT terminal freshness block persisted: {reason}");
    }
    sth.verify_freshness(freshness_policy, now_epoch_seconds)
}

pub(crate) fn persist_security_block_connection(
    connection: &mut Connection,
    reason: &str,
) -> Result<()> {
    let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
    persist_security_block(&transaction, reason)?;
    transaction.commit()?;
    Ok(())
}
