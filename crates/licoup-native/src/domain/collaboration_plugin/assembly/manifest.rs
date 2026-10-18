use anyhow::{Result, ensure};
use sha2::{Digest, Sha256};
use std::fs;
use std::path::Path;

use super::model::{AssemblyManifest, LocalAssemblyRecord, PlannedLocalAssembly};
use super::{ASSEMBLY_ADAPTER_ID, ASSEMBLY_MANIFEST_FILE, ASSEMBLY_MANIFEST_SCHEMA};
use crate::domain::collaboration_plugin::lifecycle::InstalledWorkflowPlugin;

pub(super) fn build_manifest(
    installed: &InstalledWorkflowPlugin,
    selected_component_ids: &[String],
    plan: &PlannedLocalAssembly,
) -> Result<AssemblyManifest> {
    let manifest = AssemblyManifest {
        schema_version: ASSEMBLY_MANIFEST_SCHEMA.to_owned(),
        deployment_id: plan.deployment_id.clone(),
        plugin_id: installed.plugin_id.clone(),
        source_url: installed.source_url.clone(),
        server_version: installed.version.clone(),
        package_digest_sha256: installed.digest_sha256.clone(),
        selected_component_ids: selected_component_ids.to_vec(),
        assembly_adapter_id: ASSEMBLY_ADAPTER_ID.to_owned(),
        bind_host: plan.bind_host.clone(),
        port: plan.port,
        code_executed_during_assembly: false,
        runner_execution_requires_direct_start_approval: true,
        selected_server_code_executes_on_start: true,
        external_file_transfer_authorized: false,
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
    manifest.validate()?;
    Ok(manifest)
}

pub(super) fn manifest_bytes(manifest: &AssemblyManifest) -> Result<Vec<u8>> {
    manifest.validate()?;
    Ok(serde_json::to_vec_pretty(manifest)?)
}

pub(super) fn manifest_digest(manifest: &AssemblyManifest) -> Result<String> {
    Ok(format!("{:x}", Sha256::digest(manifest_bytes(manifest)?)))
}

pub(super) fn read_bound_manifest(record: &LocalAssemblyRecord) -> Result<AssemblyManifest> {
    record.validate()?;
    let root = Path::new(&record.destination);
    let metadata = fs::symlink_metadata(root)
        .map_err(|_| anyhow::anyhow!("collaboration_local_server_assembly_missing"))?;
    ensure!(
        metadata.file_type().is_dir() && !metadata.file_type().is_symlink(),
        "collaboration_local_server_assembly_root_invalid"
    );
    let path = root.join(ASSEMBLY_MANIFEST_FILE);
    let bytes =
        crate::domain::collaboration_plugin::package::read_file_no_follow(&path, 2 * 1024 * 1024)
            .map_err(|_| anyhow::anyhow!("collaboration_local_server_manifest_missing"))?;
    ensure!(
        bytes.len() <= 2 * 1024 * 1024,
        "collaboration_local_server_manifest_too_large"
    );
    ensure!(
        format!("{:x}", Sha256::digest(&bytes)) == record.manifest_digest_sha256,
        "collaboration_local_server_manifest_digest_mismatch"
    );
    let manifest: AssemblyManifest = serde_json::from_slice(&bytes)
        .map_err(|_| anyhow::anyhow!("collaboration_local_server_manifest_invalid"))?;
    manifest.validate()?;
    ensure!(
        manifest.deployment_id == record.deployment_id
            && manifest.plugin_id == record.plugin_id
            && manifest.source_url == record.source_url
            && manifest.server_version == record.server_version
            && manifest.package_digest_sha256 == record.package_digest_sha256
            && manifest.selected_component_ids == record.selected_component_ids
            && manifest.assembly_adapter_id == record.assembly_adapter_id
            && manifest.bind_host == record.bind_host
            && manifest.port == record.port
            && manifest.runner_platform == record.runner_platform
            && manifest.runner_architecture == record.runner_architecture
            && manifest.runner_source_relative_path == record.runner_source_relative_path
            && manifest.runner_destination_relative_path == record.runner_destination_relative_path
            && manifest.runner_digest_sha256 == record.runner_digest_sha256
            && manifest.runner_contract_version == record.runner_contract_version
            && manifest.health_contract_version == record.health_contract_version
            && manifest.capabilities_contract_version == record.capabilities_contract_version
            && manifest.signed_package_inventory_digest_sha256
                == record.signed_package_inventory_digest_sha256
            && manifest.source_commit_oid == record.source_commit_oid
            && manifest.runner_trust_key_id == record.runner_trust_key_id
            && manifest.runner_trust_fingerprint_sha256 == record.runner_trust_fingerprint_sha256
            && manifest.selected_payload_files == record.selected_payload_files
            && manifest.selected_payload_inventory_digest_sha256
                == record.selected_payload_inventory_digest_sha256,
        "collaboration_local_server_manifest_binding_mismatch"
    );
    Ok(manifest)
}
