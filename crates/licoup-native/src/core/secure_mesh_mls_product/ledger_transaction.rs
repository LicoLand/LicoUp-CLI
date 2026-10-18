use super::constants::{
    MAX_PERSISTED_MLS_CAPABILITY_PROOFS, MAX_PERSISTED_MLS_KEY_PACKAGES_PER_SCOPE,
};
use super::helpers::{append_len_prefixed, hex_sha256};
use super::identity_trust::mls_credential_identity_bytes;
use super::security_ledger::{
    PreparedMlsSecurityInputs, SecureMeshMlsOperationRecord, SecureMeshMlsOperationState,
};
use anyhow::{Result, anyhow, ensure};
use base64::{Engine as _, engine::general_purpose};
use rusqlite::{Connection, OptionalExtension, Transaction, params};

use crate::core::secure_mesh_trust::DeviceTrustPublicIdentity;

pub(super) fn validate_operation_identity(
    operation_id: &str,
    action: &str,
    request_digest: &str,
) -> Result<()> {
    for (label, value) in [
        ("operation id", operation_id),
        ("request digest", request_digest),
    ] {
        ensure!(
            value.len() == 64 && value.bytes().all(|byte| byte.is_ascii_hexdigit()),
            "secure mesh MLS {label} is invalid"
        );
    }
    ensure!(
        matches!(
            action,
            "secure_mesh.mls.member.add"
                | "secure_mesh.mls.member.remove"
                | "secure_mesh.mls.group.join"
                | "secure_mesh.mls.commit.process"
        ),
        "secure mesh MLS journal action is invalid"
    );
    Ok(())
}

pub(super) fn validate_prepared_security_inputs(
    prepared: &PreparedMlsSecurityInputs,
    now_unix_seconds: i64,
) -> Result<()> {
    ensure!(
        prepared.local_endpoint_scope_hash.len() == 64,
        "secure mesh MLS prepared security scope is invalid"
    );
    let [first, second] = &prepared.capability_proofs;
    let no_capability_update = first.proof_digest.starts_with("none:")
        && first == second
        && prepared.key_package.is_none();
    if no_capability_update {
        return Ok(());
    }
    ensure!(
        first.proof_digest != second.proof_digest,
        "secure mesh MLS prepared capability proofs must be distinct"
    );
    ensure!(
        first.expires_at_unix_seconds >= now_unix_seconds
            && second.expires_at_unix_seconds >= now_unix_seconds,
        "secure mesh MLS prepared capability proof is expired"
    );
    if let Some(key_package) = &prepared.key_package {
        ensure!(
            !key_package.group_id_hash.is_empty()
                && key_package.key_package_id_hash.len() == 64
                && key_package.key_package_public_key_hash.len() == 64,
            "secure mesh MLS prepared keypackage input is invalid"
        );
    }
    Ok(())
}

pub(super) fn reservation_keys(prepared: &PreparedMlsSecurityInputs) -> Vec<String> {
    let mut keys = Vec::new();
    if let Some(key_package) = &prepared.key_package {
        keys.push(format!("keypackage-id:{}", key_package.key_package_id_hash));
        keys.push(format!(
            "keypackage-public:{}",
            key_package.key_package_public_key_hash
        ));
    }
    if !prepared.capability_proofs[0]
        .proof_digest
        .starts_with("none:")
    {
        for proof in &prepared.capability_proofs {
            keys.push(format!("capability-proof:{}", proof.proof_digest));
        }
    }
    keys
}

pub(super) fn reserve_prepared_security_transaction(
    tx: &Transaction<'_>,
    operation_id: &str,
    prepared: &PreparedMlsSecurityInputs,
) -> Result<()> {
    let effective_now_unix_seconds =
        advance_mls_replay_time_watermark(tx, prepared.consumed_at_unix_seconds)?;
    validate_prepared_security_inputs(prepared, effective_now_unix_seconds)?;
    tx.execute(
        "DELETE FROM secure_mesh_mls_capability_proof_uses WHERE expires_at_unix_seconds < ?1",
        params![effective_now_unix_seconds],
    )?;
    if !prepared.capability_proofs[0]
        .proof_digest
        .starts_with("none:")
    {
        let used: i64 = tx.query_row(
            "SELECT COUNT(*) FROM secure_mesh_mls_capability_proof_uses WHERE local_endpoint_scope_hash = ?1",
            params![prepared.local_endpoint_scope_hash],
            |row| row.get(0),
        )?;
        let reserved: i64 = tx.query_row(
            r#"
            SELECT COUNT(*) FROM secure_mesh_mls_operation_reservations
            WHERE local_endpoint_scope_hash = ?1
              AND reservation_key LIKE 'capability-proof:%'
              AND operation_id != ?2
            "#,
            params![prepared.local_endpoint_scope_hash, operation_id],
            |row| row.get(0),
        )?;
        ensure!(
            usize::try_from(used.saturating_add(reserved))
                .unwrap_or(usize::MAX)
                .saturating_add(2)
                <= MAX_PERSISTED_MLS_CAPABILITY_PROOFS,
            "secure mesh MLS capability replay guard is at capacity"
        );
    }
    if let Some(key_package) = &prepared.key_package {
        let used_count: i64 = tx.query_row(
            "SELECT COUNT(*) FROM secure_mesh_mls_keypackage_uses WHERE consumer_endpoint_id = ?1",
            params![prepared.local_endpoint_scope_hash],
            |row| row.get(0),
        )?;
        let reserved_count: i64 = tx.query_row(
            r#"
            SELECT COUNT(*) FROM secure_mesh_mls_operation_reservations
            WHERE local_endpoint_scope_hash = ?1
              AND reservation_key LIKE 'keypackage-id:%'
              AND operation_id != ?2
            "#,
            params![prepared.local_endpoint_scope_hash, operation_id],
            |row| row.get(0),
        )?;
        ensure!(
            usize::try_from(used_count.saturating_add(reserved_count)).unwrap_or(usize::MAX)
                < MAX_PERSISTED_MLS_KEY_PACKAGES_PER_SCOPE,
            "secure mesh MLS keypackage replay guard is at capacity"
        );
        let existing: i64 = tx.query_row(
            r#"
            SELECT COUNT(*) FROM secure_mesh_mls_keypackage_uses
            WHERE consumer_endpoint_id = ?1
              AND (key_package_id = ?2 OR key_package_public_key_hash = ?3)
            "#,
            params![
                prepared.local_endpoint_scope_hash,
                key_package.key_package_id_hash,
                key_package.key_package_public_key_hash
            ],
            |row| row.get(0),
        )?;
        ensure!(
            existing == 0,
            "secure mesh MLS keypackage was already consumed"
        );
    }
    for proof in &prepared.capability_proofs {
        if proof.proof_digest.starts_with("none:") {
            continue;
        }
        let existing: i64 = tx.query_row(
            r#"
            SELECT COUNT(*) FROM secure_mesh_mls_capability_proof_uses
            WHERE local_endpoint_scope_hash = ?1 AND proof_digest = ?2
            "#,
            params![prepared.local_endpoint_scope_hash, proof.proof_digest],
            |row| row.get(0),
        )?;
        ensure!(
            existing == 0,
            "secure mesh MLS capability proof replay rejected"
        );
    }
    for reservation_key in reservation_keys(prepared) {
        reserve_operation_key_transaction(
            tx,
            operation_id,
            &prepared.local_endpoint_scope_hash,
            &reservation_key,
        )?;
    }
    Ok(())
}

pub(super) fn reserve_operation_key_transaction(
    tx: &Transaction<'_>,
    operation_id: &str,
    local_scope_hash: &str,
    reservation_key: &str,
) -> Result<()> {
    tx.execute(
        r#"
        INSERT OR IGNORE INTO secure_mesh_mls_operation_reservations (
            local_endpoint_scope_hash, reservation_key, operation_id
        ) VALUES (?1, ?2, ?3)
        "#,
        params![local_scope_hash, reservation_key, operation_id],
    )?;
    let owner: String = tx.query_row(
        r#"
        SELECT operation_id FROM secure_mesh_mls_operation_reservations
        WHERE local_endpoint_scope_hash = ?1 AND reservation_key = ?2
        "#,
        params![local_scope_hash, reservation_key],
        |row| row.get(0),
    )?;
    ensure!(
        owner == operation_id,
        "secure mesh MLS operation input is reserved by another writer"
    );
    Ok(())
}

pub(super) fn consume_prepared_security_transaction(
    tx: &Transaction<'_>,
    prepared: &PreparedMlsSecurityInputs,
    now_unix_seconds: i64,
) -> Result<()> {
    let effective_now_unix_seconds = advance_mls_replay_time_watermark(tx, now_unix_seconds)?;
    validate_prepared_security_inputs(prepared, effective_now_unix_seconds)
        .map_err(|_| anyhow!("secure mesh MLS capability proof revived by clock rollback"))?;
    if let Some(key_package) = &prepared.key_package {
        consume_key_package_in_transaction(
            tx,
            &prepared.local_endpoint_scope_hash,
            &key_package.key_package_id_hash,
            &key_package.key_package_public_key_hash,
            &key_package.group_id_hash,
            prepared.consumed_at_unix_seconds,
        )?;
    }
    if prepared.capability_proofs[0]
        .proof_digest
        .starts_with("none:")
    {
        return Ok(());
    }
    tx.execute(
        "DELETE FROM secure_mesh_mls_capability_proof_uses WHERE expires_at_unix_seconds < ?1",
        params![effective_now_unix_seconds],
    )?;
    let unexpired_count: i64 = tx.query_row(
        r#"
        SELECT COUNT(*) FROM secure_mesh_mls_capability_proof_uses
        WHERE local_endpoint_scope_hash = ?1
        "#,
        params![prepared.local_endpoint_scope_hash],
        |row| row.get(0),
    )?;
    ensure!(
        usize::try_from(unexpired_count)
            .unwrap_or(usize::MAX)
            .saturating_add(2)
            <= MAX_PERSISTED_MLS_CAPABILITY_PROOFS,
        "secure mesh MLS capability replay guard is at capacity"
    );
    for proof in &prepared.capability_proofs {
        tx.execute(
            r#"
            INSERT INTO secure_mesh_mls_capability_proof_uses (
                local_endpoint_scope_hash,
                proof_digest,
                expires_at_unix_seconds,
                consumed_at_unix_seconds
            ) VALUES (?1, ?2, ?3, ?4)
            "#,
            params![
                prepared.local_endpoint_scope_hash,
                proof.proof_digest,
                proof.expires_at_unix_seconds,
                prepared.consumed_at_unix_seconds
            ],
        )?;
    }
    Ok(())
}

pub(super) fn advance_mls_replay_time_watermark(
    tx: &Transaction<'_>,
    now_unix_seconds: i64,
) -> Result<i64> {
    ensure!(
        now_unix_seconds >= 0,
        "secure mesh MLS replay clock is before unix epoch"
    );
    let persisted: i64 = tx.query_row(
        "SELECT max_observed_unix_seconds FROM secure_mesh_mls_time_guard WHERE singleton = 1",
        [],
        |row| row.get(0),
    )?;
    let effective = persisted.max(now_unix_seconds);
    tx.execute(
        "UPDATE secure_mesh_mls_time_guard SET max_observed_unix_seconds = ?1 WHERE singleton = 1",
        params![effective],
    )?;
    Ok(effective)
}

pub(super) fn read_operation_connection(
    connection: &Connection,
    operation_id: &str,
) -> Result<Option<(SecureMeshMlsOperationRecord, String, String)>> {
    connection
        .query_row(
            r#"
            SELECT action, request_digest, state, response_json, group_id_base64url,
                   base_metadata_json, expected_metadata_json
            FROM secure_mesh_mls_operations WHERE operation_id = ?1
            "#,
            params![operation_id],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, Option<String>>(3)?,
                    row.get::<_, Option<String>>(4)?,
                    row.get::<_, Option<String>>(5)?,
                    row.get::<_, Option<String>>(6)?,
                ))
            },
        )
        .optional()?
        .map(
            |(
                action,
                request_digest,
                state,
                response_json,
                group_id_base64url,
                base_metadata_json,
                metadata_json,
            )|
             -> Result<_> {
                let state = SecureMeshMlsOperationState::parse(&state)?;
                let response = response_json
                    .map(|json| {
                        serde_json::from_str(&json).map_err(|_| {
                            anyhow!("secure mesh MLS operation response journal is invalid")
                        })
                    })
                    .transpose()?;
                let expected_metadata = metadata_json
                    .map(|json| {
                        serde_json::from_str(&json).map_err(|_| {
                            anyhow!("secure mesh MLS operation metadata journal is invalid")
                        })
                    })
                    .transpose()?;
                let group_id = group_id_base64url
                    .map(|value| {
                        general_purpose::URL_SAFE_NO_PAD.decode(value).map_err(|_| {
                            anyhow!("secure mesh MLS operation group id journal is invalid")
                        })
                    })
                    .transpose()?;
                let base_metadata = base_metadata_json
                    .map(|json| {
                        serde_json::from_str(&json).map_err(|_| {
                            anyhow!("secure mesh MLS operation base metadata journal is invalid")
                        })
                    })
                    .transpose()?;
                Ok((
                    SecureMeshMlsOperationRecord {
                        operation_id: operation_id.to_string(),
                        action: action.clone(),
                        state,
                        response,
                        group_id,
                        base_metadata,
                        expected_metadata,
                    },
                    action,
                    request_digest,
                ))
            },
        )
        .transpose()
}

pub(super) fn read_operation_transaction(
    tx: &Transaction<'_>,
    operation_id: &str,
) -> Result<Option<(SecureMeshMlsOperationRecord, String, String)>> {
    read_operation_connection(tx, operation_id)
}

pub(super) fn mls_security_scope_hash(identity: &DeviceTrustPublicIdentity) -> Result<String> {
    let mut scope = Vec::new();
    scope.extend_from_slice(b"LICO-SM-MLS-SECURITY-LEDGER-SCOPE-v1");
    append_len_prefixed(&mut scope, &mls_credential_identity_bytes(identity)?)?;
    append_len_prefixed(&mut scope, &identity.signing_public_key)?;
    Ok(hex_sha256(&scope))
}

pub(super) fn consume_key_package_in_transaction(
    tx: &Transaction<'_>,
    consumer_scope_hash: &str,
    key_package_id_hash: &str,
    public_key_hash: &str,
    group_id_hash: &str,
    now_unix_seconds: i64,
) -> Result<()> {
    let changed = tx.execute(
        r#"
        INSERT OR IGNORE INTO secure_mesh_mls_keypackage_uses (
            consumer_endpoint_id,
            key_package_id,
            key_package_public_key_hash,
            group_id_hash,
            used_at
        ) VALUES (?1, ?2, ?3, ?4, ?5)
        "#,
        params![
            consumer_scope_hash,
            key_package_id_hash,
            public_key_hash,
            group_id_hash,
            now_unix_seconds.to_string(),
        ],
    )?;
    ensure!(
        changed == 1,
        "secure mesh MLS keypackage was already consumed"
    );
    Ok(())
}
