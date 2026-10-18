use anyhow::{Result, anyhow, ensure};
use sha2::{Digest, Sha256};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use super::super::manifest::read_bound_manifest;
use super::super::model::{AssemblyManifest, LocalAssemblyRecord};
use super::lease;
use crate::domain::collaboration_plugin::package::{
    inspect_package, local_deployment_choices, read_file_no_follow, select_current_server_runner,
    selected_payload_files,
};
use crate::platform::client_state::ClientStateStore;

pub(in crate::domain::collaboration_plugin::assembly) fn verify_assembly(
    store: &ClientStateStore,
    record: &LocalAssemblyRecord,
) -> Result<AssemblyManifest> {
    let manifest = verify_assembly_source_and_artifact(store, record)?;
    let (_, authority) = crate::domain::collaboration_plugin::lifecycle::verified_authority(
        store,
        "Verify the protected authority for the exact local-server assembly",
    )?;
    authority.authority.ensure_assembly(record)?;
    Ok(manifest)
}

pub(in crate::domain::collaboration_plugin::assembly) fn verify_assembly_source_and_artifact(
    store: &ClientStateStore,
    record: &LocalAssemblyRecord,
) -> Result<AssemblyManifest> {
    record.validate()?;
    let installed =
        crate::domain::collaboration_plugin::lifecycle::installed_workflow_plugin(store)?;
    ensure!(
        record.plugin_id == installed.plugin_id
            && record.package_digest_sha256 == installed.digest_sha256
            && record.source_url == installed.source_url
            && record.server_version == installed.version
            && record.source_commit_oid == installed.source_commit_oid
            && record.signed_package_inventory_digest_sha256
                == installed.signed_package_inventory_digest_sha256
            && record.runner_trust_key_id == installed.runner_trust_key_id
            && record.runner_trust_fingerprint_sha256 == installed.runner_trust_fingerprint_sha256,
        "collaboration_local_server_installed_source_changed"
    );
    let package = inspect_package(&installed.package_root)?;
    ensure!(
        package.digest_sha256 == installed.digest_sha256
            && package.manifest.signed_package_inventory_digest_sha256
                == installed.signed_package_inventory_digest_sha256,
        "collaboration_local_server_installed_package_digest_mismatch"
    );
    let packaged_runner = select_current_server_runner(&package)?;
    super::super::apply::verify_runner_trust(&installed, &package, &packaged_runner)?;
    ensure!(
        packaged_runner.contract.platform == record.runner_platform
            && packaged_runner.contract.architecture == record.runner_architecture
            && crate::domain::collaboration_plugin::manifest::normalized_relative_protocol_path(
                &packaged_runner.contract.relative_path,
            )? == record.runner_source_relative_path
            && packaged_runner.contract.digest_sha256 == record.runner_digest_sha256
            && packaged_runner.contract.runner_contract_version == record.runner_contract_version
            && packaged_runner.contract.health_contract_version == record.health_contract_version
            && packaged_runner.contract.capabilities_contract_version
                == record.capabilities_contract_version,
        "collaboration_local_server_runner_binding_changed"
    );
    let choices = local_deployment_choices(&package)?;
    let expected_payload =
        selected_payload_files(&package, &choices, &record.selected_component_ids, true)?;
    let expected_inventory = super::super::payload_inventory::from_selected(&expected_payload)?;
    ensure!(
        expected_inventory == record.selected_payload_files
            && super::super::payload_inventory::digest(&expected_inventory)?
                == record.selected_payload_inventory_digest_sha256,
        "collaboration_local_server_payload_source_binding_mismatch"
    );
    verify_assembly_artifact(record)
}

pub(in crate::domain::collaboration_plugin::assembly) fn verify_assembly_artifact(
    record: &LocalAssemblyRecord,
) -> Result<AssemblyManifest> {
    let manifest = read_bound_manifest(record)?;
    let root = Path::new(&record.destination);
    super::super::payload_inventory::verify_tree(
        root,
        &record.selected_payload_files,
        &record.runner_destination_relative_path,
    )?;
    super::super::snapshot::verify(record)?;
    verify_assembled_runner(record)?;
    Ok(manifest)
}

pub(in crate::domain::collaboration_plugin::assembly) struct SpawnedRuntime {
    pub(in crate::domain::collaboration_plugin::assembly) pid: u32,
    pub(in crate::domain::collaboration_plugin::assembly) process_identity: String,
}

pub(super) fn spawn(
    store: &ClientStateStore,
    record: &LocalAssemblyRecord,
) -> Result<SpawnedRuntime> {
    let mut prepared = command(store, record)?;
    let mut child = prepared
        .command
        .spawn()
        .map_err(|_| anyhow!("collaboration_local_server_start_failed"))?;
    let pid = child.id();
    match super::process::capture_identity(pid) {
        Ok(process_identity) => {
            super::supervisor::register(record, child, process_identity.clone())?;
            Ok(SpawnedRuntime {
                pid,
                process_identity,
            })
        }
        Err(error) => {
            let _ = child.kill();
            let _ = child.wait();
            Err(error)
        }
    }
}

struct PreparedRuntimeCommand {
    command: Command,
    _runner_image: super::immutable_file::ImmutableRuntimeFile,
    _manifest_image: super::immutable_file::ImmutableRuntimeFile,
    _snapshot_image: super::immutable_file::ImmutableRuntimeFile,
}

fn command(
    store: &ClientStateStore,
    record: &LocalAssemblyRecord,
) -> Result<PreparedRuntimeCommand> {
    let runtime_instance_id = record
        .runtime_instance_id
        .as_deref()
        .ok_or_else(|| anyhow!("collaboration_local_server_runtime_instance_missing"))?;
    let destination = PathBuf::from(&record.destination);
    ensure!(
        destination.is_absolute(),
        "collaboration_local_server_destination_invalid"
    );
    let runner_bytes = load_verified_runner(record)?;
    let manifest_path = destination.join(super::super::ASSEMBLY_MANIFEST_FILE);
    let manifest_bytes = read_file_no_follow(&manifest_path, 2 * 1024 * 1024)?;
    ensure!(
        format!("{:x}", Sha256::digest(&manifest_bytes)) == record.manifest_digest_sha256,
        "collaboration_local_server_manifest_digest_mismatch"
    );
    let snapshot_path = destination.join(super::super::ASSEMBLY_SNAPSHOT_FILE);
    let snapshot_bytes = read_file_no_follow(&snapshot_path, record.sealed_snapshot_bytes)?;
    ensure!(
        snapshot_bytes.len() == record.sealed_snapshot_bytes
            && super::super::snapshot::digest(&snapshot_bytes)
                == record.sealed_snapshot_digest_sha256,
        "collaboration_local_server_snapshot_digest_mismatch"
    );
    let runner_image = super::immutable_file::ImmutableRuntimeFile::from_verified_bytes(
        store,
        &runner_bytes,
        true,
    )?;
    let manifest_image = super::immutable_file::ImmutableRuntimeFile::from_verified_bytes(
        store,
        &manifest_bytes,
        false,
    )?;
    let snapshot_image = super::immutable_file::ImmutableRuntimeFile::from_verified_bytes(
        store,
        &snapshot_bytes,
        false,
    )?;
    let runtime_data = destination.join(super::super::ASSEMBLED_RUNTIME_DATA_DIRECTORY);
    validate_runtime_data_directory(&runtime_data)?;
    let lease_path = lease::prepare(store, &record.deployment_id)?;
    let mut command = super::sandbox::command(
        runner_image.path(),
        manifest_image.path(),
        snapshot_image.path(),
        &runtime_data,
        record.port,
    )?;
    command
        .env_clear()
        .current_dir(&destination)
        .args([
            "serve",
            "--runner-contract-version",
            &record.runner_contract_version,
            "--health-contract-version",
            &record.health_contract_version,
            "--capabilities-contract-version",
            &record.capabilities_contract_version,
            "--assembly-manifest",
        ])
        .arg(manifest_image.path())
        .arg("--assembly-snapshot")
        .arg(snapshot_image.path())
        .args(["--bind-host", "127.0.0.1", "--port"])
        .arg(record.port.to_string())
        .arg("--runtime-lease")
        .arg(&lease_path)
        .arg("--runtime-data")
        .arg(&runtime_data)
        .args(["--runtime-instance-id", runtime_instance_id])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        const DETACHED_PROCESS: u32 = 0x0000_0008;
        command.creation_flags(CREATE_NO_WINDOW | DETACHED_PROCESS);
    }
    Ok(PreparedRuntimeCommand {
        command,
        _runner_image: runner_image,
        _manifest_image: manifest_image,
        _snapshot_image: snapshot_image,
    })
}

pub(super) fn runner_path(record: &LocalAssemblyRecord) -> PathBuf {
    Path::new(&record.destination).join(&record.runner_destination_relative_path)
}

fn verify_assembled_runner(record: &LocalAssemblyRecord) -> Result<()> {
    let _ = load_verified_runner(record)?;
    Ok(())
}

fn load_verified_runner(record: &LocalAssemblyRecord) -> Result<Vec<u8>> {
    let path = runner_path(record);
    let declared_bytes = fs::symlink_metadata(&path)
        .map_err(|_| anyhow!("collaboration_local_server_runner_missing"))?
        .len();
    let maximum = usize::try_from(declared_bytes)
        .map_err(|_| anyhow!("collaboration_local_server_runner_invalid"))?;
    ensure!(
        maximum > 0 && maximum <= 32 * 1024 * 1024,
        "collaboration_local_server_runner_invalid"
    );
    let bytes = read_file_no_follow(&path, maximum)?;
    ensure!(
        format!("{:x}", Sha256::digest(&bytes)) == record.runner_digest_sha256,
        "collaboration_local_server_runner_digest_mismatch"
    );
    validate_runner_permissions(&path)?;
    Ok(bytes)
}

fn validate_runtime_data_directory(path: &Path) -> Result<()> {
    let metadata = fs::symlink_metadata(path)
        .map_err(|_| anyhow!("collaboration_local_server_runtime_data_invalid"))?;
    ensure!(
        metadata.file_type().is_dir() && !metadata.file_type().is_symlink(),
        "collaboration_local_server_runtime_data_invalid"
    );
    Ok(())
}

#[cfg(unix)]
fn validate_runner_permissions(path: &Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    ensure!(
        fs::symlink_metadata(path)?.permissions().mode() & 0o777 == 0o700,
        "collaboration_local_server_runner_permissions_invalid"
    );
    Ok(())
}

#[cfg(not(unix))]
fn validate_runner_permissions(_path: &Path) -> Result<()> {
    Ok(())
}
