use std::path::Path;

use anyhow::{Context, Result, anyhow, ensure};
use rusqlite::{Connection, OptionalExtension, params};

use super::constants::MLS_PUBLIC_STATE_DIGEST_AUTHENTICATED_BACKFILL;

#[derive(Clone, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct SecureMeshMlsGroupMetadata {
    pub group_id_hash: String,
    pub public_state_digest: String,
    pub epoch: u64,
    pub member_count: usize,
    pub own_leaf_index: u32,
    pub active: bool,
    pub participant_endpoint_id: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SecureMeshMlsDurableRecord {
    pub group_id_hash: String,
    pub public_state_digest: String,
    pub participant_endpoint_id: String,
    pub epoch: u64,
    pub state_version: u64,
    pub member_count: usize,
    pub own_leaf_index: u32,
    pub active: bool,
    pub revoked_at_epoch: Option<u64>,
    pub updated_at: String,
}

pub struct SecureMeshMlsDurableStore {
    connection: Connection,
}

impl SecureMeshMlsDurableStore {
    pub fn open_with_path_hardener(
        path: impl AsRef<Path>,
        harden_path: impl Fn(&Path) -> Result<()>,
    ) -> Result<Self> {
        let path = path.as_ref();
        let connection =
            Connection::open(path).with_context(|| "secure mesh MLS durable store open failed")?;
        harden_path(path).context("secure mesh MLS durable store private path hardening failed")?;
        let store = Self { connection };
        store.initialize()?;
        harden_path(path).context("secure mesh MLS durable store private path hardening failed")?;
        Ok(store)
    }

    pub fn upsert_initial(
        &mut self,
        metadata: &SecureMeshMlsGroupMetadata,
        updated_at: impl Into<String>,
    ) -> Result<SecureMeshMlsDurableRecord> {
        validate_metadata(metadata)?;
        let updated_at = require_text(updated_at.into(), "updated_at")?;
        let tx = self
            .connection
            .transaction()
            .context("secure mesh MLS initial durable transaction failed")?;
        let existing: Option<i64> = tx
            .query_row(
                r#"
                SELECT 1
                FROM secure_mesh_mls_group_state
                WHERE group_id_hash = ?1
                  AND participant_endpoint_id = ?2
                "#,
                params![metadata.group_id_hash, metadata.participant_endpoint_id],
                |row| row.get(0),
            )
            .optional()
            .context("secure mesh MLS durable initial existence check failed")?;
        ensure!(
            existing.is_none(),
            "secure mesh MLS durable record already exists"
        );
        tx.execute(
            r#"
            INSERT INTO secure_mesh_mls_group_state (
                group_id_hash,
                public_state_digest,
                participant_endpoint_id,
                epoch,
                state_version,
                member_count,
                own_leaf_index,
                active,
                revoked_at_epoch,
                updated_at
            ) VALUES (?1, ?2, ?3, ?4, 1, ?5, ?6, ?7, NULL, ?8)
            "#,
            params![
                metadata.group_id_hash,
                metadata.public_state_digest,
                metadata.participant_endpoint_id,
                metadata.epoch as i64,
                metadata.member_count as i64,
                metadata.own_leaf_index as i64,
                i64::from(metadata.active),
                updated_at
            ],
        )?;
        tx.commit()
            .context("secure mesh MLS initial durable commit failed")?;
        self.read(&metadata.group_id_hash, &metadata.participant_endpoint_id)?
            .ok_or_else(|| anyhow!("secure mesh MLS durable record disappeared after insert"))
    }

    pub fn commit_epoch(
        &mut self,
        previous: &SecureMeshMlsDurableRecord,
        metadata: &SecureMeshMlsGroupMetadata,
        updated_at: impl Into<String>,
    ) -> Result<SecureMeshMlsDurableRecord> {
        validate_metadata(metadata)?;
        ensure!(
            previous.group_id_hash == metadata.group_id_hash
                && previous.participant_endpoint_id == metadata.participant_endpoint_id,
            "secure mesh MLS durable commit subject mismatch"
        );
        ensure!(
            metadata.epoch > previous.epoch,
            "secure mesh MLS durable commit must strictly advance the epoch"
        );
        ensure!(
            previous.revoked_at_epoch.is_none(),
            "secure mesh MLS durable record is revoked"
        );
        let updated_at = require_text(updated_at.into(), "updated_at")?;
        let tx = self
            .connection
            .transaction()
            .context("secure mesh MLS durable commit transaction failed")?;
        let changed = tx.execute(
            r#"
            UPDATE secure_mesh_mls_group_state
            SET public_state_digest = ?1,
                epoch = ?2,
                state_version = state_version + 1,
                member_count = ?3,
                own_leaf_index = ?4,
                active = ?5,
                updated_at = ?6
            WHERE group_id_hash = ?7
              AND participant_endpoint_id = ?8
              AND state_version = ?9
              AND epoch = ?10
              AND public_state_digest = ?11
              AND revoked_at_epoch IS NULL
            "#,
            params![
                metadata.public_state_digest,
                metadata.epoch as i64,
                metadata.member_count as i64,
                metadata.own_leaf_index as i64,
                i64::from(metadata.active),
                updated_at,
                previous.group_id_hash,
                previous.participant_endpoint_id,
                previous.state_version as i64,
                previous.epoch as i64,
                previous.public_state_digest
            ],
        )?;
        ensure!(
            changed == 1,
            "secure mesh MLS durable compare-and-swap failed"
        );
        tx.commit()
            .context("secure mesh MLS durable commit failed")?;
        self.read(&metadata.group_id_hash, &metadata.participant_endpoint_id)?
            .ok_or_else(|| anyhow!("secure mesh MLS durable record disappeared after commit"))
    }

    pub fn mark_revoked(
        &mut self,
        previous: &SecureMeshMlsDurableRecord,
        revoked_at_epoch: u64,
        updated_at: impl Into<String>,
    ) -> Result<SecureMeshMlsDurableRecord> {
        ensure!(
            revoked_at_epoch >= previous.epoch,
            "secure mesh MLS durable revoke epoch rollback detected"
        );
        let updated_at = require_text(updated_at.into(), "updated_at")?;
        let tx = self
            .connection
            .transaction()
            .context("secure mesh MLS durable revoke transaction failed")?;
        let changed = tx.execute(
            r#"
            UPDATE secure_mesh_mls_group_state
            SET active = 0,
                revoked_at_epoch = ?1,
                state_version = state_version + 1,
                updated_at = ?2
            WHERE group_id_hash = ?3
              AND participant_endpoint_id = ?4
              AND state_version = ?5
              AND public_state_digest = ?6
              AND revoked_at_epoch IS NULL
            "#,
            params![
                revoked_at_epoch as i64,
                updated_at,
                previous.group_id_hash,
                previous.participant_endpoint_id,
                previous.state_version as i64,
                previous.public_state_digest
            ],
        )?;
        ensure!(
            changed == 1,
            "secure mesh MLS durable revoke compare-and-swap failed"
        );
        tx.commit()
            .context("secure mesh MLS durable revoke commit failed")?;
        self.read(&previous.group_id_hash, &previous.participant_endpoint_id)?
            .ok_or_else(|| anyhow!("secure mesh MLS durable record disappeared after revoke"))
    }

    pub fn read(
        &self,
        group_id_hash: &str,
        participant_endpoint_id: &str,
    ) -> Result<Option<SecureMeshMlsDurableRecord>> {
        read_record_tx(&self.connection, group_id_hash, participant_endpoint_id)
    }

    pub fn reconcile_authenticated_snapshot(
        &mut self,
        metadata: &SecureMeshMlsGroupMetadata,
        updated_at: impl Into<String>,
    ) -> Result<SecureMeshMlsDurableRecord> {
        validate_metadata(metadata)?;
        let updated_at = require_text(updated_at.into(), "updated_at")?;
        let tx = self
            .connection
            .transaction()
            .context("secure mesh MLS authenticated metadata reconciliation failed")?;
        let previous = read_record_tx(
            &tx,
            &metadata.group_id_hash,
            &metadata.participant_endpoint_id,
        )?
        .ok_or_else(|| anyhow!("secure mesh MLS durable group authority is missing"))?;
        if previous.public_state_digest != MLS_PUBLIC_STATE_DIGEST_AUTHENTICATED_BACKFILL {
            tx.commit()
                .context("secure mesh MLS authenticated metadata reconciliation commit failed")?;
            return Ok(previous);
        }
        ensure!(
            previous.epoch == metadata.epoch
                && previous.member_count == metadata.member_count
                && previous.own_leaf_index == metadata.own_leaf_index
                && previous.active == metadata.active,
            "secure mesh MLS selected-custody snapshot cannot authenticate durable metadata"
        );
        let changed = tx.execute(
            r#"
            UPDATE secure_mesh_mls_group_state
            SET public_state_digest = ?1,
                state_version = state_version + 1,
                updated_at = ?2
            WHERE group_id_hash = ?3
              AND participant_endpoint_id = ?4
              AND state_version = ?5
              AND epoch = ?6
              AND member_count = ?7
              AND own_leaf_index = ?8
              AND active = ?9
              AND public_state_digest = ?10
            "#,
            params![
                metadata.public_state_digest,
                updated_at,
                metadata.group_id_hash,
                metadata.participant_endpoint_id,
                previous.state_version as i64,
                metadata.epoch as i64,
                metadata.member_count as i64,
                metadata.own_leaf_index as i64,
                i64::from(metadata.active),
                MLS_PUBLIC_STATE_DIGEST_AUTHENTICATED_BACKFILL,
            ],
        )?;
        ensure!(
            changed == 1,
            "secure mesh MLS authenticated metadata reconciliation lost authority"
        );
        tx.commit()
            .context("secure mesh MLS authenticated metadata reconciliation commit failed")?;
        self.read(&metadata.group_id_hash, &metadata.participant_endpoint_id)?
            .ok_or_else(|| {
                anyhow!(
                    "secure mesh MLS durable record disappeared after authenticated reconciliation"
                )
            })
    }

    pub fn has_records_for_participant(&self, participant_endpoint_id: &str) -> Result<bool> {
        let found: Option<i64> = self
            .connection
            .query_row(
                "SELECT 1 FROM secure_mesh_mls_group_state WHERE participant_endpoint_id = ?1 LIMIT 1",
                params![participant_endpoint_id],
                |row| row.get(0),
            )
            .optional()?;
        Ok(found.is_some())
    }

    pub fn purge_unrecoverable_memory_only_state(&mut self) -> Result<usize> {
        self.connection
            .execute("DELETE FROM secure_mesh_mls_group_state", [])
            .context("secure mesh MLS unrecoverable memory-only group-state purge failed")
    }

    pub fn reset_for_kt_authority_change(&mut self) -> Result<usize> {
        let transaction = self
            .connection
            .transaction()
            .context("secure mesh MLS KT-authority reset transaction failed")?;
        let removed = transaction
            .execute("DELETE FROM secure_mesh_mls_group_state", [])
            .context("secure mesh MLS KT-authority group-state reset failed")?;
        transaction
            .commit()
            .context("secure mesh MLS KT-authority group-state reset commit failed")?;
        Ok(removed)
    }

    fn initialize(&self) -> Result<()> {
        let existing_table: Option<String> = self
            .connection
            .query_row(
                "SELECT name FROM sqlite_master WHERE type = 'table' AND name = 'secure_mesh_mls_group_state'",
                [],
                |row| row.get(0),
            )
            .optional()?;
        if existing_table.is_some() {
            let mut statement = self
                .connection
                .prepare("PRAGMA table_info(secure_mesh_mls_group_state)")?;
            let columns = statement
                .query_map([], |row| row.get::<_, String>(1))?
                .collect::<std::result::Result<Vec<_>, _>>()?;
            drop(statement);
            if !columns.iter().any(|column| column == "public_state_digest") {
                self.connection.execute(
                    "ALTER TABLE secure_mesh_mls_group_state ADD COLUMN public_state_digest TEXT NOT NULL DEFAULT 'pending:selected-custody-authenticated-backfill'",
                    [],
                )?;
            }
        }
        self.connection.execute_batch(
            r#"
            CREATE TABLE IF NOT EXISTS secure_mesh_mls_group_state (
                group_id_hash TEXT NOT NULL,
                public_state_digest TEXT NOT NULL,
                participant_endpoint_id TEXT NOT NULL,
                epoch INTEGER NOT NULL,
                state_version INTEGER NOT NULL,
                member_count INTEGER NOT NULL,
                own_leaf_index INTEGER NOT NULL,
                active INTEGER NOT NULL,
                revoked_at_epoch INTEGER,
                updated_at TEXT NOT NULL,
                PRIMARY KEY (group_id_hash, participant_endpoint_id)
            );
            CREATE INDEX IF NOT EXISTS secure_mesh_mls_group_state_epoch_idx
                ON secure_mesh_mls_group_state(group_id_hash, epoch, state_version);
            "#,
        )?;
        Ok(())
    }
}

fn read_record_tx(
    connection: &Connection,
    group_id_hash: &str,
    participant_endpoint_id: &str,
) -> Result<Option<SecureMeshMlsDurableRecord>> {
    connection
        .query_row(
            r#"
            SELECT
                group_id_hash,
                public_state_digest,
                participant_endpoint_id,
                epoch,
                state_version,
                member_count,
                own_leaf_index,
                active,
                revoked_at_epoch,
                updated_at
            FROM secure_mesh_mls_group_state
            WHERE group_id_hash = ?1
              AND participant_endpoint_id = ?2
            "#,
            params![group_id_hash, participant_endpoint_id],
            |row| {
                Ok(SecureMeshMlsDurableRecord {
                    group_id_hash: row.get(0)?,
                    public_state_digest: row.get(1)?,
                    participant_endpoint_id: row.get(2)?,
                    epoch: row.get::<_, i64>(3)? as u64,
                    state_version: row.get::<_, i64>(4)? as u64,
                    member_count: row.get::<_, i64>(5)? as usize,
                    own_leaf_index: row.get::<_, i64>(6)? as u32,
                    active: row.get::<_, i64>(7)? == 1,
                    revoked_at_epoch: row.get::<_, Option<i64>>(8)?.map(|value| value as u64),
                    updated_at: row.get(9)?,
                })
            },
        )
        .optional()
        .map_err(Into::into)
}

fn validate_metadata(metadata: &SecureMeshMlsGroupMetadata) -> Result<()> {
    ensure!(
        metadata.group_id_hash.starts_with("sha256:"),
        "secure mesh MLS group id hash is required"
    );
    ensure!(
        metadata.public_state_digest.starts_with("sha256:"),
        "secure mesh MLS public state digest is required"
    );
    ensure!(
        !metadata.participant_endpoint_id.trim().is_empty(),
        "secure mesh MLS participant endpoint id is required"
    );
    ensure!(
        metadata.member_count > 0,
        "secure mesh MLS member count is required"
    );
    Ok(())
}

fn require_text(value: String, label: &str) -> Result<String> {
    ensure!(
        !value.trim().is_empty(),
        "secure mesh MLS durable {label} is required"
    );
    Ok(value)
}
