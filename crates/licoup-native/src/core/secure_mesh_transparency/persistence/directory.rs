//! Latest-directory monotonicity, purpose-bound authorization, and quotas.

use anyhow::{Result, anyhow, ensure};
use rusqlite::{OptionalExtension, Transaction, params};

use super::super::constants::{
    MAX_PERSISTED_DIRECTORY_AUTHORIZATIONS, MAX_PERSISTED_DIRECTORY_LABELS,
};
use super::super::model::{
    DirectoryComponentCommitments, SecureMeshKtInclusionProof, SecureMeshKtMapProof,
};
use super::super::signature::{PinnedKtLogKey, VerifiedKtFreshness};
use super::sql::{sql_to_u64, u64_to_sql};
use super::time_guard::persist_security_block;

pub(crate) fn enforce_directory_latest_transaction(
    transaction: &Transaction<'_>,
    pin: &PinnedKtLogKey,
    stable_label: &str,
    version: u64,
    leaf_hash: &str,
    revoked: bool,
    components: &DirectoryComponentCommitments<'_>,
    tree_size: u64,
) -> Result<Option<&'static str>> {
    enforce_directory_label_quota(transaction, pin, stable_label)?;
    let prior = transaction
        .query_row(
            "SELECT version, leaf_hash, revoked, identity_fingerprint, identity_rotation_epoch, identity_key_digest, pairwise_prekey_version, signed_prekey_digest, one_time_prekey_digest, mls_key_package_version, mls_key_package_digest FROM secure_mesh_kt_directory_latest WHERE log_id = ?1 AND stable_label = ?2",
            params![pin.log_id(), stable_label],
            |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, i64>(2)? != 0,
                    row.get::<_, String>(3)?,
                    row.get::<_, i64>(4)?,
                    row.get::<_, String>(5)?,
                    row.get::<_, i64>(6)?,
                    row.get::<_, String>(7)?,
                    row.get::<_, String>(8)?,
                    row.get::<_, i64>(9)?,
                    row.get::<_, String>(10)?,
                ))
            },
        )
        .optional()?;
    if let Some((
        prior_version_raw,
        prior_leaf_hash,
        prior_revoked,
        prior_identity_fingerprint,
        prior_identity_rotation_raw,
        prior_identity_key_digest,
        prior_pairwise_version_raw,
        prior_signed_prekey_digest,
        prior_one_time_prekey_digest,
        prior_mls_version_raw,
        prior_mls_digest,
    )) = prior
    {
        let prior_version = u64::try_from(prior_version_raw)
            .map_err(|_| anyhow!("secure mesh KT persisted directory version is invalid"))?;
        let prior_pairwise_version = u64::try_from(prior_pairwise_version_raw)
            .map_err(|_| anyhow!("secure mesh KT persisted Pairwise prekey version is invalid"))?;
        let prior_mls_version = u64::try_from(prior_mls_version_raw)
            .map_err(|_| anyhow!("secure mesh KT persisted MLS KeyPackage version is invalid"))?;
        let prior_identity_rotation = u64::try_from(prior_identity_rotation_raw)
            .map_err(|_| anyhow!("secure mesh KT persisted identity rotation epoch is invalid"))?;
        let reason = if version < prior_version {
            Some(("directory_version_rollback", "directory version rollback"))
        } else if version == prior_version
            && (leaf_hash != prior_leaf_hash || revoked != prior_revoked)
        {
            Some((
                "directory_same_version_split",
                "directory same-version split view",
            ))
        } else if prior_revoked && !revoked {
            Some((
                "directory_revoked_resurrection",
                "revoked identity resurrection",
            ))
        } else if components.identity_rotation_epoch < prior_identity_rotation {
            Some((
                "identity_rotation_epoch_rollback",
                "identity rotation epoch rollback",
            ))
        } else if components.identity_key_digest == prior_identity_key_digest
            && (components.identity_rotation_epoch != prior_identity_rotation
                || components.identity_fingerprint != prior_identity_fingerprint)
        {
            Some((
                "identity_epoch_changed_without_key_change",
                "identity epoch changed without identity material change",
            ))
        } else if components.identity_key_digest != prior_identity_key_digest
            && components.identity_rotation_epoch <= prior_identity_rotation
        {
            Some((
                "identity_key_changed_without_epoch_advance",
                "identity key changed without strict rotation epoch advance",
            ))
        } else if components.pairwise_prekey_version < prior_pairwise_version {
            Some((
                "pairwise_prekey_version_rollback",
                "Pairwise prekey version rollback",
            ))
        } else if components.pairwise_prekey_version == prior_pairwise_version
            && (components.signed_prekey_digest != prior_signed_prekey_digest
                || components.one_time_prekey_digest != prior_one_time_prekey_digest)
        {
            Some((
                "pairwise_prekey_same_version_split",
                "Pairwise prekey same-version split view",
            ))
        } else if components.mls_key_package_version < prior_mls_version {
            Some((
                "mls_key_package_version_rollback",
                "MLS KeyPackage version rollback",
            ))
        } else if components.mls_key_package_version == prior_mls_version
            && components.mls_key_package_digest != prior_mls_digest
        {
            Some((
                "mls_key_package_same_version_split",
                "MLS KeyPackage same-version split view",
            ))
        } else {
            None
        };
        if let Some((code, message)) = reason {
            persist_security_block(transaction, code)?;
            return Ok(Some(message));
        }
    }
    transaction.execute(
        "INSERT INTO secure_mesh_kt_directory_latest(log_id, stable_label, version, leaf_hash, revoked, identity_fingerprint, identity_rotation_epoch, identity_key_digest, pairwise_prekey_version, signed_prekey_digest, one_time_prekey_digest, mls_key_package_version, mls_key_package_digest, tree_size) VALUES(?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14)
         ON CONFLICT(log_id, stable_label) DO UPDATE SET version = excluded.version, leaf_hash = excluded.leaf_hash, revoked = excluded.revoked, identity_fingerprint = excluded.identity_fingerprint, identity_rotation_epoch = excluded.identity_rotation_epoch, identity_key_digest = excluded.identity_key_digest, pairwise_prekey_version = excluded.pairwise_prekey_version, signed_prekey_digest = excluded.signed_prekey_digest, one_time_prekey_digest = excluded.one_time_prekey_digest, mls_key_package_version = excluded.mls_key_package_version, mls_key_package_digest = excluded.mls_key_package_digest, tree_size = excluded.tree_size",
        params![
            pin.log_id(),
            stable_label,
            u64_to_sql(version)?,
            leaf_hash,
            i64::from(revoked),
            components.identity_fingerprint,
            u64_to_sql(components.identity_rotation_epoch)?,
            components.identity_key_digest,
            u64_to_sql(components.pairwise_prekey_version)?,
            components.signed_prekey_digest,
            components.one_time_prekey_digest,
            u64_to_sql(components.mls_key_package_version)?,
            components.mls_key_package_digest,
            u64_to_sql(tree_size)?,
        ],
    )?;
    Ok(None)
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn persist_directory_authorization_transaction(
    transaction: &Transaction<'_>,
    pin: &PinnedKtLogKey,
    stable_label: &str,
    purpose: &str,
    directory_version: u64,
    leaf_hash: &str,
    revoked: bool,
    inclusion: &SecureMeshKtInclusionProof,
    map_proof: &SecureMeshKtMapProof,
    freshness: &VerifiedKtFreshness,
) -> Result<()> {
    reclaim_stale_directory_authorizations(transaction, pin, inclusion.signed_tree_head.tree_size)?;
    enforce_directory_authorization_quota(transaction, pin, stable_label, purpose)?;
    let inclusion_json = serde_json::to_string(inclusion)?;
    let map_proof_json = serde_json::to_string(map_proof)?;
    transaction.execute(
        "INSERT INTO secure_mesh_kt_directory_authorizations(
            log_id, stable_label, purpose, directory_version, leaf_hash, revoked,
            tree_size, root_hash, map_root_hash, issued_at_epoch_seconds,
            observed_at_epoch_seconds, inclusion_json, map_proof_json
         ) VALUES(?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)
         ON CONFLICT(log_id, stable_label, purpose) DO UPDATE SET
            directory_version = excluded.directory_version,
            leaf_hash = excluded.leaf_hash,
            revoked = excluded.revoked,
            tree_size = excluded.tree_size,
            root_hash = excluded.root_hash,
            map_root_hash = excluded.map_root_hash,
            issued_at_epoch_seconds = excluded.issued_at_epoch_seconds,
            observed_at_epoch_seconds = excluded.observed_at_epoch_seconds,
            inclusion_json = excluded.inclusion_json,
            map_proof_json = excluded.map_proof_json",
        params![
            pin.log_id(),
            stable_label,
            purpose,
            u64_to_sql(directory_version)?,
            leaf_hash,
            i64::from(revoked),
            u64_to_sql(inclusion.signed_tree_head.tree_size)?,
            inclusion.signed_tree_head.root_hash,
            inclusion.signed_tree_head.map_root_hash,
            u64_to_sql(inclusion.signed_tree_head.issued_at_epoch_seconds)?,
            u64_to_sql(freshness.observed_at_epoch_seconds)?,
            inclusion_json,
            map_proof_json,
        ],
    )?;
    Ok(())
}

pub(crate) fn enforce_directory_label_quota(
    transaction: &Transaction<'_>,
    pin: &PinnedKtLogKey,
    stable_label: &str,
) -> Result<()> {
    let exists = transaction
        .query_row(
            "SELECT 1 FROM secure_mesh_kt_directory_latest WHERE log_id = ?1 AND stable_label = ?2",
            params![pin.log_id(), stable_label],
            |_| Ok(()),
        )
        .optional()?
        .is_some();
    if exists {
        return Ok(());
    }
    let count: i64 = transaction.query_row(
        "SELECT COUNT(*) FROM secure_mesh_kt_directory_latest WHERE log_id = ?1",
        params![pin.log_id()],
        |row| row.get(0),
    )?;
    ensure!(
        sql_to_u64(count, "directory label count")? < MAX_PERSISTED_DIRECTORY_LABELS,
        "secure mesh KT directory label quota is exhausted"
    );
    Ok(())
}

pub(crate) fn reclaim_stale_directory_authorizations(
    transaction: &Transaction<'_>,
    pin: &PinnedKtLogKey,
    current_tree_size: u64,
) -> Result<()> {
    transaction.execute(
        "DELETE FROM secure_mesh_kt_directory_authorizations WHERE log_id = ?1 AND tree_size <> ?2",
        params![pin.log_id(), u64_to_sql(current_tree_size)?],
    )?;
    Ok(())
}

pub(crate) fn enforce_directory_authorization_quota(
    transaction: &Transaction<'_>,
    pin: &PinnedKtLogKey,
    stable_label: &str,
    purpose: &str,
) -> Result<()> {
    let exists = transaction
        .query_row(
            "SELECT 1 FROM secure_mesh_kt_directory_authorizations WHERE log_id = ?1 AND stable_label = ?2 AND purpose = ?3",
            params![pin.log_id(), stable_label, purpose],
            |_| Ok(()),
        )
        .optional()?
        .is_some();
    if exists {
        return Ok(());
    }
    let count: i64 = transaction.query_row(
        "SELECT COUNT(*) FROM secure_mesh_kt_directory_authorizations WHERE log_id = ?1",
        params![pin.log_id()],
        |row| row.get(0),
    )?;
    ensure!(
        sql_to_u64(count, "directory authorization count")?
            < MAX_PERSISTED_DIRECTORY_AUTHORIZATIONS,
        "secure mesh KT directory authorization quota is exhausted"
    );
    Ok(())
}
