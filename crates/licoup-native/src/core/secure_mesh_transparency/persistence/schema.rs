//! Current-schema initialization, pin binding, and explicit destructive reset.

use anyhow::{Result, anyhow, bail, ensure};
use rusqlite::{Connection, OptionalExtension, TransactionBehavior, params};
use std::path::Path;

use super::super::constants::{KT_SCHEMA_VERSION, SECURE_MESH_KT_PROTOCOL_VERSION};
use super::super::signature::PinnedKtLogKey;

const KT_SCHEMA: &str = r#"
CREATE TABLE IF NOT EXISTS secure_mesh_kt_configuration (
    singleton INTEGER PRIMARY KEY CHECK (singleton = 1),
    protocol_version TEXT NOT NULL,
    log_id TEXT NOT NULL,
    key_id TEXT NOT NULL,
    public_key_hex TEXT NOT NULL,
    provenance TEXT NOT NULL
);
CREATE TABLE IF NOT EXISTS secure_mesh_kt_guard (
    singleton INTEGER PRIMARY KEY CHECK (singleton = 1),
    blocked INTEGER NOT NULL CHECK (blocked IN (0, 1)),
    reason_code TEXT
);
CREATE TABLE IF NOT EXISTS secure_mesh_kt_time_guard (
    singleton INTEGER PRIMARY KEY CHECK (singleton = 1),
    max_observed_epoch_seconds INTEGER NOT NULL CHECK (max_observed_epoch_seconds >= 0)
);
CREATE TABLE IF NOT EXISTS secure_mesh_kt_checkpoints (
    log_id TEXT NOT NULL,
    tree_size INTEGER NOT NULL CHECK (tree_size >= 0),
    root_hash TEXT NOT NULL,
    map_root_hash TEXT NOT NULL,
    issued_at_epoch_seconds INTEGER NOT NULL CHECK (issued_at_epoch_seconds >= 0),
    key_id TEXT NOT NULL,
    PRIMARY KEY (log_id, tree_size)
);
CREATE TABLE IF NOT EXISTS secure_mesh_kt_directory_latest (
    log_id TEXT NOT NULL,
    stable_label TEXT NOT NULL,
    version INTEGER NOT NULL CHECK (version >= 0),
    leaf_hash TEXT NOT NULL,
    revoked INTEGER NOT NULL CHECK (revoked IN (0, 1)),
    identity_fingerprint TEXT NOT NULL,
    identity_rotation_epoch INTEGER NOT NULL CHECK (identity_rotation_epoch >= 0),
    identity_key_digest TEXT NOT NULL,
    pairwise_prekey_version INTEGER NOT NULL CHECK (pairwise_prekey_version >= 0),
    signed_prekey_digest TEXT NOT NULL,
    one_time_prekey_digest TEXT NOT NULL,
    mls_key_package_version INTEGER NOT NULL CHECK (mls_key_package_version >= 0),
    mls_key_package_digest TEXT NOT NULL,
    tree_size INTEGER NOT NULL CHECK (tree_size >= 0),
    PRIMARY KEY (log_id, stable_label)
);
CREATE TABLE IF NOT EXISTS secure_mesh_kt_directory_authorizations (
    log_id TEXT NOT NULL,
    stable_label TEXT NOT NULL,
    purpose TEXT NOT NULL,
    directory_version INTEGER NOT NULL CHECK (directory_version >= 0),
    leaf_hash TEXT NOT NULL,
    revoked INTEGER NOT NULL CHECK (revoked IN (0, 1)),
    tree_size INTEGER NOT NULL CHECK (tree_size >= 0),
    root_hash TEXT NOT NULL,
    map_root_hash TEXT NOT NULL,
    issued_at_epoch_seconds INTEGER NOT NULL CHECK (issued_at_epoch_seconds >= 0),
    observed_at_epoch_seconds INTEGER NOT NULL CHECK (observed_at_epoch_seconds >= 0),
    inclusion_json TEXT NOT NULL,
    map_proof_json TEXT NOT NULL,
    PRIMARY KEY (log_id, stable_label, purpose)
);
CREATE TABLE IF NOT EXISTS secure_mesh_kt_gossip_observations (
    log_id TEXT NOT NULL,
    tree_size INTEGER NOT NULL CHECK (tree_size >= 0),
    root_hash TEXT NOT NULL,
    map_root_hash TEXT NOT NULL,
    issued_at_epoch_seconds INTEGER NOT NULL CHECK (issued_at_epoch_seconds >= 0),
    observed_at_epoch_seconds INTEGER NOT NULL CHECK (observed_at_epoch_seconds >= 0),
    PRIMARY KEY (
        log_id, tree_size, root_hash, map_root_hash, issued_at_epoch_seconds
    )
);
CREATE INDEX IF NOT EXISTS secure_mesh_kt_gossip_observed_idx
    ON secure_mesh_kt_gossip_observations(log_id, observed_at_epoch_seconds);
"#;

/// Destructively clear one local verifier database during an explicitly guarded authority reset.
/// The reset is transactional inside SQLite, so WAL/SHM state is reconciled by SQLite rather than
/// by unlinking database sidecars. Callers must hold their own persistent fail-closed reset guard
/// until the new pin/scope configuration has been durably committed.
pub fn reset_kt_persistent_authority_state(path: impl AsRef<Path>) -> Result<()> {
    let mut connection = Connection::open(path)
        .map_err(|_| anyhow!("secure mesh KT authority reset database open failed"))?;
    let transaction = connection
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(|_| anyhow!("secure mesh KT authority reset transaction failed"))?;
    transaction.execute_batch(
        r#"
        DROP TABLE IF EXISTS secure_mesh_kt_configuration;
        DROP TABLE IF EXISTS secure_mesh_kt_guard;
        DROP TABLE IF EXISTS secure_mesh_kt_time_guard;
        DROP TABLE IF EXISTS secure_mesh_kt_checkpoints;
        DROP TABLE IF EXISTS secure_mesh_kt_directory_latest;
        DROP TABLE IF EXISTS secure_mesh_kt_directory_authorizations;
        DROP TABLE IF EXISTS secure_mesh_kt_gossip_observations;
        PRAGMA user_version = 0;
        "#,
    )?;
    transaction.commit()?;
    Ok(())
}

pub(crate) fn initialize_kt_schema(connection: &Connection) -> Result<()> {
    let schema_version: i64 = connection.query_row("PRAGMA user_version", [], |row| row.get(0))?;
    let existing_kt_tables: i64 = connection.query_row(
        "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name LIKE 'secure_mesh_kt_%'",
        [],
        |row| row.get(0),
    )?;
    if schema_version == 0 && existing_kt_tables > 0 {
        bail!(
            "secure mesh KT state schema is unversioned or legacy; explicit security reset and re-pairing are required"
        );
    }
    ensure!(
        matches!(schema_version, 0 | KT_SCHEMA_VERSION),
        "secure mesh KT state schema version is unsupported; explicit security reset and re-pairing are required"
    );
    connection
        .execute_batch(KT_SCHEMA)
        .map_err(|error| anyhow!("secure mesh KT state schema failed: {error}"))?;
    if schema_version == 0 {
        connection.pragma_update(None, "user_version", KT_SCHEMA_VERSION)?;
    }
    let required_time_guard_columns = ["singleton", "max_observed_epoch_seconds"];
    let mut statement = connection.prepare("PRAGMA table_info(secure_mesh_kt_time_guard)")?;
    let time_guard_columns = statement
        .query_map([], |row| row.get::<_, String>(1))?
        .collect::<rusqlite::Result<std::collections::BTreeSet<_>>>()?;
    ensure!(
        required_time_guard_columns
            .iter()
            .all(|column| time_guard_columns.contains(*column)),
        "secure mesh KT time guard schema is incomplete; explicit security reset and re-pairing are required"
    );
    let required_latest_columns = [
        "log_id",
        "stable_label",
        "version",
        "leaf_hash",
        "revoked",
        "identity_fingerprint",
        "identity_rotation_epoch",
        "identity_key_digest",
        "pairwise_prekey_version",
        "signed_prekey_digest",
        "one_time_prekey_digest",
        "mls_key_package_version",
        "mls_key_package_digest",
        "tree_size",
    ];
    let mut statement = connection.prepare("PRAGMA table_info(secure_mesh_kt_directory_latest)")?;
    let columns = statement
        .query_map([], |row| row.get::<_, String>(1))?
        .collect::<rusqlite::Result<std::collections::BTreeSet<_>>>()?;
    ensure!(
        required_latest_columns
            .iter()
            .all(|column| columns.contains(*column)),
        "secure mesh KT state schema is incomplete; explicit security reset and re-pairing are required"
    );
    let required_authorization_columns = [
        "log_id",
        "stable_label",
        "purpose",
        "directory_version",
        "leaf_hash",
        "revoked",
        "tree_size",
        "root_hash",
        "map_root_hash",
        "issued_at_epoch_seconds",
        "observed_at_epoch_seconds",
        "inclusion_json",
        "map_proof_json",
    ];
    let mut statement =
        connection.prepare("PRAGMA table_info(secure_mesh_kt_directory_authorizations)")?;
    let authorization_columns = statement
        .query_map([], |row| row.get::<_, String>(1))?
        .collect::<rusqlite::Result<std::collections::BTreeSet<_>>>()?;
    ensure!(
        required_authorization_columns
            .iter()
            .all(|column| authorization_columns.contains(*column)),
        "secure mesh KT authorization schema is incomplete; explicit security reset and re-pairing are required"
    );
    let required_gossip_columns = [
        "log_id",
        "tree_size",
        "root_hash",
        "map_root_hash",
        "issued_at_epoch_seconds",
        "observed_at_epoch_seconds",
    ];
    let mut statement =
        connection.prepare("PRAGMA table_info(secure_mesh_kt_gossip_observations)")?;
    let gossip_columns = statement
        .query_map([], |row| row.get::<_, String>(1))?
        .collect::<rusqlite::Result<std::collections::BTreeSet<_>>>()?;
    ensure!(
        required_gossip_columns
            .iter()
            .all(|column| gossip_columns.contains(*column)),
        "secure mesh KT gossip schema is incomplete; explicit security reset and re-pairing are required"
    );
    Ok(())
}

pub(crate) fn initialize_or_validate_pin(
    connection: &Connection,
    pin: &PinnedKtLogKey,
) -> Result<()> {
    let existing = connection
        .query_row(
            "SELECT protocol_version, log_id, key_id, public_key_hex, provenance FROM secure_mesh_kt_configuration WHERE singleton = 1",
            [],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, String>(4)?,
                ))
            },
        )
        .optional()?;
    if let Some((protocol, log_id, key_id, public_key, provenance)) = existing {
        ensure!(
            protocol == SECURE_MESH_KT_PROTOCOL_VERSION
                && log_id == pin.log_id()
                && key_id == pin.key_id()
                && public_key == pin.public_key_hex()
                && provenance == pin.provenance().stable_code(),
            "secure mesh KT persisted pin does not match configured authority"
        );
    } else {
        connection.execute(
            "INSERT INTO secure_mesh_kt_configuration(singleton, protocol_version, log_id, key_id, public_key_hex, provenance) VALUES(1, ?1, ?2, ?3, ?4, ?5)",
            params![
                SECURE_MESH_KT_PROTOCOL_VERSION,
                pin.log_id(),
                pin.key_id(),
                pin.public_key_hex(),
                pin.provenance().stable_code(),
            ],
        )?;
    }
    Ok(())
}
