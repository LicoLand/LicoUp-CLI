use anyhow::{Context, Result, anyhow, ensure};
use rusqlite::params;

use super::super::support::require_text;
use super::store_model::{SecureMeshPairwiseDurableRecord, SecureMeshPairwiseDurableStore};
use crate::core::secure_mesh_secret_store::{SecretStoreAuthorizationRequest, SecretStoreHandle};

impl SecureMeshPairwiseDurableStore {
    pub fn mark_revoked(
        &mut self,
        previous: &SecureMeshPairwiseDurableRecord,
        revoked_at: impl Into<String>,
    ) -> Result<SecureMeshPairwiseDurableRecord> {
        self.retry_pending_secret_cleanup()?;
        let revoked_at = require_text(revoked_at.into(), "revoked_at")?;
        let mut previous_public = self
            .read_public_snapshot(&previous.session_id, &previous.local_endpoint_id)?
            .ok_or_else(|| anyhow!("secure mesh pairwise durable previous snapshot is missing"))?;
        let previous_secret_handle = self.secret_snapshot_handle(
            &previous_public.secret_store_namespace,
            &previous_public.secret_store_key,
        )?;
        let mut received_statement = self.connection.prepare(
            r#"
            SELECT secret_store_namespace, secret_store_key
            FROM secure_mesh_pairwise_received_payloads
            WHERE session_id = ?1 AND local_endpoint_id = ?2
            ORDER BY receipt_id
            "#,
        )?;
        let received_secret_handles = received_statement
            .query_map(
                params![previous.session_id, previous.local_endpoint_id],
                |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
            )?
            .map(|row| {
                let (namespace, key) = row?;
                ensure!(
                    namespace == self.secret_store_namespace && key.starts_with("received.v1."),
                    "secure mesh received payload revoke binding is invalid"
                );
                self.secret_snapshot_handle(&namespace, &key)
            })
            .collect::<Result<Vec<SecretStoreHandle>>>()?;
        drop(received_statement);
        previous_public.revoked = true;
        let revoked_public_json = serde_json::to_string(&previous_public)
            .context("secure mesh pairwise revoked snapshot serialization failed")?;
        let tx = self
            .connection
            .transaction()
            .context("secure mesh pairwise durable revoke transaction failed")?;
        let changed = tx.execute(
            r#"
            UPDATE secure_mesh_pairwise_sessions
            SET revoked_at = ?1,
                state_version = state_version + 1,
                snapshot_json = ?2,
                updated_at = ?1
            WHERE session_id = ?3
              AND local_endpoint_id = ?4
              AND state_version = ?5
              AND revoked_at IS NULL
            "#,
            params![
                revoked_at,
                revoked_public_json,
                previous.session_id,
                previous.local_endpoint_id,
                previous.state_version as i64
            ],
        )?;
        ensure!(
            changed == 1,
            "secure mesh pairwise durable revoke compare-and-swap failed"
        );
        tx.execute(
            r#"
            DELETE FROM secure_mesh_pairwise_pending_deliveries
            WHERE session_id = ?1 AND local_endpoint_id = ?2
            "#,
            params![previous.session_id, previous.local_endpoint_id],
        )?;
        tx.execute(
            r#"
            DELETE FROM secure_mesh_pairwise_received_payloads
            WHERE session_id = ?1 AND local_endpoint_id = ?2
            "#,
            params![previous.session_id, previous.local_endpoint_id],
        )?;
        tx.commit()
            .context("secure mesh pairwise durable revoke commit failed")?;
        let revoke_session =
            self.secret_store
                .begin_authorized_session(&SecretStoreAuthorizationRequest::new(
                    "Secure Mesh pairwise durable revoke cleanup",
                    received_secret_handles.len().saturating_add(1),
                ))?;
        self.delete_secret_or_enqueue_cleanup(&revoke_session, &previous_secret_handle)
            .context("secure mesh pairwise revoked secret cleanup is incomplete")?;
        for handle in &received_secret_handles {
            self.delete_secret_or_enqueue_cleanup(&revoke_session, handle)
                .context("secure mesh received payload revoke cleanup is incomplete")?;
        }
        self.read_record(&previous.session_id, &previous.local_endpoint_id)?
            .ok_or_else(|| anyhow!("secure mesh pairwise durable record disappeared after revoke"))
    }
}
