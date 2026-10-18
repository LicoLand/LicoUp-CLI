use std::{path::Path, sync::Arc};

use anyhow::{Context, Result};
use rusqlite::Connection;

use super::super::support::require_text;
use super::{
    public_snapshot::PersistedPairwisePublicSession, store_model::SecureMeshPairwiseDurableStore,
};
use crate::core::secure_mesh_secret_store::{
    SecretStoreAuthorizationRequest, SecretStoreAuthorizationSession, SecureMeshSecretStore,
};

impl SecureMeshPairwiseDurableStore {
    pub fn open_with_secret_store(
        path: impl AsRef<Path>,
        secret_store: Arc<dyn SecureMeshSecretStore>,
        secret_store_namespace: impl Into<String>,
    ) -> Result<Self> {
        let connection = Connection::open(path.as_ref())
            .context("secure mesh pairwise durable store open failed")?;
        let store = Self {
            connection,
            secret_store,
            secret_store_namespace: require_text(
                secret_store_namespace.into(),
                "secret store namespace",
            )?,
        };
        store.initialize()?;
        Ok(store)
    }

    pub fn begin_authorized_session(
        &self,
        request: &SecretStoreAuthorizationRequest,
    ) -> Result<SecretStoreAuthorizationSession> {
        self.secret_store.begin_authorized_session(request)
    }

    pub fn secret_store_backend(&self) -> &'static str {
        self.secret_store.backend()
    }

    pub fn purge_unrecoverable_memory_only_sessions(&mut self) -> Result<usize> {
        if self.secret_store.backend() != "memory-only-ephemeral" {
            return Ok(0);
        }
        let mut statement = self.connection.prepare(
            "SELECT snapshot_json FROM secure_mesh_pairwise_sessions ORDER BY session_id, local_endpoint_id",
        )?;
        let rows = statement.query_map([], |row| row.get::<_, String>(0))?;
        let mut missing_secret = false;
        for row in rows {
            let public: PersistedPairwisePublicSession = serde_json::from_str(&row?)
                .context("secure mesh memory-only public snapshot is invalid")?;
            let handle = self
                .secret_snapshot_handle(&public.secret_store_namespace, &public.secret_store_key)?;
            if self.secret_store.get_secret(&handle)?.is_none() {
                missing_secret = true;
                break;
            }
        }
        drop(statement);
        if !missing_secret {
            return Ok(0);
        }
        let tx = self
            .connection
            .transaction()
            .context("secure mesh unrecoverable memory-only session purge transaction failed")?;
        tx.execute("DELETE FROM secure_mesh_pairwise_pending_deliveries", [])
            .context("secure mesh unrecoverable pending delivery purge failed")?;
        tx.execute("DELETE FROM secure_mesh_pairwise_received_payloads", [])
            .context("secure mesh unrecoverable received payload purge failed")?;
        let deleted = tx
            .execute("DELETE FROM secure_mesh_pairwise_sessions", [])
            .context("secure mesh unrecoverable memory-only session purge failed")?;
        tx.commit()
            .context("secure mesh unrecoverable memory-only session purge commit failed")?;
        Ok(deleted)
    }
}
