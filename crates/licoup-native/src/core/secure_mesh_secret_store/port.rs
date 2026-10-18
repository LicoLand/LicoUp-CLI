use anyhow::{Result, anyhow};

use crate::core::secure_mesh_capability::{
    CapabilityEvaluation, CapabilityEvidenceKind, CapabilityFact, capability_catalog,
    mandatory_protocol_facts,
};

use super::{
    SecretBytes, SecretStoreAuthorizationRequest, SecretStoreAuthorizationSession,
    SecretStoreHandle,
};

pub trait SecureMeshSecretStore: Send + Sync {
    fn backend(&self) -> &'static str;
    fn supported(&self) -> bool;

    fn capability_facts(&self) -> Result<Vec<CapabilityFact>> {
        Ok(Vec::new())
    }

    fn capability_evaluation(&self) -> Result<CapabilityEvaluation> {
        let mut facts = mandatory_protocol_facts(CapabilityEvidenceKind::SourceContract)?;
        facts.extend(self.capability_facts()?);
        capability_catalog()?.evaluate(&facts)
    }

    fn begin_authorized_session(
        &self,
        request: &SecretStoreAuthorizationRequest,
    ) -> Result<SecretStoreAuthorizationSession> {
        Ok(
            SecretStoreAuthorizationSession::new(self.backend(), request, false, false)
                .with_capability_report(self.capability_evaluation()?.report()),
        )
    }

    fn set_secret(&self, handle: &SecretStoreHandle, secret: SecretBytes) -> Result<()>;

    fn set_secret_with_session(
        &self,
        session: &SecretStoreAuthorizationSession,
        handle: &SecretStoreHandle,
        secret: SecretBytes,
    ) -> Result<()> {
        if session.shared_system_context_required() {
            return Err(anyhow!(
                "secure mesh secret store backend {} must implement session-aware writes for {}",
                session.backend(),
                handle.key()
            ));
        }
        session.record_secret_store_operation("write")?;
        self.set_secret(handle, secret)
    }

    fn get_secret(&self, handle: &SecretStoreHandle) -> Result<Option<SecretBytes>>;

    fn get_secret_with_session(
        &self,
        session: &SecretStoreAuthorizationSession,
        handle: &SecretStoreHandle,
    ) -> Result<Option<SecretBytes>> {
        if session.shared_system_context_required() {
            return Err(anyhow!(
                "secure mesh secret store backend {} must implement session-aware reads for {}",
                session.backend(),
                handle.key()
            ));
        }
        session.record_secret_store_operation("read")?;
        self.get_secret(handle)
    }

    fn delete_secret(&self, handle: &SecretStoreHandle) -> Result<()>;

    fn delete_secret_with_session(
        &self,
        session: &SecretStoreAuthorizationSession,
        handle: &SecretStoreHandle,
    ) -> Result<()> {
        if session.shared_system_context_required() {
            return Err(anyhow!(
                "secure mesh secret store backend {} must implement session-aware deletes for {}",
                session.backend(),
                handle.key()
            ));
        }
        session.record_secret_store_operation("delete")?;
        self.delete_secret(handle)
    }
}
