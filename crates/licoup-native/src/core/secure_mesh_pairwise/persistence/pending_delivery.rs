use anyhow::{Context, Result, ensure};
use rusqlite::{OptionalExtension, Transaction, params};
use time::{OffsetDateTime, format_description::well_known::Rfc3339};

use super::store_model::{SecureMeshPairwiseDurableStore, SecureMeshPairwisePendingDelivery};

const MAX_PENDING_ENVELOPE_JSON_BYTES: usize = 1_114_112;
const MAX_PENDING_BINDING_JSON_BYTES: usize = 4 * 1024;
const MAX_PENDING_ID_BYTES: usize = 128;
const MAX_PENDING_KIND_BYTES: usize = 64;

impl SecureMeshPairwiseDurableStore {
    pub fn read_pending_delivery(
        &self,
        session_id: &str,
        local_endpoint_id: &str,
        delivery_kind: &str,
    ) -> Result<Option<SecureMeshPairwisePendingDelivery>> {
        self.connection
            .query_row(
                r#"
                SELECT delivery_kind, envelope_id, expires_at, envelope_json, binding_json,
                       created_at
                FROM secure_mesh_pairwise_pending_deliveries
                WHERE session_id = ?1 AND local_endpoint_id = ?2 AND delivery_kind = ?3
                "#,
                params![session_id, local_endpoint_id, delivery_kind],
                |row| {
                    Ok(SecureMeshPairwisePendingDelivery {
                        delivery_kind: row.get(0)?,
                        envelope_id: row.get(1)?,
                        expires_at: row.get(2)?,
                        envelope_json: row.get(3)?,
                        binding_json: row.get(4)?,
                        created_at: row.get(5)?,
                    })
                },
            )
            .optional()
            .context("secure mesh pairwise pending delivery read failed")
    }

    pub fn delete_pending_delivery(
        &mut self,
        session_id: &str,
        local_endpoint_id: &str,
        delivery_kind: &str,
        envelope_id: &str,
    ) -> Result<bool> {
        let changed = self
            .connection
            .execute(
                r#"
                DELETE FROM secure_mesh_pairwise_pending_deliveries
                WHERE session_id = ?1
                  AND local_endpoint_id = ?2
                  AND delivery_kind = ?3
                  AND envelope_id = ?4
                "#,
                params![session_id, local_endpoint_id, delivery_kind, envelope_id],
            )
            .context("secure mesh pairwise pending delivery delete failed")?;
        ensure!(
            changed <= 1,
            "secure mesh pairwise pending delivery delete was not bounded"
        );
        Ok(changed == 1)
    }
}

pub(super) fn insert_pending_delivery(
    tx: &Transaction<'_>,
    session_id: &str,
    local_endpoint_id: &str,
    delivery: &SecureMeshPairwisePendingDelivery,
) -> Result<()> {
    validate_pending_delivery(delivery)?;
    tx.execute(
        r#"
        INSERT INTO secure_mesh_pairwise_pending_deliveries (
            session_id,
            local_endpoint_id,
            delivery_kind,
            envelope_id,
            expires_at,
            envelope_json,
            binding_json,
            created_at
        ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
        "#,
        params![
            session_id,
            local_endpoint_id,
            delivery.delivery_kind,
            delivery.envelope_id,
            delivery.expires_at,
            delivery.envelope_json,
            delivery.binding_json,
            delivery.created_at,
        ],
    )
    .context("secure mesh pairwise pending delivery insert failed")?;
    Ok(())
}

fn validate_pending_delivery(delivery: &SecureMeshPairwisePendingDelivery) -> Result<()> {
    ensure!(
        !delivery.delivery_kind.is_empty()
            && delivery.delivery_kind.len() <= MAX_PENDING_KIND_BYTES
            && delivery
                .delivery_kind
                .bytes()
                .all(|byte| byte.is_ascii_lowercase() || byte == b'_'),
        "secure mesh pairwise pending delivery kind is invalid"
    );
    ensure!(
        !delivery.envelope_id.is_empty()
            && delivery.envelope_id.len() <= MAX_PENDING_ID_BYTES
            && delivery
                .envelope_id
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_' || byte == b'-'),
        "secure mesh pairwise pending delivery ID is invalid"
    );
    ensure!(
        !delivery.envelope_json.is_empty()
            && delivery.envelope_json.len() <= MAX_PENDING_ENVELOPE_JSON_BYTES
            && serde_json::from_str::<serde_json::Value>(&delivery.envelope_json)
                .is_ok_and(|value| value.is_object()),
        "secure mesh pairwise pending envelope JSON is invalid"
    );
    ensure!(
        !delivery.binding_json.is_empty()
            && delivery.binding_json.len() <= MAX_PENDING_BINDING_JSON_BYTES
            && serde_json::from_str::<serde_json::Value>(&delivery.binding_json)
                .is_ok_and(|value| value.is_object()),
        "secure mesh pairwise pending binding JSON is invalid"
    );
    ensure!(
        OffsetDateTime::parse(&delivery.expires_at, &Rfc3339).is_ok()
            && OffsetDateTime::parse(&delivery.created_at, &Rfc3339).is_ok(),
        "secure mesh pairwise pending delivery timestamps are invalid"
    );
    Ok(())
}
