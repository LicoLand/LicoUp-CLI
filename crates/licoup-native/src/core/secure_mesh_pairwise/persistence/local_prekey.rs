use anyhow::{Context, Result, ensure};
use rusqlite::{Transaction, params};

use super::super::support::require_text;
use super::{super::key_ratchet::SecureMeshPairwiseSession, store_model::SecureMeshLocalPreKeyUse};

pub(super) struct PreparedLocalPreKeyUse {
    local_endpoint_id: String,
    local_identity_fingerprint: String,
    one_time_prekey_id: String,
    one_time_prekey_public_key_hash: String,
    one_time_mlkem1024_prekey_id: String,
    one_time_mlkem1024_prekey_public_key_hash: String,
    session_id: String,
    used_at: String,
}

impl PreparedLocalPreKeyUse {
    pub(super) fn new(
        prekey_use: &SecureMeshLocalPreKeyUse,
        session: &SecureMeshPairwiseSession,
        used_at: String,
    ) -> Result<Self> {
        ensure!(
            prekey_use.local_endpoint_id == session.local_endpoint_id,
            "secure mesh pairwise local prekey claim endpoint mismatch"
        );
        Ok(Self {
            local_endpoint_id: require_text(
                prekey_use.local_endpoint_id.clone(),
                "local prekey endpoint id",
            )?,
            local_identity_fingerprint: require_text(
                prekey_use.local_identity_fingerprint.clone(),
                "local prekey identity fingerprint",
            )?,
            one_time_prekey_id: require_text(
                prekey_use.one_time_prekey_id.clone(),
                "local one-time prekey id",
            )?,
            one_time_prekey_public_key_hash: require_text(
                prekey_use.one_time_prekey_public_key_hash.clone(),
                "local one-time prekey public key hash",
            )?,
            one_time_mlkem1024_prekey_id: require_text(
                prekey_use.one_time_mlkem1024_prekey_id.clone(),
                "local one-time ML-KEM-1024 prekey id",
            )?,
            one_time_mlkem1024_prekey_public_key_hash: require_text(
                prekey_use.one_time_mlkem1024_prekey_public_key_hash.clone(),
                "local one-time ML-KEM-1024 prekey public key hash",
            )?,
            session_id: session.session_id.clone(),
            used_at,
        })
    }
}

pub(super) fn consume_local_prekey_use(
    tx: &Transaction<'_>,
    prekey_use: &PreparedLocalPreKeyUse,
) -> Result<()> {
    let changed = tx
        .execute(
            r#"
            INSERT OR IGNORE INTO secure_mesh_pairwise_local_prekey_uses (
                local_endpoint_id,
                local_identity_fingerprint,
                one_time_prekey_id,
                one_time_prekey_public_key_hash,
                one_time_mlkem1024_prekey_id,
                one_time_mlkem1024_prekey_public_key_hash,
                session_id,
                used_at
            ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
            "#,
            params![
                prekey_use.local_endpoint_id,
                prekey_use.local_identity_fingerprint,
                prekey_use.one_time_prekey_id,
                prekey_use.one_time_prekey_public_key_hash,
                prekey_use.one_time_mlkem1024_prekey_id,
                prekey_use.one_time_mlkem1024_prekey_public_key_hash,
                prekey_use.session_id,
                prekey_use.used_at,
            ],
        )
        .context("secure mesh pairwise local one-time prekey claim failed")?;
    ensure!(
        changed == 1,
        "secure mesh pairwise local one-time prekey was already consumed"
    );
    Ok(())
}
