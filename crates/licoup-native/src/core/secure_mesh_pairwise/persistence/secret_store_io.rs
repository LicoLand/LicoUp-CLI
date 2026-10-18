use anyhow::{Context, Result, anyhow, ensure};

use super::super::{
    key_ratchet::SecureMeshPairwiseSession,
    support::{MAX_PERSISTED_SECRET_SNAPSHOT_BYTES, PAIRWISE_SNAPSHOT_SCHEMA_VERSION, sha256_hex},
};
use super::{
    namespace_binding::pairwise_secret_store_key,
    public_snapshot::PersistedPairwisePublicSession,
    secret_snapshot::{PendingPairwiseSnapshot, PersistedPairwiseSessionSecrets},
    store_model::SecureMeshPairwiseDurableStore,
};
use crate::core::secure_mesh_secret_store::{
    SecretBytes, SecretStoreAuthorizationRequest, SecretStoreAuthorizationSession,
    SecretStoreHandle,
};

impl SecureMeshPairwiseDurableStore {
    pub(super) fn prepare_secret_bound_snapshot(
        &self,
        session: &SecureMeshPairwiseSession,
        state_version: u64,
    ) -> Result<PendingPairwiseSnapshot> {
        self.prepare_secret_bound_snapshot_with_optional_authorization(session, state_version, None)
    }

    pub(super) fn prepare_secret_bound_snapshot_with_optional_authorization(
        &self,
        session: &SecureMeshPairwiseSession,
        state_version: u64,
        secret_store_session: Option<&SecretStoreAuthorizationSession>,
    ) -> Result<PendingPairwiseSnapshot> {
        self.retry_pending_secret_cleanup()?;
        let secret_store_key = pairwise_secret_store_key(
            &session.session_id,
            &session.local_endpoint_id,
            state_version,
        );
        let public = session.to_public_snapshot(
            state_version,
            self.secret_store_namespace.clone(),
            secret_store_key.clone(),
        );
        let public_json = serde_json::to_string(&public)
            .context("secure mesh pairwise public snapshot serialization failed")?;
        let secrets =
            session.to_secret_snapshot(state_version, sha256_hex(public_json.as_bytes()))?;
        let secret_json = serde_json::to_string(&secrets)
            .context("secure mesh pairwise secret snapshot serialization failed")?;
        ensure!(
            secret_json.len() <= MAX_PERSISTED_SECRET_SNAPSHOT_BYTES,
            "secure mesh pairwise secret snapshot exceeds the resource limit"
        );
        let secret_handle =
            self.secret_snapshot_handle(&public.secret_store_namespace, &public.secret_store_key)?;
        let secret_store_session = match secret_store_session {
            Some(session) => session.clone(),
            None => self.secret_store.begin_authorized_session(
                &SecretStoreAuthorizationRequest::new(
                    "Secure Mesh pairwise durable snapshot commit",
                    2,
                ),
            )?,
        };
        self.secret_store
            .set_secret_with_session(
                &secret_store_session,
                &secret_handle,
                SecretBytes::try_from_string(secret_json)?,
            )
            .context("secure mesh pairwise secret snapshot write failed")?;
        Ok(PendingPairwiseSnapshot {
            public_json,
            secret_handle,
            secret_store_session,
        })
    }

    pub(super) fn load_secret_snapshot(
        &self,
        public: &PersistedPairwisePublicSession,
        secret_store_session: Option<&SecretStoreAuthorizationSession>,
    ) -> Result<PersistedPairwiseSessionSecrets> {
        let secret_handle =
            self.secret_snapshot_handle(&public.secret_store_namespace, &public.secret_store_key)?;
        let secret_store_session = match secret_store_session {
            Some(session) => session.clone(),
            None => self.secret_store.begin_authorized_session(
                &SecretStoreAuthorizationRequest::new(
                    "Secure Mesh pairwise durable snapshot load",
                    1,
                ),
            )?,
        };
        let secret_json = self
            .secret_store
            .get_secret_with_session(&secret_store_session, &secret_handle)
            .context("secure mesh pairwise secret snapshot read failed")?
            .ok_or_else(|| anyhow!("secure mesh pairwise secret snapshot is unavailable"))?;
        ensure!(
            secret_json.expose_bytes().len() <= MAX_PERSISTED_SECRET_SNAPSHOT_BYTES,
            "secure mesh pairwise secret snapshot exceeds the resource limit"
        );
        let secrets: PersistedPairwiseSessionSecrets =
            serde_json::from_slice(secret_json.expose_bytes())
                .context("secure mesh pairwise secret snapshot deserialization failed")?;
        let public_json = serde_json::to_string(public)
            .context("secure mesh pairwise public snapshot binding serialization failed")?;
        ensure!(
            secrets.schema_version == PAIRWISE_SNAPSHOT_SCHEMA_VERSION
                && secrets.state_version == public.state_version
                && secrets.session_id == public.session_id
                && secrets.local_endpoint_id == public.local_endpoint_id
                && secrets.remote_endpoint_id == public.remote_endpoint_id
                && secrets.public_snapshot_digest == sha256_hex(public_json.as_bytes()),
            "secure mesh pairwise secret snapshot binding verification failed"
        );
        Ok(secrets)
    }

    pub(super) fn secret_snapshot_handle(
        &self,
        namespace: &str,
        key: &str,
    ) -> Result<SecretStoreHandle> {
        SecretStoreHandle::new(namespace.to_string(), key.to_string())
    }
}
