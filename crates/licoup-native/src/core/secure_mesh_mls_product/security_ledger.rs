use super::constants::{
    MAX_DELIVERED_MLS_OPERATIONS_PER_SCOPE, MAX_INCOMPLETE_MLS_OPERATIONS_PER_SCOPE,
    MLS_SECURITY_LEDGER_SCHEMA, STALE_EMPTY_PREPARED_OPERATION_SECONDS,
};
use super::helpers::hex_sha256;
use super::ledger_transaction::{
    consume_prepared_security_transaction, mls_security_scope_hash, read_operation_connection,
    read_operation_transaction, reservation_keys, reserve_operation_key_transaction,
    reserve_prepared_security_transaction, validate_operation_identity,
    validate_prepared_security_inputs,
};
use anyhow::{Result, anyhow, ensure};
use base64::{Engine as _, engine::general_purpose};
use rusqlite::{Connection, TransactionBehavior, params};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::path::Path;

use crate::core::secure_mesh_capability_proof::{
    SignedCapabilityProof, signed_capability_proof_digest,
};
use crate::core::secure_mesh_mls::SecureMeshMlsGroupMetadata;
use crate::core::secure_mesh_trust::DeviceTrustPublicIdentity;

#[cfg(test)]
mod test_support;

/// Durable, privacy-minimized ledger for every MLS one-time security input.
///
/// Member-add key-package consumption and both capability-proof consumptions are committed in
/// one SQLite transaction. Persisted identity and input values are hashes, never raw identifiers
/// or proofs.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub(crate) struct PreparedMlsCapabilityProofUse {
    pub(super) proof_digest: String,
    pub(super) expires_at_unix_seconds: i64,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub(crate) struct PreparedMlsKeyPackageUse {
    pub(super) key_package_id_hash: String,
    pub(super) key_package_public_key_hash: String,
    pub(super) group_id_hash: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub(crate) struct PreparedMlsSecurityInputs {
    pub(super) local_endpoint_scope_hash: String,
    pub(super) key_package: Option<PreparedMlsKeyPackageUse>,
    pub(super) capability_proofs: [PreparedMlsCapabilityProofUse; 2],
    pub(super) consumed_at_unix_seconds: i64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum SecureMeshMlsOperationState {
    Prepared,
    CryptoPrepared,
    CryptoCommitted,
    MetadataReconciled,
    Delivered,
}

impl SecureMeshMlsOperationState {
    pub(super) fn as_str(self) -> &'static str {
        match self {
            Self::Prepared => "prepared",
            Self::CryptoPrepared => "crypto_prepared",
            Self::CryptoCommitted => "crypto_committed",
            Self::MetadataReconciled => "metadata_reconciled",
            Self::Delivered => "delivered",
        }
    }

    pub(super) fn parse(value: &str) -> Result<Self> {
        match value {
            "prepared" => Ok(Self::Prepared),
            "crypto_prepared" => Ok(Self::CryptoPrepared),
            "crypto_committed" => Ok(Self::CryptoCommitted),
            "metadata_reconciled" => Ok(Self::MetadataReconciled),
            "delivered" => Ok(Self::Delivered),
            _ => Err(anyhow!(
                "secure mesh MLS operation journal state is invalid"
            )),
        }
    }
}

#[derive(Clone, Debug)]
pub(crate) struct SecureMeshMlsOperationRecord {
    pub operation_id: String,
    pub action: String,
    pub state: SecureMeshMlsOperationState,
    pub response: Option<Value>,
    pub group_id: Option<Vec<u8>>,
    pub base_metadata: Option<SecureMeshMlsGroupMetadata>,
    pub expected_metadata: Option<SecureMeshMlsGroupMetadata>,
}

pub struct SecureMeshMlsSecurityLedger {
    pub(super) connection: Connection,
}

impl SecureMeshMlsSecurityLedger {
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        let connection = Connection::open(path)
            .map_err(|error| anyhow!("secure mesh MLS security ledger open failed: {error}"))?;
        connection.pragma_update(None, "foreign_keys", "ON")?;
        let foreign_keys: i64 =
            connection.pragma_query_value(None, "foreign_keys", |row| row.get(0))?;
        ensure!(
            foreign_keys == 1,
            "secure mesh MLS security ledger foreign keys are disabled"
        );
        connection
            .execute_batch(MLS_SECURITY_LEDGER_SCHEMA)
            .map_err(|error| anyhow!("secure mesh MLS security ledger schema failed: {error}"))?;
        let mut statement = connection.prepare("PRAGMA table_info(secure_mesh_mls_operations)")?;
        let columns = statement
            .query_map([], |row| row.get::<_, String>(1))?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        drop(statement);
        if !columns
            .iter()
            .any(|column| column == "local_endpoint_scope_hash")
            || !columns.iter().any(|column| column == "base_metadata_json")
            || !columns.iter().any(|column| column == "group_id_base64url")
        {
            connection.execute_batch(
                r#"
                DROP TABLE IF EXISTS secure_mesh_mls_operation_reservations;
                DROP TABLE IF EXISTS secure_mesh_mls_operations;
                "#,
            )?;
            connection.execute_batch(MLS_SECURITY_LEDGER_SCHEMA)?;
        }
        Ok(Self { connection })
    }

    pub fn reset_for_kt_authority_change(&mut self) -> Result<()> {
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|_| anyhow!("secure mesh MLS KT-authority ledger reset failed"))?;
        transaction.execute("DELETE FROM secure_mesh_mls_operation_reservations", [])?;
        transaction.execute("DELETE FROM secure_mesh_mls_operations", [])?;
        transaction.execute("DELETE FROM secure_mesh_mls_capability_proof_uses", [])?;
        transaction.execute("DELETE FROM secure_mesh_mls_keypackage_uses", [])?;
        transaction.execute(
            "UPDATE secure_mesh_mls_time_guard SET max_observed_unix_seconds = 0 WHERE singleton = 1",
            [],
        )?;
        transaction.commit()?;
        Ok(())
    }

    pub(crate) fn begin_operation(
        &mut self,
        operation_id: &str,
        action: &str,
        request_digest: &str,
        local_identity: &DeviceTrustPublicIdentity,
        now_unix_seconds: i64,
    ) -> Result<SecureMeshMlsOperationRecord> {
        validate_operation_identity(operation_id, action, request_digest)?;
        let local_scope_hash = mls_security_scope_hash(local_identity)?;
        let tx = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|error| anyhow!("secure mesh MLS operation begin failed: {error}"))?;
        let existing = read_operation_transaction(&tx, operation_id)?;
        if let Some((record, existing_action, existing_request_digest)) = existing {
            let existing_scope: String = tx.query_row(
                "SELECT local_endpoint_scope_hash FROM secure_mesh_mls_operations WHERE operation_id = ?1",
                params![operation_id],
                |row| row.get(0),
            )?;
            ensure!(
                existing_action == action
                    && existing_request_digest == request_digest
                    && existing_scope == local_scope_hash,
                "secure mesh MLS operation id conflicts with another request"
            );
            tx.commit().map_err(|error| {
                anyhow!("secure mesh MLS operation begin commit failed: {error}")
            })?;
            return Ok(record);
        }
        let stale_before = now_unix_seconds
            .checked_sub(STALE_EMPTY_PREPARED_OPERATION_SECONDS)
            .ok_or_else(|| anyhow!("secure mesh MLS operation cleanup time is invalid"))?;
        tx.execute(
            r#"
            DELETE FROM secure_mesh_mls_operations
            WHERE state = 'prepared'
              AND response_json IS NULL
              AND prepared_security_json IS NULL
              AND updated_at_unix_seconds < ?1
              AND NOT EXISTS (
                  SELECT 1 FROM secure_mesh_mls_operation_reservations reservations
                  WHERE reservations.operation_id = secure_mesh_mls_operations.operation_id
              )
            "#,
            params![stale_before],
        )?;
        tx.execute(
            r#"
            DELETE FROM secure_mesh_mls_operations
            WHERE operation_id IN (
                SELECT operation_id FROM secure_mesh_mls_operations
                WHERE local_endpoint_scope_hash = ?1 AND state = 'delivered'
                ORDER BY updated_at_unix_seconds DESC, operation_id DESC
                LIMIT -1 OFFSET ?2
            )
            "#,
            params![
                local_scope_hash,
                i64::try_from(MAX_DELIVERED_MLS_OPERATIONS_PER_SCOPE).unwrap_or(i64::MAX)
            ],
        )?;
        let incomplete_count: i64 = tx.query_row(
            r#"
            SELECT COUNT(*) FROM secure_mesh_mls_operations
            WHERE local_endpoint_scope_hash = ?1 AND state != 'delivered'
            "#,
            params![local_scope_hash],
            |row| row.get(0),
        )?;
        ensure!(
            usize::try_from(incomplete_count).unwrap_or(usize::MAX)
                < MAX_INCOMPLETE_MLS_OPERATIONS_PER_SCOPE,
            "secure mesh MLS incomplete operation journal is at capacity"
        );
        tx.execute(
            r#"
            INSERT INTO secure_mesh_mls_operations (
                operation_id,
                local_endpoint_scope_hash,
                action,
                request_digest,
                state,
                response_json,
                group_id_base64url,
                base_metadata_json,
                expected_metadata_json,
                prepared_security_json,
                created_at_unix_seconds,
                updated_at_unix_seconds
            ) VALUES (?1, ?2, ?3, ?4, 'prepared', NULL, NULL, NULL, NULL, NULL, ?5, ?5)
            "#,
            params![
                operation_id,
                local_scope_hash,
                action,
                request_digest,
                now_unix_seconds
            ],
        )?;
        tx.commit()
            .map_err(|error| anyhow!("secure mesh MLS operation begin commit failed: {error}"))?;
        self.operation(operation_id)?.ok_or_else(|| {
            anyhow!("secure mesh MLS operation disappeared after journal preparation")
        })
    }

    pub(crate) fn stage_operation(
        &mut self,
        operation_id: &str,
        response: &Value,
        group_id: &[u8],
        base_metadata: Option<&SecureMeshMlsGroupMetadata>,
        expected_metadata: &SecureMeshMlsGroupMetadata,
        prepared_security: &PreparedMlsSecurityInputs,
        now_unix_seconds: i64,
    ) -> Result<SecureMeshMlsOperationRecord> {
        validate_prepared_security_inputs(prepared_security, now_unix_seconds)?;
        ensure!(
            !group_id.is_empty(),
            "secure mesh MLS operation group id is required"
        );
        let group_id_base64url = general_purpose::URL_SAFE_NO_PAD.encode(group_id);
        ensure!(
            expected_metadata.group_id_hash == format!("sha256:{}", hex_sha256(group_id)),
            "secure mesh MLS operation group id does not match expected metadata"
        );
        let response_json = serde_json::to_string(response)
            .map_err(|_| anyhow!("secure mesh MLS operation response encoding failed"))?;
        let metadata_json = serde_json::to_string(expected_metadata)
            .map_err(|_| anyhow!("secure mesh MLS operation metadata encoding failed"))?;
        let base_metadata_json = base_metadata
            .map(serde_json::to_string)
            .transpose()
            .map_err(|_| anyhow!("secure mesh MLS operation base metadata encoding failed"))?;
        if let Some(base) = base_metadata {
            ensure!(
                base.group_id_hash == expected_metadata.group_id_hash
                    && base.participant_endpoint_id == expected_metadata.participant_endpoint_id
                    && expected_metadata.epoch > base.epoch,
                "secure mesh MLS operation base metadata does not strictly precede expected state"
            );
        }
        let security_json = serde_json::to_string(prepared_security)
            .map_err(|_| anyhow!("secure mesh MLS operation security encoding failed"))?;
        let tx = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|error| anyhow!("secure mesh MLS operation stage failed: {error}"))?;
        let (record, _, _) = read_operation_transaction(&tx, operation_id)?
            .ok_or_else(|| anyhow!("secure mesh MLS operation journal entry is missing"))?;
        let operation_scope: String = tx.query_row(
            "SELECT local_endpoint_scope_hash FROM secure_mesh_mls_operations WHERE operation_id = ?1",
            params![operation_id],
            |row| row.get(0),
        )?;
        ensure!(
            operation_scope == prepared_security.local_endpoint_scope_hash,
            "secure mesh MLS operation security scope differs from journal authority"
        );
        ensure!(
            matches!(
                record.state,
                SecureMeshMlsOperationState::Prepared | SecureMeshMlsOperationState::CryptoPrepared
            ),
            "secure mesh MLS committed operation cannot be restaged"
        );
        reserve_prepared_security_transaction(&tx, operation_id, prepared_security)?;
        reserve_operation_key_transaction(
            &tx,
            operation_id,
            &prepared_security.local_endpoint_scope_hash,
            "participant-writer",
        )?;
        let changed = tx.execute(
            r#"
            UPDATE secure_mesh_mls_operations
            SET state = 'crypto_prepared',
                response_json = ?1,
                group_id_base64url = ?2,
                base_metadata_json = ?3,
                expected_metadata_json = ?4,
                prepared_security_json = ?5,
                updated_at_unix_seconds = ?6
            WHERE operation_id = ?7
              AND state IN ('prepared', 'crypto_prepared')
            "#,
            params![
                response_json,
                group_id_base64url,
                base_metadata_json,
                metadata_json,
                security_json,
                now_unix_seconds,
                operation_id
            ],
        )?;
        ensure!(
            changed == 1,
            "secure mesh MLS operation stage lost journal ownership"
        );
        tx.commit()
            .map_err(|error| anyhow!("secure mesh MLS operation stage commit failed: {error}"))?;
        self.operation(operation_id)?
            .ok_or_else(|| anyhow!("secure mesh MLS staged operation disappeared"))
    }

    pub(crate) fn reset_crypto_prepared_operation_for_retry(
        &mut self,
        operation_id: &str,
        now_unix_seconds: i64,
    ) -> Result<SecureMeshMlsOperationRecord> {
        let tx = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let (record, _, _) = read_operation_transaction(&tx, operation_id)?
            .ok_or_else(|| anyhow!("secure mesh MLS operation journal entry is missing"))?;
        if record.state == SecureMeshMlsOperationState::Prepared {
            tx.commit()?;
            return Ok(record);
        }
        ensure!(
            record.state == SecureMeshMlsOperationState::CryptoPrepared,
            "secure mesh MLS committed operation cannot reset for retry"
        );
        tx.execute(
            "DELETE FROM secure_mesh_mls_operation_reservations WHERE operation_id = ?1",
            params![operation_id],
        )?;
        let changed = tx.execute(
            r#"
            UPDATE secure_mesh_mls_operations
            SET state = 'prepared',
                response_json = NULL,
                group_id_base64url = NULL,
                base_metadata_json = NULL,
                expected_metadata_json = NULL,
                prepared_security_json = NULL,
                updated_at_unix_seconds = ?1
            WHERE operation_id = ?2 AND state = 'crypto_prepared'
            "#,
            params![now_unix_seconds, operation_id],
        )?;
        ensure!(
            changed == 1,
            "secure mesh MLS operation retry reset lost ownership"
        );
        tx.commit()?;
        self.operation(operation_id)?
            .ok_or_else(|| anyhow!("secure mesh MLS reset operation disappeared"))
    }

    pub(crate) fn abort_empty_prepared_operation(&mut self, operation_id: &str) -> Result<bool> {
        let tx = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|error| anyhow!("secure mesh MLS empty operation abort failed: {error}"))?;
        let removed = tx.execute(
            r#"
            DELETE FROM secure_mesh_mls_operations
            WHERE operation_id = ?1
              AND state = 'prepared'
              AND response_json IS NULL
              AND group_id_base64url IS NULL
              AND base_metadata_json IS NULL
              AND expected_metadata_json IS NULL
              AND prepared_security_json IS NULL
              AND NOT EXISTS (
                  SELECT 1 FROM secure_mesh_mls_operation_reservations reservations
                  WHERE reservations.operation_id = secure_mesh_mls_operations.operation_id
              )
            "#,
            params![operation_id],
        )?;
        ensure!(
            removed <= 1,
            "secure mesh MLS empty operation abort affected multiple records"
        );
        tx.commit().map_err(|error| {
            anyhow!("secure mesh MLS empty operation abort commit failed: {error}")
        })?;
        Ok(removed == 1)
    }

    pub(crate) fn commit_operation_crypto(
        &mut self,
        operation_id: &str,
        observed_metadata: &SecureMeshMlsGroupMetadata,
        now_unix_seconds: i64,
    ) -> Result<SecureMeshMlsOperationRecord> {
        let tx = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|error| anyhow!("secure mesh MLS operation crypto commit failed: {error}"))?;
        let (record, _, _) = read_operation_transaction(&tx, operation_id)?
            .ok_or_else(|| anyhow!("secure mesh MLS operation journal entry is missing"))?;
        let expected_metadata = record
            .expected_metadata
            .as_ref()
            .ok_or_else(|| anyhow!("secure mesh MLS operation expected metadata is missing"))?;
        ensure!(
            expected_metadata == observed_metadata,
            "secure mesh MLS operation snapshot does not match prepared crypto state"
        );
        if matches!(
            record.state,
            SecureMeshMlsOperationState::CryptoCommitted
                | SecureMeshMlsOperationState::MetadataReconciled
                | SecureMeshMlsOperationState::Delivered
        ) {
            tx.commit().map_err(|error| {
                anyhow!("secure mesh MLS operation crypto recovery commit failed: {error}")
            })?;
            return Ok(record);
        }
        ensure!(
            record.state == SecureMeshMlsOperationState::CryptoPrepared,
            "secure mesh MLS operation crypto state is not prepared"
        );
        let security_json: String = tx.query_row(
            "SELECT prepared_security_json FROM secure_mesh_mls_operations WHERE operation_id = ?1",
            params![operation_id],
            |row| row.get(0),
        )?;
        let prepared: PreparedMlsSecurityInputs = serde_json::from_str(&security_json)
            .map_err(|_| anyhow!("secure mesh MLS prepared security journal is invalid"))?;
        validate_prepared_security_inputs(&prepared, prepared.consumed_at_unix_seconds)?;
        consume_prepared_security_transaction(&tx, &prepared, now_unix_seconds)?;
        for reservation_key in reservation_keys(&prepared) {
            let removed = tx.execute(
                r#"
                DELETE FROM secure_mesh_mls_operation_reservations
                WHERE operation_id = ?1
                  AND local_endpoint_scope_hash = ?2
                  AND reservation_key = ?3
                "#,
                params![
                    operation_id,
                    prepared.local_endpoint_scope_hash,
                    reservation_key
                ],
            )?;
            ensure!(
                removed == 1,
                "secure mesh MLS operation security reservation is incomplete"
            );
        }
        let changed = tx.execute(
            r#"
            UPDATE secure_mesh_mls_operations
            SET state = 'crypto_committed', updated_at_unix_seconds = ?1
            WHERE operation_id = ?2 AND state = 'crypto_prepared'
            "#,
            params![now_unix_seconds, operation_id],
        )?;
        ensure!(
            changed == 1,
            "secure mesh MLS operation crypto journal commit failed"
        );
        tx.commit()
            .map_err(|error| anyhow!("secure mesh MLS operation crypto commit failed: {error}"))?;
        self.operation(operation_id)?
            .ok_or_else(|| anyhow!("secure mesh MLS committed operation disappeared"))
    }

    pub(crate) fn mark_operation_metadata_reconciled(
        &mut self,
        operation_id: &str,
        final_response: &Value,
        now_unix_seconds: i64,
    ) -> Result<SecureMeshMlsOperationRecord> {
        let response_json = serde_json::to_string(final_response)
            .map_err(|_| anyhow!("secure mesh MLS final response encoding failed"))?;
        let tx = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let (record, _, _) = read_operation_transaction(&tx, operation_id)?
            .ok_or_else(|| anyhow!("secure mesh MLS operation journal entry is missing"))?;
        if matches!(
            record.state,
            SecureMeshMlsOperationState::MetadataReconciled
                | SecureMeshMlsOperationState::Delivered
        ) {
            ensure!(
                record.response.as_ref() == Some(final_response),
                "secure mesh MLS same-state metadata response diverges"
            );
            tx.commit()?;
            return Ok(record);
        }
        ensure!(
            record.state == SecureMeshMlsOperationState::CryptoCommitted,
            "secure mesh MLS metadata journal transition is invalid"
        );
        let writer_reservation_removed = tx.execute(
            r#"
            DELETE FROM secure_mesh_mls_operation_reservations
            WHERE operation_id = ?1 AND reservation_key = 'participant-writer'
            "#,
            params![operation_id],
        )?;
        ensure!(
            writer_reservation_removed == 1,
            "secure mesh MLS participant writer reservation is missing"
        );
        let changed = tx.execute(
            r#"
            UPDATE secure_mesh_mls_operations
            SET state = 'metadata_reconciled', response_json = ?1, updated_at_unix_seconds = ?2
            WHERE operation_id = ?3 AND state = 'crypto_committed'
            "#,
            params![response_json, now_unix_seconds, operation_id],
        )?;
        ensure!(
            changed == 1,
            "secure mesh MLS metadata journal transition lost ownership"
        );
        tx.commit()?;
        self.operation(operation_id)?.ok_or_else(|| {
            anyhow!("secure mesh MLS operation disappeared after metadata reconciliation")
        })
    }

    pub(crate) fn mark_operation_delivered(
        &mut self,
        operation_id: &str,
        now_unix_seconds: i64,
    ) -> Result<SecureMeshMlsOperationRecord> {
        self.advance_operation_state(
            operation_id,
            SecureMeshMlsOperationState::MetadataReconciled,
            SecureMeshMlsOperationState::Delivered,
            now_unix_seconds,
        )
    }

    pub(crate) fn operation(
        &self,
        operation_id: &str,
    ) -> Result<Option<SecureMeshMlsOperationRecord>> {
        read_operation_connection(&self.connection, operation_id)
            .map(|value| value.map(|(record, _, _)| record))
    }

    pub(crate) fn incomplete_writer_operations(
        &self,
        local_identity: &DeviceTrustPublicIdentity,
    ) -> Result<Vec<SecureMeshMlsOperationRecord>> {
        let scope = mls_security_scope_hash(local_identity)?;
        let mut statement = self.connection.prepare(
            r#"
            SELECT operations.operation_id
            FROM secure_mesh_mls_operations operations
            INNER JOIN secure_mesh_mls_operation_reservations reservations
                ON reservations.operation_id = operations.operation_id
            WHERE operations.local_endpoint_scope_hash = ?1
              AND reservations.reservation_key = 'participant-writer'
              AND operations.state IN ('crypto_prepared', 'crypto_committed')
            ORDER BY operations.created_at_unix_seconds, operations.operation_id
            "#,
        )?;
        let ids = statement
            .query_map(params![scope], |row| row.get::<_, String>(0))?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        ids.into_iter()
            .map(|operation_id| {
                self.operation(&operation_id)?.ok_or_else(|| {
                    anyhow!("secure mesh MLS incomplete writer operation disappeared")
                })
            })
            .collect()
    }

    pub(super) fn advance_operation_state(
        &mut self,
        operation_id: &str,
        expected: SecureMeshMlsOperationState,
        next: SecureMeshMlsOperationState,
        now_unix_seconds: i64,
    ) -> Result<SecureMeshMlsOperationRecord> {
        let tx = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let (record, _, _) = read_operation_transaction(&tx, operation_id)?
            .ok_or_else(|| anyhow!("secure mesh MLS operation journal entry is missing"))?;
        if record.state == next || record.state == SecureMeshMlsOperationState::Delivered {
            tx.commit()?;
            return Ok(record);
        }
        ensure!(
            record.state == expected,
            "secure mesh MLS operation journal transition is invalid"
        );
        let changed = tx.execute(
            "UPDATE secure_mesh_mls_operations SET state = ?1, updated_at_unix_seconds = ?2 WHERE operation_id = ?3 AND state = ?4",
            params![next.as_str(), now_unix_seconds, operation_id, expected.as_str()],
        )?;
        ensure!(
            changed == 1,
            "secure mesh MLS operation journal transition lost ownership"
        );
        if next == SecureMeshMlsOperationState::Delivered {
            let scope: String = tx.query_row(
                "SELECT local_endpoint_scope_hash FROM secure_mesh_mls_operations WHERE operation_id = ?1",
                params![operation_id],
                |row| row.get(0),
            )?;
            tx.execute(
                r#"
                DELETE FROM secure_mesh_mls_operations
                WHERE operation_id IN (
                    SELECT operation_id FROM secure_mesh_mls_operations
                    WHERE local_endpoint_scope_hash = ?1
                      AND state = 'delivered'
                      AND operation_id != ?3
                    ORDER BY updated_at_unix_seconds DESC, operation_id DESC
                    LIMIT -1 OFFSET ?2
                )
                "#,
                params![
                    scope,
                    i64::try_from(MAX_DELIVERED_MLS_OPERATIONS_PER_SCOPE)
                        .unwrap_or(i64::MAX)
                        .saturating_sub(1),
                    operation_id,
                ],
            )?;
        }
        tx.commit()?;
        self.operation(operation_id)?
            .ok_or_else(|| anyhow!("secure mesh MLS operation disappeared after transition"))
    }
}

pub(crate) fn prepare_member_add_security_inputs(
    consumer_identity: &DeviceTrustPublicIdentity,
    key_package_id: &str,
    key_package_public_bytes: &[u8],
    group_id_hash: &str,
    first_proof: &SignedCapabilityProof,
    second_proof: &SignedCapabilityProof,
    now_unix_seconds: i64,
) -> Result<PreparedMlsSecurityInputs> {
    ensure!(
        !key_package_id.trim().is_empty(),
        "secure mesh MLS keypackage id is required"
    );
    let mut prepared = prepare_capability_security_inputs(
        consumer_identity,
        first_proof,
        second_proof,
        now_unix_seconds,
    )?;
    prepared.key_package = Some(PreparedMlsKeyPackageUse {
        key_package_id_hash: hex_sha256(key_package_id.as_bytes()),
        key_package_public_key_hash: hex_sha256(key_package_public_bytes),
        group_id_hash: group_id_hash.to_string(),
    });
    Ok(prepared)
}

pub(crate) fn prepare_capability_security_inputs(
    observing_identity: &DeviceTrustPublicIdentity,
    first_proof: &SignedCapabilityProof,
    second_proof: &SignedCapabilityProof,
    now_unix_seconds: i64,
) -> Result<PreparedMlsSecurityInputs> {
    let first = PreparedMlsCapabilityProofUse {
        proof_digest: signed_capability_proof_digest(first_proof)?,
        expires_at_unix_seconds: first_proof.claims.expires_at_unix_seconds,
    };
    let second = PreparedMlsCapabilityProofUse {
        proof_digest: signed_capability_proof_digest(second_proof)?,
        expires_at_unix_seconds: second_proof.claims.expires_at_unix_seconds,
    };
    let prepared = PreparedMlsSecurityInputs {
        local_endpoint_scope_hash: mls_security_scope_hash(observing_identity)?,
        key_package: None,
        capability_proofs: [first, second],
        consumed_at_unix_seconds: now_unix_seconds,
    };
    validate_prepared_security_inputs(&prepared, now_unix_seconds)?;
    Ok(prepared)
}

pub(crate) fn empty_prepared_security_inputs(
    observing_identity: &DeviceTrustPublicIdentity,
    now_unix_seconds: i64,
) -> Result<PreparedMlsSecurityInputs> {
    let placeholder = PreparedMlsCapabilityProofUse {
        proof_digest: format!(
            "none:{}",
            hex_sha256(b"secure-mesh-mls-no-capability-update")
        ),
        expires_at_unix_seconds: i64::MAX,
    };
    Ok(PreparedMlsSecurityInputs {
        local_endpoint_scope_hash: mls_security_scope_hash(observing_identity)?,
        key_package: None,
        capability_proofs: [placeholder.clone(), placeholder],
        consumed_at_unix_seconds: now_unix_seconds,
    })
}
