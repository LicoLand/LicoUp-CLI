use anyhow::{Result, anyhow, ensure};
use serde_json::{Value, json};
#[cfg(test)]
use std::fs;
use std::net::TcpListener;
use std::path::Path;
use uuid::Uuid;

use super::manifest::{build_manifest, manifest_bytes, manifest_digest};
use super::model::{
    LocalAssemblyRecord, LocalServerLifecycle, PlannedLocalAssembly, assembled_runner_relative_path,
};
use super::store::{AssemblyOperationLock, insert_record, read_records, remove_record};
use super::{ASSEMBLY_ADAPTER_ID, ASSEMBLY_MANIFEST_FILE, ASSEMBLY_STATE_SCHEMA};
use crate::domain::collaboration_plugin::lifecycle::InstalledWorkflowPlugin;
use crate::domain::collaboration_plugin::package::{
    SecureNewTree, SelectedPayloadFile, SelectedServerRunner, inspect_package,
    select_current_server_runner,
};
use crate::platform::client_state::ClientStateStore;

pub(crate) fn plan_local_assembly(
    store: &ClientStateStore,
    installed: &InstalledWorkflowPlugin,
    selected_component_ids: &[String],
    payload: &[SelectedPayloadFile],
    params: &Value,
) -> Result<PlannedLocalAssembly> {
    reject_executable_params(params)?;
    ensure_desktop_runtime()?;
    let package = inspect_package(&installed.package_root)?;
    ensure!(
        package.digest_sha256 == installed.digest_sha256
            && package.manifest.signed_package_inventory_digest_sha256
                == installed.signed_package_inventory_digest_sha256,
        "collaboration_local_server_installed_package_digest_mismatch"
    );
    let runner = select_current_server_runner(&package)?;
    verify_runner_trust(installed, &package, &runner)?;
    let port = select_port(params, &read_records(store)?)?;
    let selected_payload_files = super::payload_inventory::from_selected(payload)?;
    let selected_payload_inventory_digest_sha256 =
        super::payload_inventory::digest(&selected_payload_files)?;
    let mut plan = PlannedLocalAssembly {
        deployment_id: Uuid::new_v4().to_string(),
        source_url: installed.source_url.clone(),
        server_version: installed.version.clone(),
        assembly_adapter_id: ASSEMBLY_ADAPTER_ID.to_owned(),
        bind_host: "127.0.0.1".to_owned(),
        port,
        manifest_digest_sha256: "0".repeat(64),
        manifest_bytes: 1,
        sealed_snapshot_digest_sha256: "0".repeat(64),
        sealed_snapshot_bytes: 1,
        runner_platform: runner.contract.platform.clone(),
        runner_architecture: runner.contract.architecture.clone(),
        runner_source_relative_path: runner
            .contract
            .relative_path
            .to_string_lossy()
            .replace('\\', "/"),
        runner_destination_relative_path: assembled_runner_relative_path(&runner.contract.platform),
        runner_digest_sha256: runner.contract.digest_sha256.clone(),
        runner_contract_version: runner.contract.runner_contract_version.clone(),
        health_contract_version: runner.contract.health_contract_version.clone(),
        capabilities_contract_version: runner.contract.capabilities_contract_version.clone(),
        signed_package_inventory_digest_sha256: package
            .manifest
            .signed_package_inventory_digest_sha256
            .clone(),
        source_commit_oid: installed.source_commit_oid.clone(),
        runner_trust_key_id: installed.runner_trust_key_id.clone(),
        runner_trust_fingerprint_sha256: installed.runner_trust_fingerprint_sha256.clone(),
        selected_payload_files,
        selected_payload_inventory_digest_sha256,
    };
    let manifest = build_manifest(installed, selected_component_ids, &plan)?;
    let bytes = manifest_bytes(&manifest)?;
    plan.manifest_digest_sha256 = manifest_digest(&manifest)?;
    plan.manifest_bytes = bytes.len();
    let snapshot = super::snapshot::build(
        payload,
        &runner,
        &plan.runner_destination_relative_path,
        &bytes,
    )?;
    plan.sealed_snapshot_digest_sha256 = super::snapshot::digest(&snapshot);
    plan.sealed_snapshot_bytes = snapshot.len();
    plan.validate()?;
    Ok(plan)
}

pub(crate) fn apply_local_assembly(
    store: &ClientStateStore,
    installed: &InstalledWorkflowPlugin,
    selected_component_ids: &[String],
    destination: &Path,
    payload: &[SelectedPayloadFile],
    plan: &PlannedLocalAssembly,
    params: &Value,
) -> Result<LocalAssemblyRecord> {
    reject_executable_params(params)?;
    ensure_desktop_runtime()?;
    plan.validate()?;
    ensure!(
        plan.source_url == installed.source_url && plan.server_version == installed.version,
        "collaboration_local_server_source_binding_mismatch"
    );
    let package = inspect_package(&installed.package_root)?;
    ensure!(
        package.digest_sha256 == installed.digest_sha256
            && package.manifest.signed_package_inventory_digest_sha256
                == installed.signed_package_inventory_digest_sha256,
        "collaboration_local_server_installed_package_digest_mismatch"
    );
    let runner = select_current_server_runner(&package)?;
    verify_runner_trust(installed, &package, &runner)?;
    ensure_runner_matches_plan(&runner, plan)?;
    let payload_inventory = super::payload_inventory::from_selected(payload)?;
    ensure!(
        payload_inventory == plan.selected_payload_files
            && super::payload_inventory::digest(&payload_inventory)?
                == plan.selected_payload_inventory_digest_sha256,
        "collaboration_local_server_payload_plan_mismatch"
    );
    let _operation_lock = AssemblyOperationLock::acquire(store)?;
    super::transaction::recover(store)?;
    ensure!(
        read_records(store)?
            .iter()
            .all(|record| record.deployment_id != plan.deployment_id
                && record.destination != destination.to_string_lossy()
                && record.port != plan.port),
        "collaboration_local_server_conflict"
    );
    ensure_port_available(plan.port)?;
    let (authority_state, authority) =
        crate::domain::collaboration_plugin::lifecycle::verified_authority(
            store,
            "Verify the protected local-server authority before assembling",
        )?;
    let manifest = build_manifest(installed, selected_component_ids, plan)?;
    let bytes = manifest_bytes(&manifest)?;
    ensure!(
        manifest_digest(&manifest)? == plan.manifest_digest_sha256,
        "collaboration_local_server_manifest_digest_mismatch"
    );
    let snapshot = super::snapshot::build(
        payload,
        &runner,
        &plan.runner_destination_relative_path,
        &bytes,
    )?;
    ensure!(
        super::snapshot::digest(&snapshot) == plan.sealed_snapshot_digest_sha256
            && snapshot.len() == plan.sealed_snapshot_bytes,
        "collaboration_local_server_snapshot_plan_mismatch"
    );
    let record = record_from_plan(installed, selected_component_ids, destination, plan)?;
    let mut replacement = authority.authority.clone();
    replacement.add_assembly(&record)?;
    super::transaction::begin(store, &record)?;
    let staged_tree = match stage_assembly(destination, payload, &runner, plan, &bytes, &snapshot) {
        Ok(tree) => tree,
        Err(error) => {
            super::transaction::clear(store)?;
            return Err(error);
        }
    };
    super::transaction::advance(store, super::transaction::ApplyPhase::ArtifactWritten)?;
    if simulate_commit_failure(params) {
        staged_tree.remove_if_still_bound();
        super::transaction::clear(store)?;
        return Err(anyhow!("collaboration_local_server_test_commit_failure"));
    }
    if let Err(error) = simulate_destination_replacement(params, destination) {
        if staged_tree.remove_if_still_bound() {
            super::transaction::clear(store)?;
        }
        return Err(error);
    }
    if let Err(error) = staged_tree.sync_and_validate_binding() {
        if staged_tree.remove_if_still_bound() {
            super::transaction::clear(store)?;
        }
        return Err(error);
    }

    let inserted = if simulate_state_failure(params) {
        Err(anyhow!("collaboration_local_server_test_state_failure"))
    } else {
        insert_record(store, record.clone())
    };
    if let Err(error) = inserted {
        staged_tree.remove_if_still_bound();
        super::transaction::clear(store)?;
        ensure!(
            !destination.exists(),
            "collaboration_local_server_apply_rollback_failed"
        );
        return Err(error);
    }
    super::transaction::advance(store, super::transaction::ApplyPhase::ProjectionWritten)?;
    if let Err(error) = crate::domain::collaboration_plugin::lifecycle::replace_authority(
        store,
        authority_state,
        &authority,
        replacement,
        "Authorize the exact sealed local-server assembly",
    ) {
        match crate::domain::collaboration_plugin::lifecycle::verified_authority(
            store,
            "Recover the protected local-server assembly authority",
        ) {
            Ok((_, current)) if current.authority.ensure_assembly(&record).is_ok() => {}
            Ok(_) => {
                remove_record(store, &record.deployment_id)?;
                staged_tree.remove_if_still_bound();
                super::transaction::clear(store)?;
                ensure!(
                    !destination.exists(),
                    "collaboration_local_server_apply_rollback_failed"
                );
                return Err(error);
            }
            Err(_) => return Err(error),
        }
    }
    super::transaction::advance(store, super::transaction::ApplyPhase::AuthorityCommitted)?;
    staged_tree.sync_and_validate_binding()?;
    super::transaction::clear(store)?;
    Ok(record)
}

fn record_from_plan(
    installed: &InstalledWorkflowPlugin,
    selected_component_ids: &[String],
    destination: &Path,
    plan: &PlannedLocalAssembly,
) -> Result<LocalAssemblyRecord> {
    let record = LocalAssemblyRecord {
        schema_version: ASSEMBLY_STATE_SCHEMA.to_owned(),
        deployment_id: plan.deployment_id.clone(),
        plugin_id: installed.plugin_id.clone(),
        source_url: installed.source_url.clone(),
        server_version: installed.version.clone(),
        package_digest_sha256: installed.digest_sha256.clone(),
        selected_component_ids: selected_component_ids.to_vec(),
        destination: destination
            .to_str()
            .ok_or_else(|| anyhow!("collaboration_local_server_destination_encoding_invalid"))?
            .to_owned(),
        assembly_adapter_id: plan.assembly_adapter_id.clone(),
        bind_host: plan.bind_host.clone(),
        port: plan.port,
        manifest_digest_sha256: plan.manifest_digest_sha256.clone(),
        destination_digest_sha256: super::snapshot::destination_digest(destination)?,
        sealed_snapshot_digest_sha256: plan.sealed_snapshot_digest_sha256.clone(),
        sealed_snapshot_bytes: plan.sealed_snapshot_bytes,
        runtime_generation: 1,
        execution_started: false,
        lifecycle: LocalServerLifecycle::Stopped,
        runtime_pid: None,
        runtime_instance_id: None,
        runtime_process_identity: None,
        runner_platform: plan.runner_platform.clone(),
        runner_architecture: plan.runner_architecture.clone(),
        runner_source_relative_path: plan.runner_source_relative_path.clone(),
        runner_destination_relative_path: plan.runner_destination_relative_path.clone(),
        runner_digest_sha256: plan.runner_digest_sha256.clone(),
        runner_contract_version: plan.runner_contract_version.clone(),
        health_contract_version: plan.health_contract_version.clone(),
        capabilities_contract_version: plan.capabilities_contract_version.clone(),
        signed_package_inventory_digest_sha256: plan.signed_package_inventory_digest_sha256.clone(),
        source_commit_oid: plan.source_commit_oid.clone(),
        runner_trust_key_id: plan.runner_trust_key_id.clone(),
        runner_trust_fingerprint_sha256: plan.runner_trust_fingerprint_sha256.clone(),
        selected_payload_files: plan.selected_payload_files.clone(),
        selected_payload_inventory_digest_sha256: plan
            .selected_payload_inventory_digest_sha256
            .clone(),
    };
    record.validate()?;
    Ok(record)
}

pub(crate) fn record_projection(record: &LocalAssemblyRecord) -> Value {
    json!({
        "deploymentId": record.deployment_id,
        "status": record.lifecycle.as_str(),
        "sourceUrl": record.source_url,
        "serverVersion": record.server_version,
        "packageDigestSha256": record.package_digest_sha256,
        "selectedComponentIds": record.selected_component_ids,
        "selectedPayloadFiles": record.selected_payload_files,
        "selectedPayloadInventoryDigestSha256": record.selected_payload_inventory_digest_sha256,
        "destination": record.destination,
        "assemblyAdapterId": record.assembly_adapter_id,
        "assemblyManifestDigestSha256": record.manifest_digest_sha256,
        "destinationDigestSha256": record.destination_digest_sha256,
        "sealedSnapshotDigestSha256": record.sealed_snapshot_digest_sha256,
        "sealedSnapshotBytes": record.sealed_snapshot_bytes,
        "runtimeGeneration": record.runtime_generation,
        "bindHost": record.bind_host,
        "port": record.port,
        "runtimeCapability": super::runtime::SANDBOX_CAPABILITY,
        "runnerPlatform": record.runner_platform,
        "runnerArchitecture": record.runner_architecture,
        "runnerSourceRelativePath": record.runner_source_relative_path,
        "runnerDestinationRelativePath": record.runner_destination_relative_path,
        "runnerDigestSha256": record.runner_digest_sha256,
        "runnerContractVersion": record.runner_contract_version,
        "healthContractVersion": record.health_contract_version,
        "capabilitiesContractVersion": record.capabilities_contract_version,
        "signedPackageInventoryDigestSha256": record.signed_package_inventory_digest_sha256,
        "sourceCommitOid": record.source_commit_oid,
        "runnerTrustKeyId": record.runner_trust_key_id,
        "runnerTrustFingerprintSha256": record.runner_trust_fingerprint_sha256,
        "healthVerified": record.lifecycle == LocalServerLifecycle::Running,
        "capabilitiesVerified": record.lifecycle == LocalServerLifecycle::Running,
        "loopbackOnly": true,
        "pluginCodeExecuted": record.execution_started,
        "runnerCodeExecuting": matches!(record.lifecycle, LocalServerLifecycle::Starting | LocalServerLifecycle::Running | LocalServerLifecycle::Stopping | LocalServerLifecycle::Quarantined),
        "selectedServerCodeExecuting": matches!(record.lifecycle, LocalServerLifecycle::Starting | LocalServerLifecycle::Running | LocalServerLifecycle::Stopping | LocalServerLifecycle::Quarantined),
        "externalFileTransferAuthorized": false
    })
}

pub(crate) fn plan_projection(plan: &PlannedLocalAssembly) -> Value {
    json!({
        "deploymentId": plan.deployment_id,
        "sourceUrl": plan.source_url,
        "serverVersion": plan.server_version,
        "assemblyAdapterId": plan.assembly_adapter_id,
        "assemblyManifestDigestSha256": plan.manifest_digest_sha256,
        "assemblyManifestBytes": plan.manifest_bytes,
        "sealedSnapshotDigestSha256": plan.sealed_snapshot_digest_sha256,
        "sealedSnapshotBytes": plan.sealed_snapshot_bytes,
        "bindHost": plan.bind_host,
        "port": plan.port,
        "runtimeCapability": super::runtime::SANDBOX_CAPABILITY,
        "runnerPlatform": plan.runner_platform,
        "runnerArchitecture": plan.runner_architecture,
        "runnerSourceRelativePath": plan.runner_source_relative_path,
        "runnerDestinationRelativePath": plan.runner_destination_relative_path,
        "runnerDigestSha256": plan.runner_digest_sha256,
        "runnerContractVersion": plan.runner_contract_version,
        "healthContractVersion": plan.health_contract_version,
        "capabilitiesContractVersion": plan.capabilities_contract_version,
        "signedPackageInventoryDigestSha256": plan.signed_package_inventory_digest_sha256,
        "sourceCommitOid": plan.source_commit_oid,
        "runnerTrustKeyId": plan.runner_trust_key_id,
        "runnerTrustFingerprintSha256": plan.runner_trust_fingerprint_sha256,
        "selectedPayloadFiles": plan.selected_payload_files,
        "selectedPayloadInventoryDigestSha256": plan.selected_payload_inventory_digest_sha256,
        "runnerWillExecuteOnlyAfterDirectStartApproval": true,
        "loopbackOnly": true,
        "preflightPassed": true,
        "pluginCodeWillExecute": true,
        "pluginCodeWillExecuteDuringAssembly": false,
        "selectedServerCodeWillExecuteOnDirectStart": true,
        "externalFileTransferAuthorized": false
    })
}

pub(crate) fn reject_executable_params(params: &Value) -> Result<()> {
    const FORBIDDEN: &[&str] = &[
        "argv",
        "args",
        "command",
        "env",
        "environment",
        "executable",
        "hook",
        "hooks",
        "process",
        "script",
        "shell",
    ];
    match params {
        Value::Object(values) => {
            for (key, value) in values {
                ensure!(
                    !FORBIDDEN.contains(&key.as_str()),
                    "collaboration_local_server_executable_directive_rejected"
                );
                reject_executable_params(value)?;
            }
        }
        Value::Array(values) => {
            for value in values {
                reject_executable_params(value)?;
            }
        }
        _ => {}
    }
    Ok(())
}

fn stage_assembly(
    destination: &Path,
    payload: &[SelectedPayloadFile],
    runner: &SelectedServerRunner,
    plan: &PlannedLocalAssembly,
    manifest: &[u8],
    snapshot: &[u8],
) -> Result<SecureNewTree> {
    let tree = SecureNewTree::create(destination)?;
    let result = (|| -> Result<()> {
        tree.sync_and_validate_binding()?;
        for file in payload {
            tree.write_file(&file.destination_relative_path, &file.bytes)?;
            tree.verify_file(&file.destination_relative_path, &file.bytes)?;
        }
        let runner_relative_path = Path::new(&plan.runner_destination_relative_path);
        tree.write_file(runner_relative_path, &runner.bytes)?;
        tree.verify_file(runner_relative_path, &runner.bytes)?;
        tree.write_file(Path::new(ASSEMBLY_MANIFEST_FILE), manifest)?;
        tree.verify_file(Path::new(ASSEMBLY_MANIFEST_FILE), manifest)?;
        tree.write_file(Path::new(super::ASSEMBLY_SNAPSHOT_FILE), snapshot)?;
        tree.verify_file(Path::new(super::ASSEMBLY_SNAPSHOT_FILE), snapshot)?;
        tree.create_directory(Path::new(super::ASSEMBLED_RUNTIME_DATA_DIRECTORY))?;
        // The runner becomes executable only after every immutable input and
        // the manifest are durably present in the same descriptor-bound tree.
        tree.make_file_owner_executable(runner_relative_path)?;
        tree.sync_and_validate_binding()?;
        Ok(())
    })();
    if let Err(error) = result {
        tree.remove_if_still_bound();
        return Err(error);
    }
    let runner_path = destination.join(&plan.runner_destination_relative_path);
    ensure!(
        crate::domain::collaboration_plugin::package::runner_sha256(
            &crate::domain::collaboration_plugin::package::read_file_no_follow(
                &runner_path,
                runner.bytes.len(),
            )?,
        ) == plan.runner_digest_sha256,
        "collaboration_local_server_runner_copy_digest_mismatch"
    );
    super::payload_inventory::verify_tree(
        destination,
        &plan.selected_payload_files,
        &plan.runner_destination_relative_path,
    )?;
    tree.sync_and_validate_binding()?;
    Ok(tree)
}

fn ensure_runner_matches_plan(
    runner: &SelectedServerRunner,
    plan: &PlannedLocalAssembly,
) -> Result<()> {
    ensure!(
        runner.contract.platform == plan.runner_platform
            && runner.contract.architecture == plan.runner_architecture
            && runner
                .contract
                .relative_path
                .to_string_lossy()
                .replace('\\', "/")
                == plan.runner_source_relative_path
            && runner.contract.digest_sha256 == plan.runner_digest_sha256
            && runner.contract.runner_contract_version == plan.runner_contract_version
            && runner.contract.health_contract_version == plan.health_contract_version
            && runner.contract.capabilities_contract_version == plan.capabilities_contract_version,
        "collaboration_local_server_runner_plan_mismatch"
    );
    Ok(())
}

pub(super) fn verify_runner_trust(
    installed: &InstalledWorkflowPlugin,
    package: &crate::domain::collaboration_plugin::package::InspectedPackage,
    runner: &SelectedServerRunner,
) -> Result<()> {
    ensure!(
        runner.contract.source_url == installed.source_url
            && runner.contract.source_commit_oid == installed.source_commit_oid
            && crate::domain::collaboration_plugin::runner_signature::public_key_fingerprint(
                &installed.runner_trust_public_key_base64url,
            )? == installed.runner_trust_fingerprint_sha256,
        "collaboration_local_server_runner_trust_binding_mismatch"
    );
    crate::domain::collaboration_plugin::runner_signature::verify_runner_signature(
        &package.manifest,
        &runner.contract,
        &installed.runner_trust_public_key_base64url,
    )
}

fn select_port(params: &Value, records: &[LocalAssemblyRecord]) -> Result<u16> {
    let requested = params.get("port").and_then(|value| match value {
        Value::Number(value) => value.as_u64(),
        Value::String(value) => value.parse().ok(),
        _ => None,
    });
    if let Some(requested) = requested {
        let port = u16::try_from(requested)
            .map_err(|_| anyhow!("collaboration_local_server_port_invalid"))?;
        ensure!(
            port >= 1024 && records.iter().all(|record| record.port != port),
            "collaboration_local_server_port_invalid"
        );
        ensure_port_available(port)?;
        return Ok(port);
    }
    for _ in 0..16 {
        let listener = TcpListener::bind(("127.0.0.1", 0))
            .map_err(|_| anyhow!("collaboration_local_server_port_unavailable"))?;
        let port = listener.local_addr()?.port();
        drop(listener);
        if port >= 1024 && records.iter().all(|record| record.port != port) {
            return Ok(port);
        }
    }
    Err(anyhow!("collaboration_local_server_port_unavailable"))
}

pub(crate) fn ensure_port_available(port: u16) -> Result<()> {
    let listener = TcpListener::bind(("127.0.0.1", port))
        .map_err(|_| anyhow!("collaboration_local_server_port_unavailable"))?;
    drop(listener);
    Ok(())
}

fn ensure_desktop_runtime() -> Result<()> {
    ensure!(
        cfg!(any(
            target_os = "macos",
            target_os = "linux",
            target_os = "windows"
        )),
        "collaboration_local_server_platform_unsupported"
    );
    Ok(())
}

#[cfg(test)]
fn simulate_commit_failure(params: &Value) -> bool {
    params
        .get("simulateAssemblyCommitFailure")
        .and_then(Value::as_bool)
        .unwrap_or(false)
}

#[cfg(not(test))]
fn simulate_commit_failure(_params: &Value) -> bool {
    false
}

#[cfg(test)]
fn simulate_state_failure(params: &Value) -> bool {
    params
        .get("simulateAssemblyStateFailure")
        .and_then(Value::as_bool)
        .unwrap_or(false)
}

#[cfg(test)]
fn simulate_destination_replacement(params: &Value, destination: &Path) -> Result<()> {
    if params.get("replaceDestinationBeforeCommitIndex").is_some() {
        let leaf = destination
            .file_name()
            .ok_or_else(|| anyhow!("collaboration_local_server_destination_invalid"))?
            .to_string_lossy();
        let displaced =
            destination.with_file_name(format!(".{leaf}.displaced-{}", uuid::Uuid::new_v4()));
        fs::rename(destination, displaced)?;
        fs::create_dir(destination)?;
        fs::write(destination.join("sentinel"), b"preserve")?;
    }
    Ok(())
}

#[cfg(not(test))]
fn simulate_destination_replacement(_params: &Value, _destination: &Path) -> Result<()> {
    Ok(())
}

#[cfg(not(test))]
fn simulate_state_failure(_params: &Value) -> bool {
    false
}
