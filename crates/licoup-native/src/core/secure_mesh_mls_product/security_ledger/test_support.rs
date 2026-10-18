use super::SecureMeshMlsSecurityLedger;
use crate::core::secure_mesh_mls_product::helpers::hex_sha256;
use crate::core::secure_mesh_mls_product::ledger_transaction::mls_security_scope_hash;
use crate::core::secure_mesh_trust::DeviceTrustPublicIdentity;
use anyhow::Result;
use rusqlite::{OptionalExtension, params};

impl SecureMeshMlsSecurityLedger {
    pub(in crate::core::secure_mesh_mls_product) fn was_key_package_consumed(
        &self,
        consumer_identity: &DeviceTrustPublicIdentity,
        key_package_id: &str,
    ) -> Result<bool> {
        let consumer_scope_hash = mls_security_scope_hash(consumer_identity)?;
        let key_package_id_hash = hex_sha256(key_package_id.as_bytes());
        let found: Option<i64> = self
            .connection
            .query_row(
                r#"
                SELECT 1 FROM secure_mesh_mls_keypackage_uses
                WHERE consumer_endpoint_id = ?1 AND key_package_id = ?2
                "#,
                params![consumer_scope_hash, key_package_id_hash],
                |row| row.get(0),
            )
            .optional()?;
        Ok(found.is_some())
    }

    pub(in crate::core::secure_mesh_mls_product) fn key_package_consumed_at(
        &self,
        consumer_identity: &DeviceTrustPublicIdentity,
        key_package_id: &str,
    ) -> Result<Option<i64>> {
        let consumer_scope_hash = mls_security_scope_hash(consumer_identity)?;
        let key_package_id_hash = hex_sha256(key_package_id.as_bytes());
        self.connection
            .query_row(
                r#"
                SELECT used_at FROM secure_mesh_mls_keypackage_uses
                WHERE consumer_endpoint_id = ?1 AND key_package_id = ?2
                "#,
                params![consumer_scope_hash, key_package_id_hash],
                |row| {
                    let value: String = row.get(0)?;
                    value.parse::<i64>().map_err(|error| {
                        rusqlite::Error::FromSqlConversionFailure(
                            value.len(),
                            rusqlite::types::Type::Text,
                            Box::new(error),
                        )
                    })
                },
            )
            .optional()
            .map_err(Into::into)
    }
}
