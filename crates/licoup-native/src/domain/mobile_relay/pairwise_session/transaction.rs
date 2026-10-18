use super::store::mobile_relay_pairwise_store;
use crate::core::secure_mesh_pairwise::{
    SecureMeshPairwiseDurableRecord, SecureMeshPairwiseDurableStore,
    SecureMeshPairwisePendingDelivery, SecureMeshPairwiseReceivedPayload,
    SecureMeshPairwiseSession,
};
use crate::domain::mobile_relay::endpoint_trust::{local_endpoint_state, now_iso, session_id};
use crate::domain::mobile_relay::secret_custody::{
    RuntimeSecretContext, RuntimeSecretMaterial, ensure_secure_mesh_protected_operation_allowed,
};
use crate::platform::secure_mesh_secret_store::{
    SecretStoreAuthorizationRequest, SecretStoreAuthorizationSession,
};
use anyhow::{Result, anyhow};
use serde_json::Value;

pub(in crate::domain::mobile_relay) struct MobileRelayPairwiseOperation {
    pub(super) store: SecureMeshPairwiseDurableStore,
    pub(super) record: SecureMeshPairwiseDurableRecord,
    pub(super) session: SecureMeshPairwiseSession,
    secret_store_session: SecretStoreAuthorizationSession,
}

impl MobileRelayPairwiseOperation {
    pub(in crate::domain::mobile_relay) fn commit(&mut self) -> Result<()> {
        self.record = self.store.commit_session_with_authorized_session(
            &self.record,
            &self.session,
            now_iso(),
            &self.secret_store_session,
        )?;
        Ok(())
    }

    pub(in crate::domain::mobile_relay) fn commit_with_pending_delivery(
        &mut self,
        pending_delivery: &SecureMeshPairwisePendingDelivery,
    ) -> Result<()> {
        self.record = self
            .store
            .commit_session_with_authorized_session_and_pending_delivery(
                &self.record,
                &self.session,
                pending_delivery,
                now_iso(),
                &self.secret_store_session,
            )?;
        Ok(())
    }

    pub(in crate::domain::mobile_relay) fn commit_with_received_payload(
        &mut self,
        received_payload: &SecureMeshPairwiseReceivedPayload,
    ) -> Result<()> {
        self.record = self
            .store
            .commit_session_with_authorized_session_and_received_payload(
                &self.record,
                &self.session,
                received_payload,
                now_iso(),
                &self.secret_store_session,
            )?;
        Ok(())
    }

    pub(in crate::domain::mobile_relay) fn received_payload(
        &self,
        binding_digest: &str,
    ) -> Result<Option<SecureMeshPairwiseReceivedPayload>> {
        self.store.read_received_payload_with_authorized_session(
            &self.record.session_id,
            &self.record.local_endpoint_id,
            binding_digest,
            &self.secret_store_session,
        )
    }

    pub(in crate::domain::mobile_relay) fn delete_received_payload(
        &mut self,
        receipt_id: &str,
    ) -> Result<bool> {
        self.store.delete_received_payload_with_authorized_session(
            &self.record.session_id,
            &self.record.local_endpoint_id,
            receipt_id,
            &self.secret_store_session,
        )
    }

    pub(in crate::domain::mobile_relay) fn pending_delivery(
        &self,
        delivery_kind: &str,
    ) -> Result<Option<SecureMeshPairwisePendingDelivery>> {
        self.store.read_pending_delivery(
            &self.record.session_id,
            &self.record.local_endpoint_id,
            delivery_kind,
        )
    }

    pub(in crate::domain::mobile_relay) fn delete_pending_delivery(
        &mut self,
        delivery_kind: &str,
        envelope_id: &str,
    ) -> Result<bool> {
        self.store.delete_pending_delivery(
            &self.record.session_id,
            &self.record.local_endpoint_id,
            delivery_kind,
            envelope_id,
        )
    }
}

#[cfg(test)]
pub(in crate::domain::mobile_relay) fn mobile_relay_pairwise_operation(
    config: &Value,
    secret_material: &RuntimeSecretMaterial,
    reason: &'static str,
    operation_count: usize,
) -> Result<MobileRelayPairwiseOperation> {
    mobile_relay_pairwise_operation_with_authorized_session(
        config,
        secret_material,
        reason,
        operation_count,
        None,
    )
}

pub(in crate::domain::mobile_relay) fn mobile_relay_pairwise_operation_with_runtime_secret_context(
    config: &Value,
    reason: &'static str,
    operation_count: usize,
    secret_context: &mut RuntimeSecretContext,
) -> Result<MobileRelayPairwiseOperation> {
    let shared_session = secret_context.shared_authorization_session()?;
    mobile_relay_pairwise_operation_with_authorized_session(
        config,
        &secret_context.material,
        reason,
        operation_count,
        shared_session.as_ref(),
    )
}

pub(in crate::domain::mobile_relay) fn mobile_relay_pairwise_operation_with_authorized_session(
    config: &Value,
    secret_material: &RuntimeSecretMaterial,
    reason: &'static str,
    operation_count: usize,
    authorized_session: Option<&SecretStoreAuthorizationSession>,
) -> Result<MobileRelayPairwiseOperation> {
    ensure_secure_mesh_protected_operation_allowed()?;
    let store = mobile_relay_pairwise_store()?;
    let endpoint = local_endpoint_state(config, secret_material)?;
    let session_id = session_id(config)?;
    if let Some(record) = store.read_record(&session_id, &endpoint.endpoint_id)? {
        let secret_store_session = authorized_session
            .filter(|session| session.backend() == store.secret_store_backend())
            .cloned()
            .map(Ok)
            .unwrap_or_else(|| {
                store.begin_authorized_session(&SecretStoreAuthorizationRequest::new(
                    reason,
                    operation_count,
                ))
            })?;
        let session = store
            .load_session_with_authorized_session(
                &session_id,
                &endpoint.endpoint_id,
                &secret_store_session,
            )?
            .ok_or_else(|| anyhow!("mobile relay pairwise session record is missing"))?;
        return Ok(MobileRelayPairwiseOperation {
            store,
            record,
            session,
            secret_store_session,
        });
    }
    Err(anyhow!(
        "mobile relay pairwise session is not initialized; re-pairing is required"
    ))
}
