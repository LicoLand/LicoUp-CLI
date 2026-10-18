use anyhow::{Result, anyhow};
use uuid::Uuid;

#[cfg(target_os = "macos")]
use std::sync::{Arc, Mutex};

#[cfg(target_os = "macos")]
use super::macos_user_presence::MacosSecretStoreAccess;
use crate::core::secure_mesh_secret_store::{
    SecretBytes, SecretStoreAuthorizationRequest, SecretStoreAuthorizationSession,
    SecretStoreHandle, SecureMeshSecretStore,
};

pub struct SecretClassPersistenceProof {
    pub backend: &'static str,
    pub secret_classes: Vec<String>,
    pub requested_class_count: usize,
    pub persisted_class_count: usize,
    pub deleted_class_count: usize,
    pub all_classes_persisted: bool,
    pub all_classes_deleted: bool,
    pub raw_secret_material_included: bool,
}

#[derive(Clone)]
pub struct PlatformSecretStore {
    pub(super) service: &'static str,
    pub(super) account_prefix: &'static str,
    #[cfg(target_os = "macos")]
    macos_secret_store_access: Arc<Mutex<Option<Arc<MacosSecretStoreAccess>>>>,
}

impl PlatformSecretStore {
    pub fn new(service: &'static str, account_prefix: &'static str) -> Self {
        Self {
            service,
            account_prefix,
            #[cfg(target_os = "macos")]
            macos_secret_store_access: Arc::new(Mutex::new(None)),
        }
    }

    #[cfg(target_os = "macos")]
    #[cfg(test)]
    pub(crate) fn with_macos_secret_store_access(mut self, access: MacosSecretStoreAccess) -> Self {
        self.macos_secret_store_access = Arc::new(Mutex::new(Some(Arc::new(access))));
        self
    }

    #[cfg(target_os = "macos")]
    pub(super) fn macos_secret_store_access(&self) -> Result<Option<Arc<MacosSecretStoreAccess>>> {
        self.macos_secret_store_access
            .lock()
            .map(|selected| selected.clone())
            .map_err(|_| anyhow!("secure_mesh_presence_session_unavailable"))
    }

    #[cfg(target_os = "macos")]
    pub(super) fn select_macos_secret_store_access(
        &self,
        access: Arc<MacosSecretStoreAccess>,
    ) -> Result<()> {
        *self
            .macos_secret_store_access
            .lock()
            .map_err(|_| anyhow!("secure_mesh_presence_session_unavailable"))? = Some(access);
        Ok(())
    }

    pub fn handle_for_namespace(
        &self,
        namespace: impl Into<String>,
        key: impl Into<String>,
    ) -> Result<SecretStoreHandle> {
        SecretStoreHandle::new(format!("{}:{}", self.account_prefix, namespace.into()), key)
    }

    #[cfg(target_os = "macos")]
    pub(crate) fn get_legacy_classic_secret_with_session(
        &self,
        session: &SecretStoreAuthorizationSession,
        handle: &SecretStoreHandle,
    ) -> Result<Option<SecretBytes>> {
        self.macos_secret_store_access()?
            .ok_or_else(|| anyhow!("secure_mesh_presence_session_batch_mismatch"))?
            .get_legacy_classic_secret(self.service, session, handle)
    }

    #[cfg(target_os = "macos")]
    pub(crate) fn delete_legacy_classic_secret_with_session(
        &self,
        session: &SecretStoreAuthorizationSession,
        handle: &SecretStoreHandle,
    ) -> Result<()> {
        self.macos_secret_store_access()?
            .ok_or_else(|| anyhow!("secure_mesh_presence_session_batch_mismatch"))?
            .delete_legacy_classic_secret(self.service, session, handle)
    }

    #[cfg(target_os = "macos")]
    pub(crate) fn with_legacy_credential_migration_session<T>(
        &self,
        request: &SecretStoreAuthorizationRequest,
        migrate: impl FnOnce(&SecretStoreAuthorizationSession) -> Result<T>,
    ) -> Result<T> {
        use crate::core::secure_mesh_secret_store::{
            SecretStoreCallerChannel, SecretStoreKeyClass,
        };
        if request.caller_channel() != SecretStoreCallerChannel::GatewayCredentialMigration
            || request.key_class() != SecretStoreKeyClass::GatewayCredential
            || !request.allow_interaction()
        {
            return Err(anyhow!("secure_mesh_keychain_migration_scope_invalid"));
        }
        let access = self
            .macos_secret_store_access()?
            .filter(|access| access.is_injected())
            .map(Ok)
            .unwrap_or_else(|| {
                super::macos_user_presence::production_access(request).map(Arc::new)
            })?;
        let session = access.begin_session(self.backend(), request)?;
        struct MigrationSessionScope<'a> {
            access: &'a MacosSecretStoreAccess,
            session: &'a SecretStoreAuthorizationSession,
        }
        impl Drop for MigrationSessionScope<'_> {
            fn drop(&mut self) {
                self.access.finish_migration_session(self.session);
            }
        }
        let _scope = MigrationSessionScope {
            access: &access,
            session: &session,
        };
        self.select_macos_secret_store_access(Arc::clone(&access))?;
        migrate(&session)
    }

    pub fn verify_secret_class_persistence(
        &self,
        namespace: impl Into<String>,
        secret_classes: &[&str],
    ) -> Result<SecretClassPersistenceProof> {
        if !self.supported() {
            return Err(anyhow!(
                "secure mesh native secret store unsupported for shared secret class proof"
            ));
        }
        let namespace = namespace.into();
        let session = self.begin_authorized_session(&SecretStoreAuthorizationRequest::new(
            "Secure Mesh shared secret class persistence proof",
            secret_classes.len().saturating_mul(4),
        ))?;
        self.verify_secret_class_persistence_with_session(&session, namespace, secret_classes)
    }

    pub fn verify_secret_class_persistence_with_session(
        &self,
        session: &SecretStoreAuthorizationSession,
        namespace: impl Into<String>,
        secret_classes: &[&str],
    ) -> Result<SecretClassPersistenceProof> {
        let namespace = namespace.into();
        let mut persisted_class_count = 0usize;
        let mut deleted_class_count = 0usize;
        let mut handles = Vec::new();
        for secret_class in secret_classes {
            let handle = self.handle_for_namespace(&namespace, *secret_class)?;
            let proof_secret = format!(
                "secure-mesh-secret-store-proof:{}:{}",
                secret_class,
                Uuid::new_v4()
            );
            self.set_secret_with_session(
                session,
                &handle,
                SecretBytes::try_from_string(proof_secret.clone())?,
            )?;
            if self
                .get_secret_with_session(session, &handle)?
                .as_ref()
                .map(SecretBytes::expose_bytes)
                == Some(proof_secret.as_bytes())
            {
                persisted_class_count += 1;
            }
            handles.push(handle);
        }
        for handle in &handles {
            self.delete_secret_with_session(session, handle)?;
            if self.get_secret_with_session(session, handle)?.is_none() {
                deleted_class_count += 1;
            }
        }
        Ok(SecretClassPersistenceProof {
            backend: self.backend(),
            secret_classes: secret_classes
                .iter()
                .map(|secret_class| (*secret_class).to_string())
                .collect(),
            requested_class_count: secret_classes.len(),
            persisted_class_count,
            deleted_class_count,
            all_classes_persisted: persisted_class_count == secret_classes.len(),
            all_classes_deleted: deleted_class_count == secret_classes.len(),
            raw_secret_material_included: false,
        })
    }
}

impl std::fmt::Debug for PlatformSecretStore {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("PlatformSecretStore")
            .field("backend_configuration", &"redacted")
            .finish()
    }
}
