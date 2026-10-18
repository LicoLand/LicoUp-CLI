use std::sync::Arc;

use crate::core::secure_mesh_mls::{SecureMeshMlsDurableStore, SecureMeshMlsParticipant};
use crate::core::secure_mesh_mls_product::{
    SecureMeshMlsSecurityLedger, participant_from_device_identity,
};
use crate::core::secure_mesh_trust::{DeviceTrustPublicIdentity, DeviceTrustState};
use crate::platform::secure_mesh_secret_store::{
    SecretStoreAuthorizationSession, SecretStoreHandle, SecureMeshSecretStore,
};
use anyhow::{Result, ensure};
use ed25519_dalek::SigningKey;
use serde_json::Value;

use super::input_codec::hex_sha256;
use super::journal_recovery::recover_incomplete_writer_operations;

const MLS_PARTICIPANT_SNAPSHOT_KEY_PREFIX: &str = "secureMeshMlsParticipantMlKem1024_";

pub(super) enum ParticipantRequirement {
    CreateIfMissing,
    Required,
}

pub(super) struct LocalParticipantRuntime<'a> {
    pub(super) config: &'a mut Value,
    pub(super) identity: &'a DeviceTrustPublicIdentity,
    pub(super) signing_key: &'a SigningKey,
    pub(super) secret_store: &'a Arc<dyn SecureMeshSecretStore>,
    pub(super) authorization: &'a SecretStoreAuthorizationSession,
    pub(super) snapshot_handle: &'a SecretStoreHandle,
    pub(super) participant: &'a mut SecureMeshMlsParticipant,
    pub(super) group_store: &'a mut Option<SecureMeshMlsDurableStore>,
}

impl LocalParticipantRuntime<'_> {
    pub(super) fn persist_participant(&self) -> Result<()> {
        self.participant.save_secret_store_with_session(
            self.secret_store.as_ref(),
            self.snapshot_handle,
            self.authorization,
        )
    }

    pub(super) fn authoritative_trust_state(
        &self,
        identity: &DeviceTrustPublicIdentity,
    ) -> Result<DeviceTrustState> {
        if identity == self.identity {
            return Ok(DeviceTrustState::Verified);
        }
        crate::domain::mobile_relay::persisted_mobile_relay_peer_trust_state(
            self.config,
            self.identity,
            identity,
        )
    }
}

pub(super) fn with_local_participant(
    params: &Value,
    requirement: ParticipantRequirement,
    operation: impl FnOnce(&mut LocalParticipantRuntime<'_>) -> Result<(Value, bool)>,
) -> Result<Value> {
    crate::domain::mobile_relay::with_secure_mesh_mls_participant(
        params,
        4,
        |config, identity, signing_key, secret_store, authorization, namespace| {
            let handle = participant_snapshot_handle(namespace, identity)?;
            let mut group_store: Option<SecureMeshMlsDurableStore> = None;
            let mut participant =
                match SecureMeshMlsParticipant::load_from_secret_store_optional_with_session(
                    crate::core::secure_mesh_mls_product::mls_credential_identity_bytes(identity)?,
                    identity.signing_public_key,
                    secret_store.as_ref(),
                    &handle,
                    authorization,
                )? {
                    Some(participant) => participant,
                    None => {
                        handle_missing_participant_snapshot(
                            &mut group_store,
                            identity,
                            secret_store.backend(),
                        )?;
                        ensure!(
                            matches!(requirement, ParticipantRequirement::CreateIfMissing),
                            "secure mesh MLS participant state is unavailable in selected custody"
                        );
                        participant_from_device_identity(identity, signing_key)?
                    }
                };
            let mut runtime = LocalParticipantRuntime {
                config,
                identity,
                signing_key,
                secret_store,
                authorization,
                snapshot_handle: &handle,
                participant: &mut participant,
                group_store: &mut group_store,
            };
            recover_incomplete_writer_operations(
                &mut *runtime.group_store,
                runtime.participant,
                runtime.identity,
            )?;
            let (response, persist) = operation(&mut runtime)?;
            if persist {
                participant.save_secret_store_with_session(
                    secret_store.as_ref(),
                    &handle,
                    authorization,
                )?;
            }
            Ok(response)
        },
    )
}

pub(super) fn handle_missing_participant_snapshot(
    group_store: &mut Option<SecureMeshMlsDurableStore>,
    identity: &DeviceTrustPublicIdentity,
    selected_backend: &str,
) -> Result<()> {
    if group_store.is_none() {
        *group_store = Some(open_group_state_store()?);
    }
    let group_store = group_store
        .as_mut()
        .expect("secure mesh MLS durable group-state store opened above");
    let participant_scope = identity.fingerprint()?;
    let has_group_state = group_store.has_records_for_participant(&participant_scope)?;
    if selected_backend == "memory-only-ephemeral" {
        group_store.purge_unrecoverable_memory_only_state()?;
        return Ok(());
    }
    ensure!(
        !has_group_state,
        "secure mesh MLS persistent participant snapshot is missing while durable group state exists"
    );
    Ok(())
}

pub(super) fn group_state_store(
    group_store: &mut Option<SecureMeshMlsDurableStore>,
) -> Result<&mut SecureMeshMlsDurableStore> {
    if group_store.is_none() {
        *group_store = Some(open_group_state_store()?);
    }
    Ok(group_store
        .as_mut()
        .expect("secure mesh MLS durable group-state store opened above"))
}

fn open_group_state_store() -> Result<SecureMeshMlsDurableStore> {
    crate::platform::secure_mesh_mls_store::open(
        crate::domain::mobile_relay::secure_mesh_mls_state_dir()?.join("group-state.sqlite3"),
    )
}

fn participant_snapshot_handle(
    namespace: &str,
    identity: &DeviceTrustPublicIdentity,
) -> Result<SecretStoreHandle> {
    let digest = hex_sha256(identity.fingerprint()?.as_bytes());
    SecretStoreHandle::new(
        namespace,
        format!("{MLS_PARTICIPANT_SNAPSHOT_KEY_PREFIX}{digest}"),
    )
}

pub(crate) fn reset_selected_custody_for_kt_authority_change(
    identity: &DeviceTrustPublicIdentity,
    secret_store: &dyn SecureMeshSecretStore,
    authorization: &SecretStoreAuthorizationSession,
    namespace: &str,
) -> Result<()> {
    let handle = participant_snapshot_handle(namespace, identity)?;
    secret_store.delete_secret_with_session(authorization, &handle)?;
    Ok(())
}

pub(crate) fn reset_durable_state_for_kt_authority_change() -> Result<()> {
    let state_dir = crate::domain::mobile_relay::secure_mesh_mls_state_dir()?;
    let mut group_store =
        crate::platform::secure_mesh_mls_store::open(state_dir.join("group-state.sqlite3"))?;
    group_store.reset_for_kt_authority_change()?;
    let mut security_ledger =
        SecureMeshMlsSecurityLedger::open(state_dir.join("security-ledger.sqlite3"))?;
    security_ledger.reset_for_kt_authority_change()?;
    Ok(())
}
