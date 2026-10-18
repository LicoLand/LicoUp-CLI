use super::super::package::inspect_package;
use super::install::plans_root;
use super::model::{CapabilityState, PendingCleanup};
use super::state::{collaboration_root, epoch_seconds, plugins_root, read_state, write_state};
use super::support::{require_direct_confirmation, require_direct_request, required_digest};
use anyhow::{Result, anyhow, ensure};
use serde_json::{Value, json};
use std::fs;
use std::path::PathBuf;
use uuid::Uuid;

use crate::platform::client_state::ClientStateStore;

pub(super) fn uninstall_in(store: &ClientStateStore, params: &Value) -> Result<Value> {
    let _transaction = super::super::transaction::CollaborationTransactionGuard::acquire(store)?;
    require_direct_request(params)?;
    require_direct_confirmation(
        params,
        "collaboration_plugin_uninstall_confirmation_required",
    )?;
    let mut state = read_state(store)?;
    let original_state = state.clone();
    let expected_authority = super::super::authority::projected(&state)?;
    ensure!(
        !super::super::assembly::has_assemblies(store)?,
        "collaboration_local_server_uninstall_required"
    );
    let installed = state
        .installed
        .clone()
        .ok_or_else(|| anyhow!("collaboration_plugin_not_installed"))?;
    let expected_digest = required_digest(params, "expectedDigestSha256")?;
    ensure!(
        installed.digest_sha256 == expected_digest,
        "collaboration_plugin_uninstall_digest_mismatch"
    );
    let destination = plugins_root(store)?.join(&installed.plugin_id);
    let package = inspect_package(&destination)?;
    ensure!(
        package.digest_sha256 == installed.digest_sha256,
        "collaboration_plugin_installed_digest_mismatch"
    );
    let operation_id = Uuid::new_v4();
    let entry_name = format!(".uninstall-{operation_id}");
    let quarantine = plugins_root(store)?.join(&entry_name);
    let pending = PendingCleanup {
        kind: "uninstall-quarantine".to_owned(),
        entry_name,
    };
    push_pending_cleanup(&mut state, pending.clone())?;
    let registrations = super::super::registration::registration_root(store);
    let registration_entry_name = format!(".uninstall-registrations-{operation_id}");
    let registration_quarantine = collaboration_root(store).join(&registration_entry_name);
    let registration_pending = registrations.exists().then(|| PendingCleanup {
        kind: "uninstall-registrations-quarantine".to_owned(),
        entry_name: registration_entry_name,
    });
    if let Some(pending) = registration_pending.as_ref() {
        push_pending_cleanup(&mut state, pending.clone())?;
    }
    // Persist the exact quarantine names before any rename. A crash can then
    // resume or clean the prepared transaction without losing its targets.
    write_state(store, &state)?;
    super::super::workflow::commit_directory_no_replace(&destination, &quarantine).map_err(
        |_| {
            let _ = write_state(store, &original_state);
            anyhow!("collaboration_plugin_uninstall_prepare_failed")
        },
    )?;
    if registration_pending.is_some()
        && super::super::workflow::commit_directory_no_replace(
            &registrations,
            &registration_quarantine,
        )
        .is_err()
    {
        let _ = super::super::workflow::commit_directory_no_replace(&quarantine, &destination);
        let _ = write_state(store, &original_state);
        return Err(anyhow!("collaboration_plugin_uninstall_prepare_failed"));
    }
    state.capability_enabled = false;
    state.installed = None;
    let mut replacement_authority = expected_authority.authority.clone();
    replacement_authority.capability_enabled = false;
    replacement_authority.installed = None;
    replacement_authority.assemblies.clear();
    replacement_authority.registrations.clear();
    let bound_authority = match super::super::authority::replace(
        store,
        &expected_authority,
        replacement_authority,
        "Uninstall the exact local-server package and revoke its local bindings",
    ) {
        Ok(bound) => bound,
        Err(error) => {
            if registration_pending.is_some() {
                let _ = super::super::workflow::commit_directory_no_replace(
                    &registration_quarantine,
                    &registrations,
                );
            }
            let _ = super::super::workflow::commit_directory_no_replace(&quarantine, &destination);
            let _ = write_state(store, &original_state);
            return Err(error);
        }
    };
    super::super::authority::apply_projection(&mut state, &bound_authority)?;
    if let Err(error) = write_state(store, &state) {
        // Authority already advanced. Keep the prepared quarantine and its
        // persisted names for explicit recovery; rolling files back would
        // reintroduce a revoked artifact.
        return Err(error);
    }
    let cleanup_pending = if simulate_cleanup_failure(params) {
        true
    } else if fs::remove_dir_all(&quarantine).is_ok()
        && (registration_pending.is_none() || fs::remove_dir_all(&registration_quarantine).is_ok())
    {
        state.cleanup_pending.retain(|item| item != &pending);
        if let Some(registration_pending) = registration_pending.as_ref() {
            state
                .cleanup_pending
                .retain(|item| item != registration_pending);
        }
        write_state(store, &state).is_err()
    } else {
        true
    };
    Ok(json!({
        "ok": true,
        "status": "uninstalled",
        "capabilityEnabled": false,
        "pluginInstalled": false,
        "pluginLoaded": false,
        "cleanupPending": cleanup_pending
    }))
}

pub(super) fn cleanup_in(store: &ClientStateStore, params: &Value) -> Result<Value> {
    let _transaction = super::super::transaction::CollaborationTransactionGuard::acquire(store)?;
    require_direct_request(params)?;
    require_direct_confirmation(params, "collaboration_plugin_cleanup_confirmation_required")?;
    let mut state = read_state(store)?;
    let mut retained = Vec::new();
    for pending in &state.cleanup_pending {
        let path = pending_cleanup_path(store, pending)?;
        if path.exists() && fs::remove_dir_all(&path).is_err() {
            retained.push(pending.clone());
        }
    }
    state.cleanup_pending = retained;
    prune_cancelled_install_plans(&mut state);
    write_state(store, &state)?;
    Ok(json!({
        "ok": true,
        "status": if state.cleanup_pending.is_empty() { "clean" } else { "cleanup-pending" },
        "cleanupPending": !state.cleanup_pending.is_empty(),
        "cleanupPendingCount": state.cleanup_pending.len()
    }))
}

pub(super) fn push_pending_cleanup(
    state: &mut CapabilityState,
    pending: PendingCleanup,
) -> Result<()> {
    ensure!(
        state.cleanup_pending.len() < 16,
        "collaboration_plugin_cleanup_pending_limit_reached"
    );
    ensure!(
        !state.cleanup_pending.contains(&pending),
        "collaboration_plugin_cleanup_pending_duplicate"
    );
    validate_cleanup_entry_name(&pending.entry_name)?;
    state.cleanup_pending.push(pending);
    Ok(())
}

fn pending_cleanup_path(store: &ClientStateStore, pending: &PendingCleanup) -> Result<PathBuf> {
    validate_cleanup_entry_name(&pending.entry_name)?;
    match pending.kind.as_str() {
        "install-plan" | "install-cancel" => Ok(plans_root(store)?.join(&pending.entry_name)),
        "uninstall-quarantine" => Ok(plugins_root(store)?.join(&pending.entry_name)),
        "uninstall-registrations-quarantine" => {
            Ok(collaboration_root(store).join(&pending.entry_name))
        }
        _ => Err(anyhow!("collaboration_plugin_cleanup_kind_invalid")),
    }
}

pub(super) fn validate_cleanup_entry_name(value: &str) -> Result<()> {
    ensure!(
        !value.is_empty()
            && value.len() <= 192
            && value
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.'))
            && value != "."
            && value != "..",
        "collaboration_plugin_cleanup_entry_invalid"
    );
    Ok(())
}

pub(super) fn prune_cancelled_install_plans(state: &mut CapabilityState) {
    let now = epoch_seconds();
    state
        .cancelled_install_plans
        .retain(|receipt| receipt.expires_at_epoch_seconds > now);
    if state.cancelled_install_plans.len() > 32 {
        let discard = state.cancelled_install_plans.len() - 32;
        state.cancelled_install_plans.drain(..discard);
    }
}

#[cfg(test)]
pub(super) fn simulate_cleanup_failure(params: &Value) -> bool {
    params
        .get("simulateCleanupFailure")
        .and_then(|value| match value {
            Value::Bool(value) => Some(*value),
            Value::String(value) => Some(value == "true"),
            _ => None,
        })
        .unwrap_or(false)
}

#[cfg(not(test))]
pub(super) fn simulate_cleanup_failure(_params: &Value) -> bool {
    false
}
