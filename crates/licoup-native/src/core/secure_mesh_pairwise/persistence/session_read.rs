use anyhow::{Context, Result, ensure};
use rusqlite::{OptionalExtension, params};

use super::super::key_ratchet::SecureMeshPairwiseSession;
use super::{
    namespace_binding::pairwise_secret_store_key_is_bound,
    public_snapshot::PersistedPairwisePublicSession,
    store_model::{SecureMeshPairwiseDurableRecord, SecureMeshPairwiseDurableStore},
};
use crate::core::secure_mesh_secret_store::SecretStoreAuthorizationSession;

impl SecureMeshPairwiseDurableStore {
    pub fn load_session(
        &self,
        session_id: &str,
        local_endpoint_id: &str,
    ) -> Result<Option<SecureMeshPairwiseSession>> {
        self.load_session_with_optional_authorization(session_id, local_endpoint_id, None)
    }

    pub fn load_session_with_authorized_session(
        &self,
        session_id: &str,
        local_endpoint_id: &str,
        secret_store_session: &SecretStoreAuthorizationSession,
    ) -> Result<Option<SecureMeshPairwiseSession>> {
        self.load_session_with_optional_authorization(
            session_id,
            local_endpoint_id,
            Some(secret_store_session),
        )
    }

    pub(super) fn load_session_with_optional_authorization(
        &self,
        session_id: &str,
        local_endpoint_id: &str,
        secret_store_session: Option<&SecretStoreAuthorizationSession>,
    ) -> Result<Option<SecureMeshPairwiseSession>> {
        let snapshot_record: Option<(String, Option<String>, i64)> = self
            .connection
            .query_row(
                r#"
                SELECT snapshot_json, revoked_at, state_version
                FROM secure_mesh_pairwise_sessions
                WHERE session_id = ?1
                  AND local_endpoint_id = ?2
                "#,
                params![session_id, local_endpoint_id],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .optional()
            .context("secure mesh pairwise durable snapshot read failed")?;
        snapshot_record
            .map(|(value, revoked_at, state_version)| {
                if revoked_at.is_some() {
                    return Ok(None);
                }
                let public: PersistedPairwisePublicSession = serde_json::from_str(&value)
                    .context("secure mesh pairwise public snapshot deserialization failed")?;
                ensure!(
                    u64::try_from(state_version).ok() == Some(public.state_version)
                        && public.session_id == session_id
                        && public.local_endpoint_id == local_endpoint_id
                        && public.secret_store_namespace == self.secret_store_namespace
                        && pairwise_secret_store_key_is_bound(
                            &public.secret_store_key,
                            session_id,
                            local_endpoint_id,
                            public.state_version,
                        ),
                    "secure mesh pairwise public snapshot row binding verification failed"
                );
                let secrets = self.load_secret_snapshot(&public, secret_store_session)?;
                SecureMeshPairwiseSession::from_persisted_snapshots(public, secrets).map(Some)
            })
            .transpose()
            .map(Option::flatten)
    }

    pub fn read_record(
        &self,
        session_id: &str,
        local_endpoint_id: &str,
    ) -> Result<Option<SecureMeshPairwiseDurableRecord>> {
        self.connection
            .query_row(
                r#"
                SELECT
                    session_id,
                    local_endpoint_id,
                    remote_endpoint_id,
                    state_version,
                    dh_epoch,
                    sent_count,
                    received_count,
                    revoked_at,
                    updated_at
                FROM secure_mesh_pairwise_sessions
                WHERE session_id = ?1
                  AND local_endpoint_id = ?2
                "#,
                params![session_id, local_endpoint_id],
                |row| {
                    Ok(SecureMeshPairwiseDurableRecord {
                        session_id: row.get(0)?,
                        local_endpoint_id: row.get(1)?,
                        remote_endpoint_id: row.get(2)?,
                        state_version: row.get::<_, i64>(3)? as u64,
                        dh_epoch: row.get::<_, i64>(4)? as u64,
                        sent_count: row.get::<_, i64>(5)? as u64,
                        received_count: row.get::<_, i64>(6)? as u64,
                        revoked_at: row.get(7)?,
                        updated_at: row.get(8)?,
                    })
                },
            )
            .optional()
            .map_err(Into::into)
    }

    /// Enumerates the platform-secret-store handles referenced by this durable store.
    ///
    /// The public SQLite snapshot is untrusted input for cleanup purposes. Every handle is
    /// therefore rebound to the row identity, state version, and this store's namespace before it
    /// is returned. Callers can safely delete the returned handles in a single externally-owned
    pub(super) fn read_public_snapshot(
        &self,
        session_id: &str,
        local_endpoint_id: &str,
    ) -> Result<Option<PersistedPairwisePublicSession>> {
        let record = self
            .connection
            .query_row(
                r#"
                SELECT snapshot_json, state_version
                FROM secure_mesh_pairwise_sessions
                WHERE session_id = ?1
                  AND local_endpoint_id = ?2
                "#,
                params![session_id, local_endpoint_id],
                |row| Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?)),
            )
            .optional()
            .context("secure mesh pairwise public snapshot read failed")?;
        record
            .map(|(snapshot_json, state_version)| {
                let public: PersistedPairwisePublicSession =
                    serde_json::from_str(&snapshot_json)
                        .context("secure mesh pairwise public snapshot deserialization failed")?;
                ensure!(
                    u64::try_from(state_version).ok() == Some(public.state_version)
                        && public.session_id == session_id
                        && public.local_endpoint_id == local_endpoint_id
                        && public.secret_store_namespace == self.secret_store_namespace
                        && pairwise_secret_store_key_is_bound(
                            &public.secret_store_key,
                            session_id,
                            local_endpoint_id,
                            public.state_version,
                        ),
                    "secure mesh pairwise public snapshot row binding verification failed"
                );
                Ok(public)
            })
            .transpose()
    }
}
