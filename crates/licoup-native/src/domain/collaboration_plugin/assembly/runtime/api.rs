use anyhow::{Result, anyhow, ensure};
use serde_json::{Value, json};
use std::fs;
use std::path::Path;
use uuid::Uuid;

use super::super::apply::reject_executable_params;
use super::super::model::LocalServerLifecycle;
use super::super::store::{AssemblyOperationLock, find_record, read_records, remove_record};
use super::lifecycle::{SystemRuntimeControl, start_with, status_with, stop_with};
use crate::domain::collaboration_plugin::lifecycle::{
    client_state_store, require_direct_confirmation,
};
use crate::domain::collaboration_plugin::workflow::commit_directory_no_replace;
use crate::platform::client_state::ClientStateStore;

pub(crate) fn status(params: &Value) -> Result<Value> {
    reject_executable_params(params)?;
    let store = client_state_store(params)?;
    let _transaction =
        crate::domain::collaboration_plugin::transaction::CollaborationTransactionGuard::acquire(
            &store,
        )?;
    status_with(&store, &SystemRuntimeControl)
}

pub(crate) fn start(params: &Value) -> Result<Value> {
    reject_executable_params(params)?;
    require_direct_request(params)?;
    require_direct_confirmation(
        params,
        "collaboration_local_server_start_confirmation_required",
    )?;
    let deployment_id = deployment_id(params)?;
    let store = client_state_store(params)?;
    let _transaction =
        crate::domain::collaboration_plugin::transaction::CollaborationTransactionGuard::acquire(
            &store,
        )?;
    start_with(&store, deployment_id, &SystemRuntimeControl)
}

pub(crate) fn stop(params: &Value) -> Result<Value> {
    reject_executable_params(params)?;
    require_direct_request(params)?;
    require_direct_confirmation(
        params,
        "collaboration_local_server_stop_confirmation_required",
    )?;
    let deployment_id = deployment_id(params)?;
    let store = client_state_store(params)?;
    let _transaction =
        crate::domain::collaboration_plugin::transaction::CollaborationTransactionGuard::acquire(
            &store,
        )?;
    stop_with(&store, deployment_id, &SystemRuntimeControl)
}

pub(crate) fn uninstall(params: &Value) -> Result<Value> {
    reject_executable_params(params)?;
    require_direct_request(params)?;
    require_direct_confirmation(
        params,
        "collaboration_local_server_uninstall_confirmation_required",
    )?;
    let deployment_id = deployment_id(params)?;
    let expected = params
        .get("expectedAssemblyManifestDigestSha256")
        .and_then(Value::as_str)
        .ok_or_else(|| anyhow!("collaboration_local_server_expected_digest_required"))?;
    let store = client_state_store(params)?;
    let _transaction =
        crate::domain::collaboration_plugin::transaction::CollaborationTransactionGuard::acquire(
            &store,
        )?;
    uninstall_in(&store, deployment_id, expected, params)
}

pub(crate) fn has_assemblies(store: &ClientStateStore) -> Result<bool> {
    Ok(!read_records(store)?.is_empty() || super::super::transaction::pending_count(store)? > 0)
}

pub(crate) fn stop_all(store: &ClientStateStore) -> Result<()> {
    let runtime = SystemRuntimeControl;
    let _ = status_with(store, &runtime)?;
    for record in read_records(store)? {
        if record.runtime_pid.is_some() {
            stop_with(store, &record.deployment_id, &runtime)?;
        }
    }
    Ok(())
}

fn uninstall_in(
    store: &ClientStateStore,
    deployment_id: &str,
    expected_digest: &str,
    params: &Value,
) -> Result<Value> {
    let _operation_lock = AssemblyOperationLock::acquire(store)?;
    super::super::transaction::recover(store)?;
    if recover_pending_uninstall(store, deployment_id)? {
        return Ok(json!({
            "ok": true,
            "status": "uninstalled",
            "deploymentId": deployment_id,
            "assemblyManifestDigestSha256": expected_digest,
            "cleanupPending": super::super::cleanup::count(store)? > 0,
            "recoveredPreparedTransaction": true
        }));
    }
    let record = find_record(store, deployment_id)?;
    ensure!(
        record.lifecycle == LocalServerLifecycle::Stopped
            && record.runtime_pid.is_none()
            && record.runtime_instance_id.is_none()
            && record.runtime_process_identity.is_none(),
        "collaboration_local_server_must_be_stopped_before_uninstall"
    );
    ensure!(
        expected_digest == record.manifest_digest_sha256,
        "collaboration_local_server_uninstall_digest_mismatch"
    );
    super::runner::verify_assembly(store, &record)?;
    let (authority_state, authority) =
        crate::domain::collaboration_plugin::lifecycle::verified_authority(
            store,
            "Verify the protected authority before local-server uninstall",
        )?;
    authority.authority.ensure_assembly(&record)?;
    let mut replacement = authority.authority.clone();
    replacement.remove_assembly(&record)?;
    let destination = Path::new(&record.destination);
    let operation_id = Uuid::new_v4().to_string();
    let pending =
        super::super::cleanup::PendingAssemblyCleanup::prepare(store, &record, &operation_id)?;
    let quarantine = Path::new(&pending.quarantine);
    if let Err(error) = commit_directory_no_replace(destination, quarantine) {
        let _ = super::super::cleanup::remove(store, deployment_id);
        let _ = error;
        return Err(anyhow!(
            "collaboration_local_server_uninstall_prepare_failed"
        ));
    }
    if simulate_uninstall_state_failure(params) {
        let _ = commit_directory_no_replace(quarantine, destination);
        let _ = super::super::cleanup::remove(store, deployment_id);
        return Err(anyhow!("collaboration_local_server_test_state_failure"));
    }
    if let Err(error) = crate::domain::collaboration_plugin::lifecycle::replace_authority(
        store,
        authority_state,
        &authority,
        replacement,
        "Revoke the exact protected local-server assembly",
    ) {
        match crate::domain::collaboration_plugin::lifecycle::verified_authority(
            store,
            "Recover the protected local-server uninstall authority",
        ) {
            Ok((_, current)) if !current.authority.contains_assembly(deployment_id) => {}
            Ok((_, current)) if current.authority.ensure_assembly(&record).is_ok() => {
                let _ = commit_directory_no_replace(quarantine, destination);
                let _ = super::super::cleanup::remove(store, deployment_id);
                return Err(error);
            }
            Ok(_) | Err(_) => return Err(error),
        }
    }
    remove_record(store, deployment_id)?;
    let cleanup_pending = fs::remove_dir_all(quarantine).is_err()
        || super::super::cleanup::remove(store, deployment_id).is_err();
    Ok(json!({
        "ok": true,
        "status": "uninstalled",
        "deploymentId": deployment_id,
        "assemblyManifestDigestSha256": expected_digest,
        "cleanupPending": cleanup_pending
    }))
}

fn recover_pending_uninstall(store: &ClientStateStore, deployment_id: &str) -> Result<bool> {
    let Some(pending) = super::super::cleanup::find(store, deployment_id)? else {
        return Ok(false);
    };
    let original = Path::new(&pending.original_destination);
    let quarantine = Path::new(&pending.quarantine);
    let record = read_records(store)?
        .into_iter()
        .find(|record| record.deployment_id == deployment_id);
    let (_, authority) = crate::domain::collaboration_plugin::lifecycle::verified_authority(
        store,
        "Recover a prepared local-server uninstall transaction",
    )?;
    if let Some(record) = record.as_ref() {
        if authority.authority.ensure_assembly(record).is_ok() {
            if original.exists() && !quarantine.exists() {
                super::super::cleanup::remove(store, deployment_id)?;
                return Ok(false);
            }
            ensure!(
                !original.exists() && quarantine.exists(),
                "collaboration_local_server_cleanup_state_invalid"
            );
            commit_directory_no_replace(quarantine, original)
                .map_err(|_| anyhow!("collaboration_local_server_uninstall_recovery_failed"))?;
            super::super::cleanup::remove(store, deployment_id)?;
            return Ok(false);
        }
        ensure!(
            !authority.authority.contains_assembly(deployment_id),
            "collaboration_authority_assembly_binding_mismatch"
        );
    } else {
        ensure!(
            !authority.authority.contains_assembly(deployment_id),
            "collaboration_local_server_cleanup_state_invalid"
        );
    }
    if original.exists() && !quarantine.exists() {
        commit_directory_no_replace(original, quarantine)
            .map_err(|_| anyhow!("collaboration_local_server_uninstall_recovery_failed"))?;
    }
    if record.is_some() {
        remove_record(store, deployment_id)?;
    }
    if quarantine.exists() && fs::remove_dir_all(quarantine).is_err() {
        return Ok(true);
    }
    super::super::cleanup::remove(store, deployment_id)?;
    Ok(true)
}

fn require_direct_request(params: &Value) -> Result<()> {
    ensure!(
        params.get("requestOrigin").and_then(Value::as_str) == Some("direct-user"),
        "collaboration_local_server_direct_user_origin_required"
    );
    ensure!(
        ["agentTriggered", "scheduled", "startupTriggered"]
            .iter()
            .all(|key| params.get(*key).and_then(Value::as_bool) != Some(true)),
        "collaboration_local_server_automatic_trigger_forbidden"
    );
    Ok(())
}

fn deployment_id(params: &Value) -> Result<&str> {
    let value = params
        .get("deploymentId")
        .and_then(Value::as_str)
        .ok_or_else(|| anyhow!("collaboration_local_server_deployment_id_required"))?;
    ensure!(
        Uuid::parse_str(value).is_ok_and(|parsed| parsed.to_string() == value),
        "collaboration_local_server_deployment_id_invalid"
    );
    Ok(value)
}

#[cfg(test)]
fn simulate_uninstall_state_failure(params: &Value) -> bool {
    params
        .get("simulateAssemblyUninstallStateFailure")
        .and_then(Value::as_bool)
        .unwrap_or(false)
}

#[cfg(not(test))]
fn simulate_uninstall_state_failure(_params: &Value) -> bool {
    false
}
