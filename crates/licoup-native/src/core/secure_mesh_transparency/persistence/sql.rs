//! Checked conversion at the SQLite integer boundary.

use anyhow::{Result, anyhow};

pub(crate) fn u64_to_sql(value: u64) -> Result<i64> {
    i64::try_from(value).map_err(|_| anyhow!("secure mesh KT integer exceeds SQLite range"))
}

pub(crate) fn sql_to_u64(value: i64, label: &str) -> Result<u64> {
    u64::try_from(value).map_err(|_| anyhow!("secure mesh KT persisted {label} is invalid"))
}
