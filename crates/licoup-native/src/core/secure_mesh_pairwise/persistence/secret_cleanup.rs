use anyhow::{Context, Result, anyhow, ensure};
use rusqlite::params;

use super::super::support::{PAIRWISE_SECRET_STORE_CLASS, PAIRWISE_SNAPSHOT_SCHEMA_VERSION};
use super::{
    namespace_binding::pairwise_secret_store_key_is_bound,
    public_snapshot::PersistedPairwisePublicSession, secret_snapshot::PendingPairwiseSnapshot,
    store_model::SecureMeshPairwiseDurableStore,
};
use crate::core::secure_mesh_secret_store::{
    SecretStoreAuthorizationRequest, SecretStoreAuthorizationSession, SecretStoreHandle,
};

impl SecureMeshPairwiseDurableStore {
    pub fn referenced_secret_snapshot_handles(&self) -> Result<Vec<SecretStoreHandle>> {
        let mut statement = self
            .connection
            .prepare(
                r#"
                SELECT session_id, local_endpoint_id, state_version, snapshot_json
                FROM secure_mesh_pairwise_sessions
                ORDER BY session_id, local_endpoint_id
                "#,
            )
            .context("secure mesh pairwise cleanup snapshot query prepare failed")?;
        let rows = statement
            .query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, i64>(2)?,
                    row.get::<_, String>(3)?,
                ))
            })
            .context("secure mesh pairwise cleanup snapshot query failed")?;
        let mut handles = Vec::new();
        for row in rows {
            let (session_id, local_endpoint_id, state_version, snapshot_json) =
                row.context("secure mesh pairwise cleanup snapshot row read failed")?;
            let state_version = u64::try_from(state_version)
                .context("secure mesh pairwise cleanup snapshot state version is invalid")?;
            ensure!(
                state_version > 0,
                "secure mesh pairwise cleanup snapshot state version is invalid"
            );
            let public: PersistedPairwisePublicSession = serde_json::from_str(&snapshot_json)
                .context("secure mesh pairwise cleanup public snapshot is invalid")?;
            ensure!(
                public.schema_version == PAIRWISE_SNAPSHOT_SCHEMA_VERSION,
                "secure mesh pairwise cleanup snapshot schema is unsupported"
            );
            ensure!(
                public.secret_store_class == PAIRWISE_SECRET_STORE_CLASS,
                "secure mesh pairwise cleanup secret class mismatch"
            );
            ensure!(
                public.session_id == session_id && public.local_endpoint_id == local_endpoint_id,
                "secure mesh pairwise cleanup snapshot subject mismatch"
            );
            ensure!(
                public.secret_store_namespace == self.secret_store_namespace,
                "secure mesh pairwise cleanup secret namespace mismatch"
            );
            ensure!(
                public.state_version == state_version
                    && pairwise_secret_store_key_is_bound(
                        &public.secret_store_key,
                        &session_id,
                        &local_endpoint_id,
                        state_version,
                    ),
                "secure mesh pairwise cleanup secret key mismatch"
            );
            handles.push(self.secret_snapshot_handle(
                &public.secret_store_namespace,
                &public.secret_store_key,
            )?);
        }
        drop(statement);
        let mut received_statement = self
            .connection
            .prepare(
                r#"
                SELECT secret_store_namespace, secret_store_key
                FROM secure_mesh_pairwise_received_payloads
                ORDER BY secret_store_namespace, secret_store_key
                "#,
            )
            .context("secure mesh received payload cleanup query prepare failed")?;
        let received_rows = received_statement
            .query_map([], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
            })
            .context("secure mesh received payload cleanup query failed")?;
        for row in received_rows {
            let (namespace, key) =
                row.context("secure mesh received payload cleanup row read failed")?;
            ensure!(
                namespace == self.secret_store_namespace && key.starts_with("received.v1."),
                "secure mesh received payload cleanup binding is invalid"
            );
            handles.push(self.secret_snapshot_handle(&namespace, &key)?);
        }
        handles.sort_by(|left, right| {
            left.namespace()
                .cmp(right.namespace())
                .then_with(|| left.key().cmp(right.key()))
        });
        handles.dedup();
        Ok(handles)
    }

    pub fn purge_sessions_preserving_prekey_history(&mut self) -> Result<usize> {
        self.retry_pending_secret_cleanup()?;
        let handles = self.referenced_secret_snapshot_handles()?;
        if !handles.is_empty() {
            let authorization = self.secret_store.begin_authorized_session(
                &SecretStoreAuthorizationRequest::new(
                    "Secure Mesh pairwise session purge",
                    handles.len(),
                ),
            )?;
            for handle in &handles {
                self.secret_store
                    .delete_secret_with_session(&authorization, handle)
                    .context("secure mesh pairwise session secret purge failed")?;
            }
        }
        let tx = self
            .connection
            .transaction()
            .context("secure mesh pairwise session purge transaction failed")?;
        tx.execute("DELETE FROM secure_mesh_pairwise_pending_deliveries", [])
            .context("secure mesh pairwise pending delivery purge failed")?;
        tx.execute("DELETE FROM secure_mesh_pairwise_received_payloads", [])
            .context("secure mesh pairwise received payload purge failed")?;
        let deleted = tx
            .execute("DELETE FROM secure_mesh_pairwise_sessions", [])
            .context("secure mesh pairwise session purge failed")?;
        tx.commit()
            .context("secure mesh pairwise session purge commit failed")?;
        Ok(deleted)
    }

    pub(super) fn enqueue_secret_cleanup(&self, handle: &SecretStoreHandle) -> Result<()> {
        self.connection
            .execute(
                r#"
                INSERT INTO secure_mesh_pairwise_secret_cleanup (
                    secret_store_namespace,
                    secret_store_key,
                    attempt_count
                ) VALUES (?1, ?2, 1)
                ON CONFLICT(secret_store_namespace, secret_store_key) DO UPDATE SET
                    attempt_count = CASE
                        WHEN attempt_count < 9223372036854775807
                        THEN attempt_count + 1
                        ELSE attempt_count
                    END
                "#,
                params![handle.namespace(), handle.key()],
            )
            .context("secure mesh pairwise secret cleanup retry enqueue failed")?;
        Ok(())
    }

    pub(super) fn delete_secret_or_enqueue_cleanup(
        &self,
        authorization: &SecretStoreAuthorizationSession,
        handle: &SecretStoreHandle,
    ) -> Result<()> {
        match self
            .secret_store
            .delete_secret_with_session(authorization, handle)
        {
            Ok(()) => {
                self.connection
                    .execute(
                        r#"
                        DELETE FROM secure_mesh_pairwise_secret_cleanup
                        WHERE secret_store_namespace = ?1
                          AND secret_store_key = ?2
                        "#,
                        params![handle.namespace(), handle.key()],
                    )
                    .context("secure mesh pairwise secret cleanup retry dequeue failed")?;
                Ok(())
            }
            Err(_) => {
                self.enqueue_secret_cleanup(handle)?;
                Err(anyhow!(
                    "secure mesh pairwise secret deletion is pending a bounded retry"
                ))
            }
        }
    }

    pub(super) fn cleanup_pending_snapshot(&self, pending: &PendingPairwiseSnapshot) -> Result<()> {
        self.delete_secret_or_enqueue_cleanup(&pending.secret_store_session, &pending.secret_handle)
    }

    pub fn retry_pending_secret_cleanup(&self) -> Result<usize> {
        let mut statement = self
            .connection
            .prepare(
                r#"
                SELECT secret_store_namespace, secret_store_key
                FROM secure_mesh_pairwise_secret_cleanup
                ORDER BY secret_store_namespace, secret_store_key
                "#,
            )
            .context("secure mesh pairwise secret cleanup retry query prepare failed")?;
        let handles = statement
            .query_map([], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
            })
            .context("secure mesh pairwise secret cleanup retry query failed")?
            .map(|row| {
                let (namespace, key) =
                    row.context("secure mesh pairwise secret cleanup retry row read failed")?;
                self.secret_snapshot_handle(&namespace, &key)
            })
            .collect::<Result<Vec<_>>>()?;
        drop(statement);
        if handles.is_empty() {
            return Ok(0);
        }
        let authorization =
            self.secret_store
                .begin_authorized_session(&SecretStoreAuthorizationRequest::new(
                    "Secure Mesh pairwise pending secret cleanup retry",
                    handles.len(),
                ))?;
        let mut deleted = 0usize;
        let mut pending = false;
        for handle in handles {
            match self.delete_secret_or_enqueue_cleanup(&authorization, &handle) {
                Ok(()) => deleted += 1,
                Err(_) => pending = true,
            }
        }
        ensure!(
            !pending,
            "secure mesh pairwise secret cleanup remains pending"
        );
        Ok(deleted)
    }

    #[cfg(test)]
    pub(in crate::core::secure_mesh_pairwise) fn pending_secret_cleanup_count(
        &self,
    ) -> Result<usize> {
        let count: i64 = self.connection.query_row(
            "SELECT COUNT(*) FROM secure_mesh_pairwise_secret_cleanup",
            [],
            |row| row.get(0),
        )?;
        usize::try_from(count)
            .map_err(|_| anyhow!("secure mesh pairwise secret cleanup count is invalid"))
    }
}
