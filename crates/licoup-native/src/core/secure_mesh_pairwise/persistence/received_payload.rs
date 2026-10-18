use anyhow::{Context, Result, anyhow, ensure};
use rusqlite::{OptionalExtension, Transaction, params};
use serde::{Deserialize, Serialize};
use time::{OffsetDateTime, format_description::well_known::Rfc3339};
use uuid::Uuid;

use super::super::support::{append_len_prefixed_bytes, sha256_hex};
use super::store_model::{SecureMeshPairwiseDurableStore, SecureMeshPairwiseReceivedPayload};
use crate::core::secure_mesh_secret_store::{
    SecretBytes, SecretStoreAuthorizationSession, SecretStoreHandle,
};

const RECEIVED_PAYLOAD_SECRET_SCHEMA: &str = "secureMesh.pairwise.receivedPayload.v1";
const MAX_RECEIVED_PAYLOAD_JSON_BYTES: usize = 896 * 1024;
const MAX_RECEIVED_ID_BYTES: usize = 128;
const MAX_RECEIVED_BINDING_DIGEST_BYTES: usize = 96;

pub(super) struct PreparedReceivedPayload {
    pub(super) payload: SecureMeshPairwiseReceivedPayload,
    pub(super) secret_handle: SecretStoreHandle,
    pub(super) secret_store_session: SecretStoreAuthorizationSession,
}

#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ReceivedPayloadSecret {
    schema: String,
    session_id: String,
    local_endpoint_id: String,
    receipt_id: String,
    binding_digest: String,
    mailbox_id: String,
    payload_json: String,
    received_at: String,
}

impl SecureMeshPairwiseDurableStore {
    pub(super) fn prepare_received_payload(
        &self,
        session_id: &str,
        local_endpoint_id: &str,
        payload: &SecureMeshPairwiseReceivedPayload,
        secret_store_session: &SecretStoreAuthorizationSession,
    ) -> Result<PreparedReceivedPayload> {
        validate_received_payload(payload)?;
        let secret_store_key =
            received_payload_secret_key(session_id, local_endpoint_id, &payload.receipt_id);
        let secret_handle =
            self.secret_snapshot_handle(&self.secret_store_namespace, &secret_store_key)?;
        let secret = ReceivedPayloadSecret {
            schema: RECEIVED_PAYLOAD_SECRET_SCHEMA.to_string(),
            session_id: session_id.to_string(),
            local_endpoint_id: local_endpoint_id.to_string(),
            receipt_id: payload.receipt_id.clone(),
            binding_digest: payload.binding_digest.clone(),
            mailbox_id: payload.mailbox_id.clone(),
            payload_json: payload.payload_json.clone(),
            received_at: payload.received_at.clone(),
        };
        let encoded = serde_json::to_string(&secret)
            .context("secure mesh received payload secret serialization failed")?;
        self.secret_store
            .set_secret_with_session(
                secret_store_session,
                &secret_handle,
                SecretBytes::try_from_string(encoded)?,
            )
            .context("secure mesh received payload secret write failed")?;
        Ok(PreparedReceivedPayload {
            payload: payload.clone(),
            secret_handle,
            secret_store_session: secret_store_session.clone(),
        })
    }

    pub fn read_received_payload_with_authorized_session(
        &self,
        session_id: &str,
        local_endpoint_id: &str,
        binding_digest: &str,
        secret_store_session: &SecretStoreAuthorizationSession,
    ) -> Result<Option<SecureMeshPairwiseReceivedPayload>> {
        let row = self
            .connection
            .query_row(
                r#"
                SELECT receipt_id, mailbox_id, secret_store_namespace, secret_store_key,
                       received_at
                FROM secure_mesh_pairwise_received_payloads
                WHERE session_id = ?1
                  AND local_endpoint_id = ?2
                  AND binding_digest = ?3
                "#,
                params![session_id, local_endpoint_id, binding_digest],
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
            .optional()
            .context("secure mesh received payload lookup failed")?;
        let Some((receipt_id, mailbox_id, namespace, key, received_at)) = row else {
            return Ok(None);
        };
        ensure!(
            namespace == self.secret_store_namespace && key.starts_with("received.v1."),
            "secure mesh received payload secret binding is invalid"
        );
        let handle = self.secret_snapshot_handle(&namespace, &key)?;
        let encoded = self
            .secret_store
            .get_secret_with_session(secret_store_session, &handle)
            .context("secure mesh received payload secret read failed")?
            .ok_or_else(|| anyhow!("secure mesh received payload secret is unavailable"))?;
        let secret: ReceivedPayloadSecret = serde_json::from_slice(encoded.expose_bytes())
            .context("secure mesh received payload secret is invalid")?;
        ensure!(
            secret.schema == RECEIVED_PAYLOAD_SECRET_SCHEMA
                && secret.session_id == session_id
                && secret.local_endpoint_id == local_endpoint_id
                && secret.receipt_id == receipt_id
                && secret.binding_digest == binding_digest
                && secret.mailbox_id == mailbox_id
                && secret.received_at == received_at,
            "secure mesh received payload secret does not match its durable binding"
        );
        let payload = SecureMeshPairwiseReceivedPayload {
            receipt_id,
            binding_digest: binding_digest.to_string(),
            mailbox_id,
            payload_json: secret.payload_json,
            received_at,
        };
        validate_received_payload(&payload)?;
        Ok(Some(payload))
    }

    pub fn delete_received_payload_with_authorized_session(
        &mut self,
        session_id: &str,
        local_endpoint_id: &str,
        receipt_id: &str,
        secret_store_session: &SecretStoreAuthorizationSession,
    ) -> Result<bool> {
        let key = self
            .connection
            .query_row(
                r#"
                SELECT secret_store_key
                FROM secure_mesh_pairwise_received_payloads
                WHERE session_id = ?1 AND local_endpoint_id = ?2 AND receipt_id = ?3
                "#,
                params![session_id, local_endpoint_id, receipt_id],
                |row| row.get::<_, String>(0),
            )
            .optional()?;
        let Some(key) = key else {
            return Ok(false);
        };
        let handle = self.secret_snapshot_handle(&self.secret_store_namespace, &key)?;
        let changed = self.connection.execute(
            r#"
            DELETE FROM secure_mesh_pairwise_received_payloads
            WHERE session_id = ?1 AND local_endpoint_id = ?2 AND receipt_id = ?3
            "#,
            params![session_id, local_endpoint_id, receipt_id],
        )?;
        ensure!(
            changed == 1,
            "secure mesh received payload delete was not exact"
        );
        let _ = self.delete_secret_or_enqueue_cleanup(secret_store_session, &handle);
        Ok(true)
    }
}

pub(super) fn cleanup_prepared_received_payload(
    store: &SecureMeshPairwiseDurableStore,
    prepared: &PreparedReceivedPayload,
) -> Result<()> {
    store.delete_secret_or_enqueue_cleanup(&prepared.secret_store_session, &prepared.secret_handle)
}

pub(super) fn insert_received_payload(
    tx: &Transaction<'_>,
    session_id: &str,
    local_endpoint_id: &str,
    prepared: &PreparedReceivedPayload,
) -> Result<()> {
    tx.execute(
        r#"
        INSERT INTO secure_mesh_pairwise_received_payloads (
            session_id,
            local_endpoint_id,
            receipt_id,
            binding_digest,
            mailbox_id,
            secret_store_namespace,
            secret_store_key,
            received_at
        ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
        "#,
        params![
            session_id,
            local_endpoint_id,
            prepared.payload.receipt_id,
            prepared.payload.binding_digest,
            prepared.payload.mailbox_id,
            prepared.secret_handle.namespace(),
            prepared.secret_handle.key(),
            prepared.payload.received_at,
        ],
    )
    .context("secure mesh received payload durable insert failed")?;
    Ok(())
}

fn validate_received_payload(payload: &SecureMeshPairwiseReceivedPayload) -> Result<()> {
    ensure!(
        !payload.receipt_id.is_empty()
            && payload.receipt_id.len() <= MAX_RECEIVED_ID_BYTES
            && payload
                .receipt_id
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_' || byte == b'-'),
        "secure mesh received payload receipt ID is invalid"
    );
    ensure!(
        !payload.binding_digest.is_empty()
            && payload.binding_digest.len() <= MAX_RECEIVED_BINDING_DIGEST_BYTES
            && payload
                .binding_digest
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_' || byte == b'-'),
        "secure mesh received payload binding digest is invalid"
    );
    ensure!(
        !payload.mailbox_id.is_empty() && payload.mailbox_id.len() <= MAX_RECEIVED_ID_BYTES,
        "secure mesh received payload mailbox ID is invalid"
    );
    ensure!(
        !payload.payload_json.is_empty()
            && payload.payload_json.len() <= MAX_RECEIVED_PAYLOAD_JSON_BYTES
            && serde_json::from_str::<serde_json::Value>(&payload.payload_json)
                .is_ok_and(|value| value.is_object()),
        "secure mesh received payload JSON is invalid"
    );
    ensure!(
        OffsetDateTime::parse(&payload.received_at, &Rfc3339).is_ok(),
        "secure mesh received payload timestamp is invalid"
    );
    Ok(())
}

fn received_payload_secret_key(
    session_id: &str,
    local_endpoint_id: &str,
    receipt_id: &str,
) -> String {
    let mut material = Vec::new();
    material.extend_from_slice(b"LCOSM-PAIRWISE-RECEIVED-PAYLOAD-v1");
    let _ = append_len_prefixed_bytes(&mut material, session_id.as_bytes());
    let _ = append_len_prefixed_bytes(&mut material, local_endpoint_id.as_bytes());
    let _ = append_len_prefixed_bytes(&mut material, receipt_id.as_bytes());
    format!(
        "received.v1.{}.{}",
        sha256_hex(&material),
        Uuid::new_v4().simple()
    )
}
