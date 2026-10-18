use anyhow::{Context, Result};

use super::super::support::PAIRWISE_SNAPSHOT_SCHEMA_VERSION;
use super::store_model::SecureMeshPairwiseDurableStore;
use crate::core::secure_mesh_secret_store::SecretStoreAuthorizationRequest;

impl SecureMeshPairwiseDurableStore {
    pub(super) fn initialize(&self) -> Result<()> {
        self.connection
            .execute_batch("PRAGMA secure_delete = ON;")
            .context("secure mesh pairwise secure-delete enable failed")?;
        let schema_version: u32 = self
            .connection
            .query_row("PRAGMA user_version", [], |row| row.get(0))
            .context("secure mesh pairwise schema version read failed")?;
        let pairwise_tables_exist: bool = self
            .connection
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type = 'table' AND name LIKE 'secure_mesh_pairwise_%')",
                [],
                |row| row.get(0),
            )
            .context("secure mesh pairwise schema existence check failed")?;
        let session_table_exists: bool = self
            .connection
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = 'secure_mesh_pairwise_sessions')",
                [],
                |row| row.get(0),
            )
            .context("secure mesh pairwise session table existence check failed")?;
        let incompatible_schema =
            pairwise_tables_exist && schema_version != PAIRWISE_SNAPSHOT_SCHEMA_VERSION;
        if incompatible_schema {
            if session_table_exists {
                self.remove_incompatible_schema_secrets()?;
            }
            self.connection
                .execute_batch(
                    r#"
                    DROP TABLE IF EXISTS secure_mesh_pairwise_sessions;
                    DROP TABLE IF EXISTS secure_mesh_pairwise_remote_prekey_uses;
                    DROP TABLE IF EXISTS secure_mesh_pairwise_local_prekey_uses;
                    DROP TABLE IF EXISTS secure_mesh_pairwise_capability_proof_uses;
                    DROP TABLE IF EXISTS secure_mesh_pairwise_secret_cleanup;
                    DROP TABLE IF EXISTS secure_mesh_pairwise_time_guard;
                    DROP TABLE IF EXISTS secure_mesh_pairwise_pending_deliveries;
                    DROP TABLE IF EXISTS secure_mesh_pairwise_received_payloads;
                    PRAGMA user_version = 0;
                    "#,
                )
                .context("secure mesh pairwise incompatible state reset failed")?;
        }
        self.connection.execute_batch(
            r#"
            CREATE TABLE IF NOT EXISTS secure_mesh_pairwise_sessions (
                session_id TEXT NOT NULL,
                local_endpoint_id TEXT NOT NULL,
                remote_endpoint_id TEXT NOT NULL,
                state_version INTEGER NOT NULL,
                dh_epoch INTEGER NOT NULL,
                sent_count INTEGER NOT NULL,
                received_count INTEGER NOT NULL,
                revoked_at TEXT,
                snapshot_json TEXT NOT NULL,
                updated_at TEXT NOT NULL,
                PRIMARY KEY (session_id, local_endpoint_id)
            );
            CREATE INDEX IF NOT EXISTS secure_mesh_pairwise_sessions_remote_idx
                ON secure_mesh_pairwise_sessions(remote_endpoint_id, dh_epoch, state_version);
            CREATE TABLE IF NOT EXISTS secure_mesh_pairwise_remote_prekey_uses (
                remote_endpoint_id TEXT NOT NULL,
                remote_identity_fingerprint TEXT NOT NULL,
                signed_prekey_id TEXT NOT NULL,
                one_time_prekey_id TEXT NOT NULL,
                one_time_prekey_public_key_hash TEXT NOT NULL,
                one_time_mlkem1024_prekey_id TEXT NOT NULL,
                one_time_mlkem1024_prekey_public_key_hash TEXT NOT NULL,
                directory_authorization_digest TEXT NOT NULL,
                session_id TEXT NOT NULL,
                local_endpoint_id TEXT NOT NULL,
                used_at TEXT NOT NULL,
                PRIMARY KEY (
                    remote_endpoint_id,
                    remote_identity_fingerprint,
                    one_time_prekey_id,
                    one_time_prekey_public_key_hash
                ),
                UNIQUE (
                    remote_endpoint_id,
                    remote_identity_fingerprint,
                    one_time_prekey_id
                ),
                UNIQUE (
                    remote_endpoint_id,
                    remote_identity_fingerprint,
                    one_time_prekey_public_key_hash
                ),
                UNIQUE (
                    remote_endpoint_id,
                    remote_identity_fingerprint,
                    one_time_mlkem1024_prekey_id
                ),
                UNIQUE (
                    remote_endpoint_id,
                    remote_identity_fingerprint,
                    one_time_mlkem1024_prekey_public_key_hash
                )
            );
            CREATE INDEX IF NOT EXISTS secure_mesh_pairwise_remote_prekey_uses_local_idx
                ON secure_mesh_pairwise_remote_prekey_uses(local_endpoint_id, session_id);
            CREATE TABLE IF NOT EXISTS secure_mesh_pairwise_local_prekey_uses (
                local_endpoint_id TEXT NOT NULL,
                local_identity_fingerprint TEXT NOT NULL,
                one_time_prekey_id TEXT NOT NULL,
                one_time_prekey_public_key_hash TEXT NOT NULL,
                one_time_mlkem1024_prekey_id TEXT NOT NULL,
                one_time_mlkem1024_prekey_public_key_hash TEXT NOT NULL,
                session_id TEXT NOT NULL,
                used_at TEXT NOT NULL,
                PRIMARY KEY (
                    local_endpoint_id,
                    local_identity_fingerprint,
                    one_time_prekey_id,
                    one_time_prekey_public_key_hash
                ),
                UNIQUE (
                    local_endpoint_id,
                    local_identity_fingerprint,
                    one_time_prekey_id
                ),
                UNIQUE (
                    local_endpoint_id,
                    local_identity_fingerprint,
                    one_time_prekey_public_key_hash
                ),
                UNIQUE (
                    local_endpoint_id,
                    local_identity_fingerprint,
                    one_time_mlkem1024_prekey_id
                ),
                UNIQUE (
                    local_endpoint_id,
                    local_identity_fingerprint,
                    one_time_mlkem1024_prekey_public_key_hash
                )
            );
            CREATE INDEX IF NOT EXISTS secure_mesh_pairwise_local_prekey_uses_session_idx
                ON secure_mesh_pairwise_local_prekey_uses(session_id, local_endpoint_id);
            CREATE TABLE IF NOT EXISTS secure_mesh_pairwise_capability_proof_uses (
                local_endpoint_scope_hash TEXT NOT NULL,
                proof_digest TEXT NOT NULL,
                expires_at_unix_seconds INTEGER NOT NULL,
                consumed_at_unix_seconds INTEGER NOT NULL,
                PRIMARY KEY (local_endpoint_scope_hash, proof_digest)
            );
            CREATE INDEX IF NOT EXISTS secure_mesh_pairwise_capability_proof_expiry_idx
                ON secure_mesh_pairwise_capability_proof_uses(expires_at_unix_seconds);
            CREATE TABLE IF NOT EXISTS secure_mesh_pairwise_secret_cleanup (
                secret_store_namespace TEXT NOT NULL,
                secret_store_key TEXT NOT NULL,
                attempt_count INTEGER NOT NULL CHECK (attempt_count > 0),
                PRIMARY KEY (secret_store_namespace, secret_store_key)
            );
            CREATE TABLE IF NOT EXISTS secure_mesh_pairwise_time_guard (
                singleton INTEGER PRIMARY KEY CHECK (singleton = 1),
                max_observed_unix_seconds INTEGER NOT NULL CHECK (
                    max_observed_unix_seconds >= 0
                )
            );
            INSERT OR IGNORE INTO secure_mesh_pairwise_time_guard (
                singleton,
                max_observed_unix_seconds
            ) VALUES (1, 0);
            CREATE TABLE IF NOT EXISTS secure_mesh_pairwise_pending_deliveries (
                session_id TEXT NOT NULL,
                local_endpoint_id TEXT NOT NULL,
                delivery_kind TEXT NOT NULL,
                envelope_id TEXT NOT NULL,
                expires_at TEXT NOT NULL,
                envelope_json TEXT NOT NULL,
                binding_json TEXT NOT NULL,
                created_at TEXT NOT NULL,
                PRIMARY KEY (session_id, local_endpoint_id, delivery_kind),
                UNIQUE (envelope_id)
            );
            CREATE TABLE IF NOT EXISTS secure_mesh_pairwise_received_payloads (
                session_id TEXT NOT NULL,
                local_endpoint_id TEXT NOT NULL,
                receipt_id TEXT NOT NULL,
                binding_digest TEXT NOT NULL,
                mailbox_id TEXT NOT NULL,
                secret_store_namespace TEXT NOT NULL,
                secret_store_key TEXT NOT NULL,
                received_at TEXT NOT NULL,
                PRIMARY KEY (session_id, local_endpoint_id, receipt_id),
                UNIQUE (session_id, local_endpoint_id, binding_digest),
                UNIQUE (secret_store_namespace, secret_store_key)
            );
            PRAGMA user_version = 11;
            "#,
        )?;
        if incompatible_schema {
            self.connection
                .execute_batch("VACUUM;")
                .context("secure mesh pairwise incompatible state secure purge failed")?;
        }
        Ok(())
    }

    pub(super) fn remove_incompatible_schema_secrets(&self) -> Result<()> {
        let mut statement = self
            .connection
            .prepare("SELECT snapshot_json FROM secure_mesh_pairwise_sessions")
            .context("secure mesh pairwise reset snapshot query prepare failed")?;
        let rows = statement
            .query_map([], |row| row.get::<_, String>(0))
            .context("secure mesh pairwise reset snapshot query failed")?;
        let mut handles = Vec::new();
        for row in rows {
            let Ok(snapshot_json) = row else {
                continue;
            };
            let Ok(value) = serde_json::from_str::<serde_json::Value>(&snapshot_json) else {
                continue;
            };
            let namespace = value
                .get("secret_store_namespace")
                .and_then(serde_json::Value::as_str);
            let key = value
                .get("secret_store_key")
                .and_then(serde_json::Value::as_str);
            if let (Some(namespace), Some(key)) = (namespace, key) {
                if namespace == self.secret_store_namespace {
                    if let Ok(handle) = self.secret_snapshot_handle(namespace, key) {
                        handles.push(handle);
                    }
                }
            }
        }
        drop(statement);
        let received_table_exists: bool = self.connection.query_row(
            "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = 'secure_mesh_pairwise_received_payloads')",
            [],
            |row| row.get(0),
        )?;
        if received_table_exists {
            let mut received_statement = self.connection.prepare(
                r#"
                SELECT secret_store_namespace, secret_store_key
                FROM secure_mesh_pairwise_received_payloads
                "#,
            )?;
            let received_rows = received_statement.query_map([], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
            })?;
            for row in received_rows {
                let Ok((namespace, key)) = row else {
                    continue;
                };
                if namespace == self.secret_store_namespace
                    && key.starts_with("received.v1.")
                    && let Ok(handle) = self.secret_snapshot_handle(&namespace, &key)
                {
                    handles.push(handle);
                }
            }
        }
        handles.sort_by(|left, right| {
            left.namespace()
                .cmp(right.namespace())
                .then_with(|| left.key().cmp(right.key()))
        });
        handles.dedup();
        if handles.is_empty() {
            return Ok(());
        }
        let authorization =
            self.secret_store
                .begin_authorized_session(&SecretStoreAuthorizationRequest::new(
                    "Secure Mesh incompatible pairwise state reset",
                    handles.len(),
                ))?;
        for handle in handles {
            self.secret_store
                .delete_secret_with_session(&authorization, &handle)
                .context("secure mesh pairwise reset secret removal failed")?;
        }
        Ok(())
    }
}
