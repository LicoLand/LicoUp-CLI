mod catalog;
mod cleanup;
mod install;
mod model;
mod state;
mod support;
mod trust;

#[cfg(test)]
mod tests;

use anyhow::Result;
use serde_json::Value;

use crate::platform::client_state::ClientStateStore;

pub(in crate::domain::collaboration_plugin) use model::{
    CapabilityState, InstalledPlugin, InstalledWorkflowPlugin, RunnerTrustRecord,
};

pub fn status(params: &Value) -> Result<Value> {
    state::status_in(&state::client_state_store(params)?)
}

pub fn enable(params: &Value) -> Result<Value> {
    state::enable_in(&state::client_state_store(params)?, params)
}

pub fn install_plan(params: &Value) -> Result<Value> {
    install::install_plan_in(&state::client_state_store(params)?, params)
}

pub fn install_apply(params: &Value) -> Result<Value> {
    install::install_apply_in(&state::client_state_store(params)?, params)
}

pub fn install_cancel(params: &Value) -> Result<Value> {
    install::install_cancel_in(&state::client_state_store(params)?, params)
}

pub fn runner_trust_import(params: &Value) -> Result<Value> {
    trust::trust_import_in(&state::client_state_store(params)?, params)
}

pub fn runner_trust_remove(params: &Value) -> Result<Value> {
    trust::trust_remove_in(&state::client_state_store(params)?, params)
}

pub fn workflow_catalog(params: &Value) -> Result<Value> {
    catalog::workflow_catalog_in(&state::client_state_store(params)?)
}

pub fn disable(params: &Value) -> Result<Value> {
    state::disable_in(&state::client_state_store(params)?, params)
}

pub fn uninstall(params: &Value) -> Result<Value> {
    cleanup::uninstall_in(&state::client_state_store(params)?, params)
}

pub fn cleanup(params: &Value) -> Result<Value> {
    cleanup::cleanup_in(&state::client_state_store(params)?, params)
}

pub(super) fn installed_workflow_plugin(
    store: &ClientStateStore,
) -> Result<InstalledWorkflowPlugin> {
    state::installed_workflow_plugin(store)
}

pub(super) fn verified_authority(
    store: &ClientStateStore,
    reason: &str,
) -> Result<(CapabilityState, super::authority::BoundAuthority)> {
    let mut state = state::read_state(store)?;
    let projected = super::authority::projected(&state)?;
    let verified = match super::authority::read(
        store,
        projected.secure_record.version(),
        projected.secure_record.record_digest_sha256(),
        reason,
    ) {
        Ok(verified) => verified,
        Err(_) => {
            let current = super::authority::recover_current(
                store,
                "Recover the canonical optional-collaboration authority projection",
            )?;
            super::authority::ensure_projection_matches(&current.authority, &state)?;
            if current.secure_record != projected.secure_record {
                state.authority_record = Some(current.secure_record.clone());
                state::write_state(store, &state)?;
            }
            current
        }
    };
    super::authority::ensure_projection_matches(&verified.authority, &state)?;
    Ok((state, verified))
}

pub(super) fn replace_authority(
    store: &ClientStateStore,
    mut state: CapabilityState,
    expected: &super::authority::BoundAuthority,
    replacement: super::authority::CollaborationAuthority,
    reason: &str,
) -> Result<super::authority::BoundAuthority> {
    let bound = super::authority::replace(store, expected, replacement, reason)?;
    super::authority::apply_projection(&mut state, &bound)?;
    state::write_state(store, &state)?;
    Ok(bound)
}

pub(super) fn require_direct_confirmation(params: &Value, code: &'static str) -> Result<()> {
    support::require_direct_confirmation(params, code)
}

pub(super) fn collaboration_root(store: &ClientStateStore) -> std::path::PathBuf {
    state::collaboration_root(store)
}

pub(super) fn client_state_store(params: &Value) -> Result<ClientStateStore> {
    state::client_state_store(params)
}

pub(super) fn epoch_seconds() -> u64 {
    state::epoch_seconds()
}

#[cfg(test)]
pub(super) fn status_in(store: &ClientStateStore) -> Result<Value> {
    state::status_in(store)
}

#[cfg(test)]
pub(super) fn enable_in(store: &ClientStateStore, params: &Value) -> Result<Value> {
    state::enable_in(store, params)
}

#[cfg(test)]
pub(super) fn disable_in(store: &ClientStateStore, params: &Value) -> Result<Value> {
    state::disable_in(store, params)
}

#[cfg(test)]
pub(super) fn workflow_catalog_in(store: &ClientStateStore) -> Result<Value> {
    catalog::workflow_catalog_in(store)
}

#[cfg(test)]
pub(super) fn uninstall_in(store: &ClientStateStore, params: &Value) -> Result<Value> {
    cleanup::uninstall_in(store, params)
}

#[cfg(test)]
pub(super) fn cleanup_in(store: &ClientStateStore, params: &Value) -> Result<Value> {
    cleanup::cleanup_in(store, params)
}

#[cfg(test)]
pub(super) fn install_apply_in(store: &ClientStateStore, params: &Value) -> Result<Value> {
    install::install_apply_in(store, params)
}

#[cfg(test)]
pub(super) fn install_cancel_in(store: &ClientStateStore, params: &Value) -> Result<Value> {
    install::install_cancel_in(store, params)
}

#[cfg(test)]
pub(super) fn runner_trust_import_in(store: &ClientStateStore, params: &Value) -> Result<Value> {
    trust::trust_import_in(store, params)
}

#[cfg(test)]
pub(super) fn install_plan_from_directory_in(
    store: &ClientStateStore,
    source_root: &std::path::Path,
) -> Result<Value> {
    install::install_plan_from_directory_in(store, source_root)
}

#[cfg(test)]
pub(super) fn plan_root(store: &ClientStateStore, plan_id: &str) -> Result<std::path::PathBuf> {
    install::plan_root(store, plan_id)
}
