use anyhow::{Result, ensure};
use rusqlite::{Transaction, params};

pub(super) fn advance_pairwise_replay_time_watermark(
    tx: &Transaction<'_>,
    now_unix_seconds: i64,
) -> Result<i64> {
    ensure!(
        now_unix_seconds >= 0,
        "secure mesh pairwise replay clock is before unix epoch"
    );
    let persisted: i64 = tx.query_row(
        "SELECT max_observed_unix_seconds FROM secure_mesh_pairwise_time_guard WHERE singleton = 1",
        [],
        |row| row.get(0),
    )?;
    let effective = persisted.max(now_unix_seconds);
    tx.execute(
        "UPDATE secure_mesh_pairwise_time_guard SET max_observed_unix_seconds = ?1 WHERE singleton = 1",
        params![effective],
    )?;
    Ok(effective)
}
