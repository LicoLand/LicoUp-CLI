use crate::core::secure_mesh_capability::{
    CapabilityEvaluation, CapabilityEvaluationReport, CustodyRestartSemantics,
    SecretCustodyStrategy, SecurityCapability,
};
use crate::core::secure_mesh_transparency::KT_JSON_SAFE_INTEGER_MAX;
use crate::core::secure_mesh_trust::DeviceTrustPublicIdentity;
use crate::platform::client_state::ClientStateStore;
use crate::platform::secure_mesh_secret_store::{
    EphemeralSecretStore, PlatformSecretStore, SecretClassPersistenceProof,
    SecretStoreAuthorizationRequest, SecretStoreAuthorizationSession, SecretStoreHandle,
    SecureMeshSecretStore, platform_linux_secret_service_probe_snapshot,
    platform_native_secret_store_supported,
};
use anyhow::{Context, Result, anyhow, ensure};
use ed25519_dalek::SigningKey;
use licoup_foundation::platform::file_security::{
    create_private_state_marker, private_state_marker_exists, read_private_state_marker,
    remove_private_state_marker,
};
use serde_json::{Map, Value, json};
use std::cell::RefCell;
use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};
use uuid::Uuid;

use super::config::{default_config, normalize_config, prepare_station_fields_for_persistence};
use super::endpoint_trust::{
    ensure_mobile_relay_endpoint_descriptor, ensure_mobile_relay_endpoint_material,
    existing_identity_requires_custody, local_endpoint_state, secure_mesh_mls_state_dir,
    sha256_hex, validate_existing_identity_custody,
};
use super::pairwise_session::{mobile_relay_pairwise_store, mobile_relay_pairwise_store_path};
use super::support::{bool_param, text_param};

mod cleanup;
mod cleanup_authority;
mod config_store;
mod inventory;
mod persistence;
mod presentation;
mod recovery;
mod reset_guard;
mod runtime;
mod runtime_secret_material;
mod secret_material;
mod self_test;

#[cfg(test)]
mod tests;

pub(in crate::domain::mobile_relay) use cleanup::*;
pub(in crate::domain::mobile_relay) use cleanup_authority::*;
pub(in crate::domain::mobile_relay) use config_store::*;
pub(in crate::domain::mobile_relay) use inventory::*;
pub(in crate::domain::mobile_relay) use persistence::*;
pub(in crate::domain::mobile_relay) use presentation::*;
pub(crate) use recovery::prepare_recovered_custody_metadata;
pub(in crate::domain::mobile_relay) use reset_guard::*;
pub(in crate::domain::mobile_relay) use runtime::*;
#[cfg(test)]
pub(crate) use runtime_secret_material::test_runtime_secret_material;
pub(in crate::domain::mobile_relay) use runtime_secret_material::*;
pub(in crate::domain::mobile_relay) use secret_material::*;

pub fn with_pairwise_secret_store_override<T>(
    store: Arc<dyn SecureMeshSecretStore>,
    operation: impl FnOnce() -> Result<T>,
) -> Result<T> {
    runtime::with_pairwise_secret_store_override_in(store, operation)
}

pub fn with_mobile_relay_secret_store_override<T>(
    store: Arc<dyn SecureMeshSecretStore>,
    operation: impl FnOnce() -> Result<T>,
) -> Result<T> {
    runtime::with_mobile_relay_secret_store_override_in(store, operation)
}

pub fn selected_mobile_relay_capability_evaluation() -> Result<CapabilityEvaluation> {
    runtime::selected_mobile_relay_capability_evaluation_in()
}

pub(crate) fn with_secure_mesh_mls_participant<T>(
    params: &Value,
    additional_secret_store_operations: usize,
    operation: impl FnOnce(
        &mut Value,
        &DeviceTrustPublicIdentity,
        &SigningKey,
        &Arc<dyn SecureMeshSecretStore>,
        &SecretStoreAuthorizationSession,
        &str,
    ) -> Result<T>,
) -> Result<T> {
    runtime::with_secure_mesh_mls_participant_in(
        params,
        additional_secret_store_operations,
        operation,
    )
}

pub(crate) fn ensure_secure_mesh_protected_operation_allowed() -> Result<()> {
    reset_guard::ensure_secure_mesh_protected_operation_allowed_in()
}

pub fn e2ee_secret_store_cleanup(params: &Value) -> Result<Value> {
    cleanup::e2ee_secret_store_cleanup_in(params)
}

/// Read-only projection of the bounded custody cleanup inventory, the subject
/// it belongs to and the confirmation contract a cleanup would require.
pub fn e2ee_secret_store_cleanup_inventory() -> Result<Value> {
    cleanup_authority::custody_cleanup_inventory_report()
}

pub fn e2ee_secret_store_self_test(params: &Value) -> Result<Value> {
    self_test::e2ee_secret_store_self_test_in(params)
}
