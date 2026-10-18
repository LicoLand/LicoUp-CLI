use anyhow::{Context, Result, ensure};
use rusqlite::{Transaction, params};

use super::super::support::{require_sha256_hex, require_text};
use super::store_model::{SecureMeshPairwiseDurableStore, SecureMeshRemotePreKeyUse};

pub(super) struct PreparedRemotePreKeyUse {
    pub(super) session_id: String,
    pub(super) local_endpoint_id: String,
    pub(super) remote_endpoint_id: String,
    pub(super) remote_identity_fingerprint: String,
    pub(super) signed_prekey_id: String,
    pub(super) one_time_prekey_id: String,
    pub(super) one_time_prekey_public_key_hash: String,
    pub(super) one_time_mlkem1024_prekey_id: String,
    pub(super) one_time_mlkem1024_prekey_public_key_hash: String,
    pub(super) directory_authorization_digest: String,
    pub(super) used_at: String,
}

impl PreparedRemotePreKeyUse {
    pub(super) fn new(prekey_use: &SecureMeshRemotePreKeyUse, used_at: String) -> Result<Self> {
        Ok(Self {
            session_id: require_text(prekey_use.session_id.clone(), "session_id")?,
            local_endpoint_id: require_text(
                prekey_use.local_endpoint_id.clone(),
                "local_endpoint_id",
            )?,
            remote_endpoint_id: require_text(
                prekey_use.remote_endpoint_id.clone(),
                "remote_endpoint_id",
            )?,
            remote_identity_fingerprint: require_text(
                prekey_use.remote_identity_fingerprint.clone(),
                "remote_identity_fingerprint",
            )?,
            signed_prekey_id: require_text(
                prekey_use.signed_prekey_id.clone(),
                "signed_prekey_id",
            )?,
            one_time_prekey_id: require_text(
                prekey_use.one_time_prekey_id.clone(),
                "one_time_prekey_id",
            )?,
            one_time_prekey_public_key_hash: require_text(
                prekey_use.one_time_prekey_public_key_hash.clone(),
                "one_time_prekey_public_key_hash",
            )?,
            one_time_mlkem1024_prekey_id: require_text(
                prekey_use.one_time_mlkem1024_prekey_id.clone(),
                "one_time_mlkem1024_prekey_id",
            )?,
            one_time_mlkem1024_prekey_public_key_hash: require_text(
                prekey_use.one_time_mlkem1024_prekey_public_key_hash.clone(),
                "one_time_mlkem1024_prekey_public_key_hash",
            )?,
            directory_authorization_digest: require_sha256_hex(
                prekey_use.directory_authorization_digest.clone(),
                "directory_authorization_digest",
            )?,
            used_at: require_text(used_at, "used_at")?,
        })
    }
}

pub(super) fn consume_remote_prekey_use(
    tx: &Transaction<'_>,
    prekey_use: &PreparedRemotePreKeyUse,
) -> Result<()> {
    let changed = tx
        .execute(
            r#"
            INSERT OR IGNORE INTO secure_mesh_pairwise_remote_prekey_uses (
                remote_endpoint_id,
                remote_identity_fingerprint,
                signed_prekey_id,
                one_time_prekey_id,
                one_time_prekey_public_key_hash,
                one_time_mlkem1024_prekey_id,
                one_time_mlkem1024_prekey_public_key_hash,
                directory_authorization_digest,
                session_id,
                local_endpoint_id,
                used_at
            ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)
            "#,
            params![
                prekey_use.remote_endpoint_id,
                prekey_use.remote_identity_fingerprint,
                prekey_use.signed_prekey_id,
                prekey_use.one_time_prekey_id,
                prekey_use.one_time_prekey_public_key_hash,
                prekey_use.one_time_mlkem1024_prekey_id,
                prekey_use.one_time_mlkem1024_prekey_public_key_hash,
                prekey_use.directory_authorization_digest,
                prekey_use.session_id,
                prekey_use.local_endpoint_id,
                prekey_use.used_at
            ],
        )
        .context("secure mesh pairwise remote prekey-use insert failed")?;
    ensure!(
        changed == 1,
        "secure mesh pairwise remote one-time prekey was already used"
    );
    Ok(())
}

impl SecureMeshPairwiseDurableStore {
    #[cfg(test)]
    pub fn record_remote_prekey_use(
        &mut self,
        prekey_use: &SecureMeshRemotePreKeyUse,
        used_at: impl Into<String>,
    ) -> Result<()> {
        let prepared = PreparedRemotePreKeyUse::new(prekey_use, used_at.into())?;
        let tx = self
            .connection
            .transaction()
            .context("secure mesh pairwise remote prekey-use transaction failed")?;
        consume_remote_prekey_use(&tx, &prepared)?;
        tx.commit()
            .context("secure mesh pairwise remote prekey-use commit failed")?;
        Ok(())
    }
}
