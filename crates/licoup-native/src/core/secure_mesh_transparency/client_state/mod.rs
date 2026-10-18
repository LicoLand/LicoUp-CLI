//! Durable verifier construction and shared state.

mod authorization;
mod observation;

use anyhow::{Result, anyhow};
use rusqlite::{Connection, params};
use std::path::Path;

use super::model::SecureMeshKtCachedCheckpoint;
use super::persistence::{
    initialize_kt_schema, initialize_or_validate_pin, latest_checkpoint_connection,
};
use super::signature::{KtFreshnessPolicy, PinnedKtLogKey};

/// Durable verifier state. Construction always requires a preconfigured pin in release builds.
pub struct SecureMeshKtClientState {
    pub(super) connection: Connection,
    pin: Option<PinnedKtLogKey>,
    freshness_policy: KtFreshnessPolicy,
}

impl SecureMeshKtClientState {
    pub fn open(
        path: impl AsRef<Path>,
        pin: PinnedKtLogKey,
        freshness_policy: KtFreshnessPolicy,
    ) -> Result<Self> {
        let connection = Connection::open(path)
            .map_err(|error| anyhow!("secure mesh KT state open failed: {error}"))?;
        Self::from_connection(connection, pin, freshness_policy)
    }

    pub fn open_in_memory(
        pin: PinnedKtLogKey,
        freshness_policy: KtFreshnessPolicy,
    ) -> Result<Self> {
        let connection = Connection::open_in_memory()
            .map_err(|error| anyhow!("secure mesh KT in-memory state open failed: {error}"))?;
        Self::from_connection(connection, pin, freshness_policy)
    }

    fn from_connection(
        connection: Connection,
        pin: PinnedKtLogKey,
        freshness_policy: KtFreshnessPolicy,
    ) -> Result<Self> {
        initialize_kt_schema(&connection)?;
        initialize_or_validate_pin(&connection, &pin)?;
        connection.execute(
            "INSERT OR IGNORE INTO secure_mesh_kt_guard(singleton, blocked, reason_code) VALUES(1, 0, NULL)",
            [],
        )?;
        connection.execute(
            "INSERT OR IGNORE INTO secure_mesh_kt_time_guard(singleton, max_observed_epoch_seconds) VALUES(1, 0)",
            [],
        )?;
        Ok(Self {
            connection,
            pin: Some(pin),
            freshness_policy,
        })
    }

    pub fn pin(&self) -> Result<&PinnedKtLogKey> {
        self.pin
            .as_ref()
            .ok_or_else(|| anyhow!("secure mesh KT explicit log pin is required"))
    }

    pub fn equivocation_detected(&self) -> Result<bool> {
        Ok(self.connection.query_row(
            "SELECT blocked FROM secure_mesh_kt_guard WHERE singleton = 1",
            [],
            |row| row.get::<_, i64>(0),
        )? != 0)
    }

    pub fn latest_checkpoint(&self) -> Result<Option<SecureMeshKtCachedCheckpoint>> {
        latest_checkpoint_connection(&self.connection, self.pin()?.log_id())
    }

    pub fn checkpoint_count(&self) -> Result<u64> {
        let count: i64 = self.connection.query_row(
            "SELECT COUNT(*) FROM secure_mesh_kt_checkpoints WHERE log_id = ?1",
            params![self.pin()?.log_id()],
            |row| row.get(0),
        )?;
        u64::try_from(count).map_err(|_| anyhow!("secure mesh KT checkpoint count is invalid"))
    }
}
