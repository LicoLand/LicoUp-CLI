use anyhow::{Result, ensure};
use serde::{Deserialize, Serialize};

use super::{
    ASSEMBLED_RUNNER_DIRECTORY, ASSEMBLY_ADAPTER_ID, ASSEMBLY_MANIFEST_SCHEMA,
    ASSEMBLY_STATE_SCHEMA,
};
use crate::domain::collaboration_plugin::manifest::{
    SERVER_CAPABILITIES_CONTRACT, SERVER_HEALTH_CONTRACT, SERVER_RUNNER_CONTRACT,
    expected_server_runner_path,
};

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub(crate) struct AssemblyPayloadFile {
    pub(crate) selection_id: String,
    pub(crate) source_relative_path: String,
    pub(crate) destination_relative_path: String,
    pub(crate) digest_sha256: String,
    pub(crate) bytes: usize,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub(crate) struct PlannedLocalAssembly {
    pub(crate) deployment_id: String,
    pub(crate) source_url: String,
    pub(crate) server_version: String,
    pub(crate) assembly_adapter_id: String,
    pub(crate) bind_host: String,
    pub(crate) port: u16,
    pub(crate) manifest_digest_sha256: String,
    pub(crate) manifest_bytes: usize,
    pub(crate) sealed_snapshot_digest_sha256: String,
    pub(crate) sealed_snapshot_bytes: usize,
    pub(crate) runner_platform: String,
    pub(crate) runner_architecture: String,
    pub(crate) runner_source_relative_path: String,
    pub(crate) runner_destination_relative_path: String,
    pub(crate) runner_digest_sha256: String,
    pub(crate) runner_contract_version: String,
    pub(crate) health_contract_version: String,
    pub(crate) capabilities_contract_version: String,
    pub(crate) signed_package_inventory_digest_sha256: String,
    pub(crate) source_commit_oid: String,
    pub(crate) runner_trust_key_id: String,
    pub(crate) runner_trust_fingerprint_sha256: String,
    pub(crate) selected_payload_files: Vec<AssemblyPayloadFile>,
    pub(crate) selected_payload_inventory_digest_sha256: String,
}

impl PlannedLocalAssembly {
    pub(crate) fn validate(&self) -> Result<()> {
        ensure!(
            uuid::Uuid::parse_str(&self.deployment_id)
                .is_ok_and(|value| value.to_string() == self.deployment_id),
            "collaboration_local_server_deployment_id_invalid"
        );
        super::payload_inventory::validate(&self.selected_payload_files)?;
        ensure!(
            super::payload_inventory::digest(&self.selected_payload_files)?
                == self.selected_payload_inventory_digest_sha256,
            "collaboration_local_server_payload_inventory_digest_mismatch"
        );
        ensure!(
            self.source_url.starts_with("https://github.com/")
                && self.source_url.ends_with(".git")
                && self.server_version == self.server_version.trim()
                && !self.server_version.is_empty()
                && self.server_version.len() <= 255,
            "collaboration_local_server_source_binding_invalid"
        );
        ensure!(
            self.assembly_adapter_id == ASSEMBLY_ADAPTER_ID
                && self.bind_host == "127.0.0.1"
                && self.port >= 1024
                && is_sha256(&self.manifest_digest_sha256)
                && self.manifest_bytes > 0
                && self.manifest_bytes <= 2 * 1024 * 1024
                && is_sha256(&self.sealed_snapshot_digest_sha256)
                && self.sealed_snapshot_bytes > 0
                && self.sealed_snapshot_bytes <= super::snapshot::MAX_SNAPSHOT_BYTES,
            "collaboration_local_server_build_plan_invalid"
        );
        let expected_source =
            expected_server_runner_path(&self.runner_platform, &self.runner_architecture)
                .to_string_lossy()
                .replace('\\', "/");
        let expected_destination = assembled_runner_relative_path(&self.runner_platform);
        ensure!(
            matches!(
                self.runner_platform.as_str(),
                "macos" | "windows" | "ubuntu"
            ) && matches!(self.runner_architecture.as_str(), "x86_64" | "aarch64")
                && self.runner_source_relative_path == expected_source
                && self.runner_destination_relative_path == expected_destination
                && is_sha256(&self.runner_digest_sha256)
                && self.runner_contract_version == SERVER_RUNNER_CONTRACT
                && self.health_contract_version == SERVER_HEALTH_CONTRACT
                && self.capabilities_contract_version == SERVER_CAPABILITIES_CONTRACT,
            "collaboration_local_server_runner_contract_invalid"
        );
        ensure!(
            is_sha256(&self.signed_package_inventory_digest_sha256)
                && self.source_commit_oid.len() == 40
                && self
                    .source_commit_oid
                    .bytes()
                    .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
                && !self.runner_trust_key_id.is_empty()
                && self.runner_trust_key_id.len() <= 128
                && is_sha256(&self.runner_trust_fingerprint_sha256),
            "collaboration_local_server_runner_trust_binding_invalid"
        );
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum LocalServerLifecycle {
    Stopped,
    Starting,
    Running,
    Stopping,
    Quarantined,
}

impl LocalServerLifecycle {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Stopped => "assembled-awaiting-deployment",
            Self::Starting => "deployment-starting",
            Self::Running => "running",
            Self::Stopping => "deployment-stopping",
            Self::Quarantined => "quarantined-runtime-identity",
        }
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub(crate) struct LocalAssemblyRecord {
    pub(crate) schema_version: String,
    pub(crate) deployment_id: String,
    pub(crate) plugin_id: String,
    pub(crate) source_url: String,
    pub(crate) server_version: String,
    pub(crate) package_digest_sha256: String,
    pub(crate) selected_component_ids: Vec<String>,
    pub(crate) destination: String,
    pub(crate) assembly_adapter_id: String,
    pub(crate) bind_host: String,
    pub(crate) port: u16,
    pub(crate) manifest_digest_sha256: String,
    pub(crate) destination_digest_sha256: String,
    pub(crate) sealed_snapshot_digest_sha256: String,
    pub(crate) sealed_snapshot_bytes: usize,
    pub(crate) runtime_generation: u64,
    pub(crate) execution_started: bool,
    pub(crate) lifecycle: LocalServerLifecycle,
    pub(crate) runtime_pid: Option<u32>,
    pub(crate) runtime_instance_id: Option<String>,
    pub(crate) runtime_process_identity: Option<String>,
    pub(crate) runner_platform: String,
    pub(crate) runner_architecture: String,
    pub(crate) runner_source_relative_path: String,
    pub(crate) runner_destination_relative_path: String,
    pub(crate) runner_digest_sha256: String,
    pub(crate) runner_contract_version: String,
    pub(crate) health_contract_version: String,
    pub(crate) capabilities_contract_version: String,
    pub(crate) signed_package_inventory_digest_sha256: String,
    pub(crate) source_commit_oid: String,
    pub(crate) runner_trust_key_id: String,
    pub(crate) runner_trust_fingerprint_sha256: String,
    pub(crate) selected_payload_files: Vec<AssemblyPayloadFile>,
    pub(crate) selected_payload_inventory_digest_sha256: String,
}

impl LocalAssemblyRecord {
    pub(crate) fn validate(&self) -> Result<()> {
        ensure!(
            self.schema_version == ASSEMBLY_STATE_SCHEMA,
            "collaboration_local_server_state_schema_invalid"
        );
        PlannedLocalAssembly {
            deployment_id: self.deployment_id.clone(),
            source_url: self.source_url.clone(),
            server_version: self.server_version.clone(),
            assembly_adapter_id: self.assembly_adapter_id.clone(),
            bind_host: self.bind_host.clone(),
            port: self.port,
            manifest_digest_sha256: self.manifest_digest_sha256.clone(),
            manifest_bytes: 1,
            sealed_snapshot_digest_sha256: self.sealed_snapshot_digest_sha256.clone(),
            sealed_snapshot_bytes: self.sealed_snapshot_bytes,
            runner_platform: self.runner_platform.clone(),
            runner_architecture: self.runner_architecture.clone(),
            runner_source_relative_path: self.runner_source_relative_path.clone(),
            runner_destination_relative_path: self.runner_destination_relative_path.clone(),
            runner_digest_sha256: self.runner_digest_sha256.clone(),
            runner_contract_version: self.runner_contract_version.clone(),
            health_contract_version: self.health_contract_version.clone(),
            capabilities_contract_version: self.capabilities_contract_version.clone(),
            signed_package_inventory_digest_sha256: self
                .signed_package_inventory_digest_sha256
                .clone(),
            source_commit_oid: self.source_commit_oid.clone(),
            runner_trust_key_id: self.runner_trust_key_id.clone(),
            runner_trust_fingerprint_sha256: self.runner_trust_fingerprint_sha256.clone(),
            selected_payload_files: self.selected_payload_files.clone(),
            selected_payload_inventory_digest_sha256: self
                .selected_payload_inventory_digest_sha256
                .clone(),
        }
        .validate()?;
        ensure!(
            is_slug(&self.plugin_id)
                && is_sha256(&self.package_digest_sha256)
                && !self.selected_component_ids.is_empty()
                && self.selected_component_ids.len() <= 256
                && self
                    .selected_component_ids
                    .windows(2)
                    .all(|pair| pair[0] < pair[1])
                && self
                    .selected_component_ids
                    .iter()
                    .all(|value| is_slug(value))
                && !self.destination.is_empty(),
            "collaboration_local_server_state_invalid"
        );
        ensure!(
            std::path::Path::new(&self.destination).is_absolute()
                && super::snapshot::destination_digest(std::path::Path::new(&self.destination))?
                    == self.destination_digest_sha256
                && self.runtime_generation > 0,
            "collaboration_local_server_authority_binding_invalid"
        );
        ensure!(
            matches!(self.lifecycle, LocalServerLifecycle::Stopped) == self.runtime_pid.is_none()
                && self.runtime_pid.is_none() == self.runtime_instance_id.is_none()
                && self.runtime_pid.is_none() == self.runtime_process_identity.is_none()
                && self.runtime_instance_id.as_ref().map_or(true, |value| {
                    uuid::Uuid::parse_str(value).is_ok_and(|parsed| parsed.to_string() == *value)
                })
                && self
                    .runtime_process_identity
                    .as_ref()
                    .map_or(true, |value| {
                        value == value.trim()
                            && !value.is_empty()
                            && value.len() <= 512
                            && value.bytes().all(|byte| byte.is_ascii_graphic())
                    }),
            "collaboration_local_server_process_state_invalid"
        );
        Ok(())
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub(super) struct AssemblyManifest {
    pub(super) schema_version: String,
    pub(super) deployment_id: String,
    pub(super) plugin_id: String,
    pub(super) source_url: String,
    pub(super) server_version: String,
    pub(super) package_digest_sha256: String,
    pub(super) selected_component_ids: Vec<String>,
    pub(super) assembly_adapter_id: String,
    pub(super) bind_host: String,
    pub(super) port: u16,
    pub(super) code_executed_during_assembly: bool,
    pub(super) runner_execution_requires_direct_start_approval: bool,
    pub(super) selected_server_code_executes_on_start: bool,
    pub(super) external_file_transfer_authorized: bool,
    pub(super) runner_platform: String,
    pub(super) runner_architecture: String,
    pub(super) runner_source_relative_path: String,
    pub(super) runner_destination_relative_path: String,
    pub(super) runner_digest_sha256: String,
    pub(super) runner_contract_version: String,
    pub(super) health_contract_version: String,
    pub(super) capabilities_contract_version: String,
    pub(super) signed_package_inventory_digest_sha256: String,
    pub(super) source_commit_oid: String,
    pub(super) runner_trust_key_id: String,
    pub(super) runner_trust_fingerprint_sha256: String,
    pub(super) selected_payload_files: Vec<AssemblyPayloadFile>,
    pub(super) selected_payload_inventory_digest_sha256: String,
}

impl AssemblyManifest {
    pub(super) fn validate(&self) -> Result<()> {
        ensure!(
            self.schema_version == ASSEMBLY_MANIFEST_SCHEMA
                && self.assembly_adapter_id == ASSEMBLY_ADAPTER_ID
                && self.bind_host == "127.0.0.1"
                && !self.code_executed_during_assembly
                && self.runner_execution_requires_direct_start_approval
                && self.selected_server_code_executes_on_start
                && !self.external_file_transfer_authorized
                && is_slug(&self.plugin_id)
                && is_sha256(&self.package_digest_sha256)
                && !self.selected_component_ids.is_empty()
                && self
                    .selected_component_ids
                    .windows(2)
                    .all(|pair| pair[0] < pair[1]),
            "collaboration_local_server_manifest_invalid"
        );
        super::payload_inventory::validate(&self.selected_payload_files)?;
        ensure!(
            super::payload_inventory::digest(&self.selected_payload_files)?
                == self.selected_payload_inventory_digest_sha256,
            "collaboration_local_server_payload_inventory_digest_mismatch"
        );
        ensure!(
            self.runner_source_relative_path
                == expected_server_runner_path(&self.runner_platform, &self.runner_architecture)
                    .to_string_lossy()
                    .replace('\\', "/")
                && self.runner_destination_relative_path
                    == assembled_runner_relative_path(&self.runner_platform)
                && is_sha256(&self.runner_digest_sha256)
                && self.runner_contract_version == SERVER_RUNNER_CONTRACT
                && self.health_contract_version == SERVER_HEALTH_CONTRACT
                && self.capabilities_contract_version == SERVER_CAPABILITIES_CONTRACT,
            "collaboration_local_server_runner_contract_invalid"
        );
        ensure!(
            is_sha256(&self.signed_package_inventory_digest_sha256)
                && self.source_commit_oid.len() == 40
                && !self.runner_trust_key_id.is_empty()
                && is_sha256(&self.runner_trust_fingerprint_sha256),
            "collaboration_local_server_runner_trust_binding_invalid"
        );
        Ok(())
    }
}

pub(super) fn assembled_runner_relative_path(platform: &str) -> String {
    let executable = if platform == "windows" {
        "licomesh-server-runner.exe"
    } else {
        "licomesh-server-runner"
    };
    format!("{ASSEMBLED_RUNNER_DIRECTORY}/{executable}")
}

fn is_slug(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && value
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
}

fn is_sha256(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
}
