use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Result, anyhow, ensure};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use uuid::Uuid;

use crate::platform::client_state::ClientStateStore;

use super::authority::AuthorityRegistration;
#[cfg(test)]
use super::lifecycle::installed_workflow_plugin;
use super::lifecycle::{InstalledWorkflowPlugin, collaboration_root};
use super::package::{SelectedPayloadFile, WorkflowChoice};

pub(super) const REGISTRATION_SCHEMA: &str = "licoup.mcp-agent-registration.v2";
const REGISTRATION_ROOT: &str = "mcp-agent-registrations";
const MAX_REGISTRATION_BYTES: usize = 2 * 1024 * 1024;
pub(super) const OUTBOUND_POLICY: &str = "direct-user-exact-scope-one-shot";
pub(super) const ACTIVATION_POLICY: &str = "disabled-authenticated-broker-unavailable";
pub(super) const BRIDGE_KIND: &str = "licoup-stdio-mcp-gate";

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub(in crate::domain::collaboration_plugin) struct AgentDestination {
    pub(in crate::domain::collaboration_plugin) agent_id: String,
    pub(in crate::domain::collaboration_plugin) install_destination: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub(in crate::domain::collaboration_plugin) struct PlannedAgentRegistration {
    pub(in crate::domain::collaboration_plugin) agent_id: String,
    pub(in crate::domain::collaboration_plugin) registration_id: String,
    pub(in crate::domain::collaboration_plugin) destination: String,
    pub(in crate::domain::collaboration_plugin) digest_sha256: String,
    pub(in crate::domain::collaboration_plugin) content: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub(crate) struct McpAgentRegistration {
    pub(super) schema_version: String,
    pub(super) registration_id: String,
    pub(super) registration_digest_sha256: String,
    pub(super) agent_id: String,
    pub(super) collaboration_plugin_id: String,
    pub(super) package_digest_sha256: String,
    pub(super) selected_plugin_ids: Vec<String>,
    pub(super) payload_roots: Vec<RegistrationPayloadRoot>,
    pub(super) payload_files: Vec<RegistrationPayloadFile>,
    pub(super) servers: Vec<RegistrationServer>,
    pub(super) bridge_kind: String,
    pub(super) activation_policy: String,
    pub(super) automatic_triggers_allowed: bool,
    pub(super) plugin_executed_during_install: bool,
    pub(super) external_file_transfer_authorized: bool,
    pub(super) outbound_policy: String,
    pub(super) requires_direct_user_confirmation: bool,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub(super) struct RegistrationPayloadRoot {
    pub(super) plugin_id: String,
    pub(super) path: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub(super) struct RegistrationPayloadFile {
    pub(super) plugin_id: String,
    pub(super) relative_path: String,
    pub(super) digest_sha256: String,
    pub(super) bytes: usize,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub(crate) struct RegistrationServer {
    pub(crate) plugin_id: String,
    pub(crate) transport: String,
    pub(crate) endpoint: String,
}

impl McpAgentRegistration {
    pub(super) fn seal(&mut self) -> Result<()> {
        self.registration_digest_sha256.clear();
        self.registration_digest_sha256 = digest_record(self)?;
        Ok(())
    }

    fn validate(&self) -> Result<()> {
        ensure!(
            self.schema_version == REGISTRATION_SCHEMA
                && is_uuid(&self.registration_id)
                && is_sha256(&self.registration_digest_sha256)
                && is_sha256(&self.package_digest_sha256)
                && self.bridge_kind == BRIDGE_KIND
                && self.activation_policy == ACTIVATION_POLICY
                && !self.automatic_triggers_allowed
                && !self.plugin_executed_during_install
                && !self.external_file_transfer_authorized
                && self.outbound_policy == OUTBOUND_POLICY
                && self.requires_direct_user_confirmation,
            "collaboration_mcp_registration_policy_invalid"
        );
        ensure!(
            canonical_agent_id(&self.agent_id).as_deref() == Some(self.agent_id.as_str()),
            "collaboration_mcp_registration_agent_invalid"
        );
        let mut unsigned = self.clone();
        unsigned.registration_digest_sha256.clear();
        ensure!(
            digest_record(&unsigned)? == self.registration_digest_sha256,
            "collaboration_mcp_registration_binding_invalid"
        );
        ensure!(
            !self.selected_plugin_ids.is_empty()
                && self.selected_plugin_ids.len() == self.servers.len()
                && self.selected_plugin_ids.len() == self.payload_roots.len()
                && self
                    .selected_plugin_ids
                    .windows(2)
                    .all(|pair| pair[0] < pair[1]),
            "collaboration_mcp_registration_selection_invalid"
        );
        for (index, plugin_id) in self.selected_plugin_ids.iter().enumerate() {
            ensure!(
                self.servers[index].plugin_id == *plugin_id
                    && self.servers[index].transport == "streamable-http"
                    && self.payload_roots[index].plugin_id == *plugin_id
                    && Path::new(&self.payload_roots[index].path).is_absolute(),
                "collaboration_mcp_registration_server_invalid"
            );
            validate_endpoint(&self.servers[index].endpoint)?;
        }
        ensure!(
            !self.payload_files.is_empty()
                && self.payload_files.iter().all(|file| {
                    self.selected_plugin_ids.contains(&file.plugin_id)
                        && is_relative_safe_path(&file.relative_path)
                        && is_sha256(&file.digest_sha256)
                })
                && self.payload_files.windows(2).all(|pair| {
                    (&pair[0].plugin_id, &pair[0].relative_path)
                        < (&pair[1].plugin_id, &pair[1].relative_path)
                }),
            "collaboration_mcp_registration_payload_invalid"
        );
        Ok(())
    }
}

pub(in crate::domain::collaboration_plugin) fn build_registrations(
    store: &ClientStateStore,
    installed: &InstalledWorkflowPlugin,
    choices: &[WorkflowChoice],
    selected_ids: &[String],
    destinations: &[AgentDestination],
    payload: &[SelectedPayloadFile],
) -> Result<Vec<PlannedAgentRegistration>> {
    let choices = choices
        .iter()
        .map(|choice| (choice.id.as_str(), choice))
        .collect::<BTreeMap<_, _>>();
    let root = registration_root(store);
    licoup_foundation::platform::file_security::ensure_private_dir(&root)?;
    let mut registrations = Vec::with_capacity(destinations.len());
    for destination in destinations {
        let agent_root = root.join(&destination.agent_id);
        licoup_foundation::platform::file_security::ensure_private_dir(&agent_root)?;
        let registration_id = Uuid::new_v4().to_string();
        registrations.push(build_registration(
            &root,
            installed,
            &choices,
            selected_ids,
            destination,
            payload,
            &registration_id,
        )?);
    }
    Ok(registrations)
}

pub(in crate::domain::collaboration_plugin) fn revalidate_registrations(
    store: &ClientStateStore,
    installed: &InstalledWorkflowPlugin,
    choices: &[WorkflowChoice],
    selected_ids: &[String],
    destinations: &[AgentDestination],
    payload: &[SelectedPayloadFile],
    planned: &[PlannedAgentRegistration],
) -> Result<()> {
    ensure!(
        destinations.len() == planned.len(),
        "collaboration_mcp_registration_count_changed"
    );
    let choices = choices
        .iter()
        .map(|choice| (choice.id.as_str(), choice))
        .collect::<BTreeMap<_, _>>();
    let root = registration_root(store);
    for destination in destinations {
        let registration = planned
            .iter()
            .find(|registration| registration.agent_id == destination.agent_id)
            .ok_or_else(|| anyhow!("collaboration_mcp_registration_missing"))?;
        let expected = build_registration(
            &root,
            installed,
            &choices,
            selected_ids,
            destination,
            payload,
            &registration.registration_id,
        )?;
        ensure!(
            expected == *registration,
            "collaboration_mcp_registration_changed"
        );
    }
    Ok(())
}

pub(in crate::domain::collaboration_plugin) fn authority_bindings(
    store: &ClientStateStore,
    destinations: &[AgentDestination],
    planned: &[PlannedAgentRegistration],
) -> Result<Vec<AuthorityRegistration>> {
    ensure!(
        destinations.len() == planned.len(),
        "collaboration_mcp_registration_count_changed"
    );
    let mut bindings = Vec::with_capacity(planned.len());
    for destination in destinations {
        let registration = planned
            .iter()
            .find(|registration| registration.agent_id == destination.agent_id)
            .ok_or_else(|| anyhow!("collaboration_mcp_registration_missing"))?;
        let expected_registration_destination = registration_root(store)
            .join(&destination.agent_id)
            .join(format!("{}.json", registration.registration_id));
        ensure!(
            Path::new(&registration.destination) == expected_registration_destination,
            "collaboration_mcp_registration_destination_changed"
        );
        let record = planned_record(registration)?;
        ensure!(
            record.agent_id == destination.agent_id
                && record.registration_id == registration.registration_id
                && record.payload_roots.iter().all(|root| {
                    Path::new(&root.path)
                        == Path::new(&destination.install_destination).join(&root.plugin_id)
                }),
            "collaboration_mcp_registration_binding_invalid"
        );
        bindings.push(authority_binding(destination, registration, &record)?);
    }
    bindings.sort_by(|left, right| left.registration_id.cmp(&right.registration_id));
    ensure!(
        bindings
            .windows(2)
            .all(|pair| pair[0].registration_id < pair[1].registration_id),
        "collaboration_authority_registration_collection_invalid"
    );
    Ok(bindings)
}

pub(in crate::domain::collaboration_plugin) fn verify_installed_registration_targets(
    installed: &InstalledWorkflowPlugin,
    destinations: &[AgentDestination],
    planned: &[PlannedAgentRegistration],
) -> Result<()> {
    for destination in destinations {
        let registration = planned
            .iter()
            .find(|registration| registration.agent_id == destination.agent_id)
            .ok_or_else(|| anyhow!("collaboration_mcp_registration_missing"))?;
        verify_registration_target(registration, Path::new(&registration.destination))?;
        verify_payload_target(registration, Path::new(&destination.install_destination))?;
        validate_runtime_binding(&planned_record(registration)?, installed)?;
    }
    Ok(())
}

pub(in crate::domain::collaboration_plugin) fn verify_registration_target(
    registration: &PlannedAgentRegistration,
    path: &Path,
) -> Result<()> {
    let bytes = super::package::read_file_no_follow(path, MAX_REGISTRATION_BYTES)
        .map_err(|_| anyhow!("collaboration_mcp_registration_file_invalid"))?;
    ensure!(
        bytes == registration.content.as_bytes()
            && sha256_hex(&bytes) == registration.digest_sha256,
        "collaboration_mcp_registration_changed"
    );
    let actual: McpAgentRegistration = serde_json::from_slice(&bytes)
        .map_err(|_| anyhow!("collaboration_mcp_registration_file_invalid"))?;
    actual.validate()?;
    ensure!(
        actual.registration_id == registration.registration_id
            && actual.agent_id == registration.agent_id,
        "collaboration_mcp_registration_binding_invalid"
    );
    Ok(())
}

pub(in crate::domain::collaboration_plugin) fn verify_payload_target(
    registration: &PlannedAgentRegistration,
    root: &Path,
) -> Result<()> {
    let record = planned_record(registration)?;
    let metadata = fs::symlink_metadata(root)
        .map_err(|_| anyhow!("collaboration_mcp_registration_payload_changed"))?;
    ensure!(
        metadata.is_dir() && !metadata.file_type().is_symlink(),
        "collaboration_mcp_registration_payload_changed"
    );
    let expected = record
        .payload_files
        .iter()
        .map(|file| {
            (
                file.relative_path.clone(),
                (file.digest_sha256.clone(), file.bytes),
            )
        })
        .collect::<BTreeMap<_, _>>();
    let actual = collect_payload_files(root)?;
    ensure!(
        actual.keys().eq(expected.keys()),
        "collaboration_mcp_registration_payload_changed"
    );
    for (relative, (digest, bytes)) in expected {
        let payload = super::package::read_file_no_follow(&root.join(&relative), bytes)
            .map_err(|_| anyhow!("collaboration_mcp_registration_payload_changed"))?;
        ensure!(
            payload.len() == bytes && sha256_hex(&payload) == digest,
            "collaboration_mcp_registration_payload_changed"
        );
    }
    Ok(())
}

fn build_registration(
    root: &Path,
    installed: &InstalledWorkflowPlugin,
    choices: &BTreeMap<&str, &WorkflowChoice>,
    selected_ids: &[String],
    destination: &AgentDestination,
    payload: &[SelectedPayloadFile],
    registration_id: &str,
) -> Result<PlannedAgentRegistration> {
    ensure!(
        is_uuid(registration_id),
        "collaboration_mcp_registration_id_invalid"
    );
    let registration_path = root
        .join(&destination.agent_id)
        .join(format!("{registration_id}.json"));
    let payload_roots = selected_ids
        .iter()
        .map(|plugin_id| RegistrationPayloadRoot {
            plugin_id: plugin_id.clone(),
            path: PathBuf::from(&destination.install_destination)
                .join(plugin_id)
                .to_string_lossy()
                .into_owned(),
        })
        .collect::<Vec<_>>();
    let payload_files = payload
        .iter()
        .map(|file| RegistrationPayloadFile {
            plugin_id: file.selection_id.clone(),
            relative_path: file
                .destination_relative_path
                .to_string_lossy()
                .into_owned(),
            digest_sha256: file.digest_sha256.clone(),
            bytes: file.bytes.len(),
        })
        .collect::<Vec<_>>();
    let servers = selected_ids
        .iter()
        .map(|plugin_id| {
            let choice = choices
                .get(plugin_id.as_str())
                .ok_or_else(|| anyhow!("collaboration_mcp_registration_selection_invalid"))?;
            Ok(RegistrationServer {
                plugin_id: plugin_id.clone(),
                transport: "streamable-http".to_owned(),
                endpoint: choice
                    .endpoint
                    .clone()
                    .ok_or_else(|| anyhow!("collaboration_mcp_registration_endpoint_missing"))?,
            })
        })
        .collect::<Result<Vec<_>>>()?;
    let mut record = McpAgentRegistration {
        schema_version: REGISTRATION_SCHEMA.to_owned(),
        registration_id: registration_id.to_owned(),
        registration_digest_sha256: String::new(),
        agent_id: destination.agent_id.clone(),
        collaboration_plugin_id: installed.plugin_id.clone(),
        package_digest_sha256: installed.digest_sha256.clone(),
        selected_plugin_ids: selected_ids.to_vec(),
        payload_roots,
        payload_files,
        servers,
        bridge_kind: BRIDGE_KIND.to_owned(),
        activation_policy: ACTIVATION_POLICY.to_owned(),
        automatic_triggers_allowed: false,
        plugin_executed_during_install: false,
        external_file_transfer_authorized: false,
        outbound_policy: OUTBOUND_POLICY.to_owned(),
        requires_direct_user_confirmation: true,
    };
    record.seal()?;
    record.validate()?;
    let mut content = serde_json::to_string_pretty(&record)?;
    content.push('\n');
    Ok(PlannedAgentRegistration {
        agent_id: destination.agent_id.clone(),
        registration_id: registration_id.to_owned(),
        destination: path_text(&registration_path)?,
        digest_sha256: sha256_hex(content.as_bytes()),
        content,
    })
}

pub(crate) fn acp_servers_for_runtime(runtime_id: &str) -> Result<Vec<Value>> {
    let Some(agent_id) = canonical_agent_id(runtime_id) else {
        return Ok(Vec::new());
    };
    let store = ClientStateStore::portable()?;
    acp_servers_in(&store, &agent_id)
}

pub(crate) fn acp_servers_in(store: &ClientStateStore, agent_id: &str) -> Result<Vec<Value>> {
    let _canonical = canonical_agent_id(agent_id)
        .filter(|value| value == agent_id)
        .ok_or_else(|| anyhow!("collaboration_mcp_registration_agent_invalid"))?;
    let _ = store;
    // A local installation never activates outbound access. ACP receives no
    // bridge descriptor until an authenticated exact-review broker exists.
    Ok(Vec::new())
}

#[cfg(test)]
pub(crate) fn load_bridge_registration(
    store: &ClientStateStore,
    agent_id: &str,
    registration_id: &str,
) -> Result<McpAgentRegistration> {
    ensure!(
        is_uuid(registration_id),
        "collaboration_mcp_registration_id_invalid"
    );
    let canonical = canonical_agent_id(agent_id)
        .filter(|value| value == agent_id)
        .ok_or_else(|| anyhow!("collaboration_mcp_registration_agent_invalid"))?;
    let installed = installed_workflow_plugin(store)?;
    let path = registration_root(store)
        .join(&canonical)
        .join(format!("{registration_id}.json"));
    let record = read_record(&path)?;
    ensure!(
        record.registration_id == registration_id && record.agent_id == canonical,
        "collaboration_mcp_registration_binding_invalid"
    );
    validate_runtime_binding(&record, &installed)?;
    let binding = loaded_authority_binding(store, &record)?;
    let (_, authority) = super::lifecycle::verified_authority(
        store,
        "Verify the protected authority before starting the exact MCP bridge registration",
    )?;
    authority
        .authority
        .ensure_registrations(std::slice::from_ref(&binding))?;
    Ok(record)
}

pub(in crate::domain::collaboration_plugin) fn registration_root(
    store: &ClientStateStore,
) -> PathBuf {
    collaboration_root(store).join(REGISTRATION_ROOT)
}

pub(in crate::domain::collaboration_plugin) fn canonical_agent_id(
    runtime_id: &str,
) -> Option<String> {
    let canonical = match runtime_id {
        "copilot" | "copilot-acp" => "copilot",
        "cursor" | "cursor-acp" | "cursor-cli" => "cursor",
        "hermes" | "hermes-acp" => "hermes",
        "kimi-code" | "kimi-code-acp" => "kimi-code",
        "openclaw" | "openclaw-acp" => "openclaw",
        _ => return None,
    };
    Some(canonical.to_owned())
}

#[cfg(test)]
fn read_record(path: &Path) -> Result<McpAgentRegistration> {
    let metadata = fs::symlink_metadata(path)
        .map_err(|_| anyhow!("collaboration_mcp_registration_missing"))?;
    ensure!(
        metadata.is_file()
            && !metadata.file_type().is_symlink()
            && usize::try_from(metadata.len()).is_ok_and(|size| size <= MAX_REGISTRATION_BYTES),
        "collaboration_mcp_registration_file_invalid"
    );
    let bytes = super::package::read_file_no_follow(path, MAX_REGISTRATION_BYTES)
        .map_err(|_| anyhow!("collaboration_mcp_registration_file_invalid"))?;
    ensure!(
        bytes.len() <= MAX_REGISTRATION_BYTES,
        "collaboration_mcp_registration_file_invalid"
    );
    let record: McpAgentRegistration = serde_json::from_slice(&bytes)
        .map_err(|_| anyhow!("collaboration_mcp_registration_file_invalid"))?;
    record.validate()?;
    Ok(record)
}

fn validate_runtime_binding(
    record: &McpAgentRegistration,
    installed: &InstalledWorkflowPlugin,
) -> Result<()> {
    ensure!(
        record.collaboration_plugin_id == installed.plugin_id
            && record.package_digest_sha256 == installed.digest_sha256,
        "collaboration_mcp_registration_package_changed"
    );
    for file in &record.payload_files {
        let path = PathBuf::from(
            record
                .payload_roots
                .iter()
                .find(|root| root.plugin_id == file.plugin_id)
                .ok_or_else(|| anyhow!("collaboration_mcp_registration_payload_invalid"))?
                .path
                .as_str(),
        )
        .join(
            Path::new(&file.relative_path)
                .strip_prefix(&file.plugin_id)
                .map_err(|_| anyhow!("collaboration_mcp_registration_payload_invalid"))?,
        );
        let metadata = fs::symlink_metadata(&path)
            .map_err(|_| anyhow!("collaboration_mcp_registration_payload_changed"))?;
        ensure!(
            metadata.is_file()
                && !metadata.file_type().is_symlink()
                && metadata.len() == file.bytes as u64,
            "collaboration_mcp_registration_payload_changed"
        );
        let bytes = fs::read(&path)?;
        ensure!(
            sha256_hex(&bytes) == file.digest_sha256,
            "collaboration_mcp_registration_payload_changed"
        );
    }
    Ok(())
}

fn digest_record(record: &McpAgentRegistration) -> Result<String> {
    Ok(sha256_hex(&serde_json::to_vec(record)?))
}

fn planned_record(registration: &PlannedAgentRegistration) -> Result<McpAgentRegistration> {
    ensure!(
        sha256_hex(registration.content.as_bytes()) == registration.digest_sha256,
        "collaboration_mcp_registration_digest_mismatch"
    );
    let record: McpAgentRegistration = serde_json::from_str(&registration.content)
        .map_err(|_| anyhow!("collaboration_mcp_registration_file_invalid"))?;
    record.validate()?;
    ensure!(
        record.registration_id == registration.registration_id
            && record.agent_id == registration.agent_id,
        "collaboration_mcp_registration_binding_invalid"
    );
    Ok(record)
}

fn authority_binding(
    destination: &AgentDestination,
    registration: &PlannedAgentRegistration,
    record: &McpAgentRegistration,
) -> Result<AuthorityRegistration> {
    authority_binding_for_record(
        &destination.install_destination,
        &registration.destination,
        &registration.digest_sha256,
        record,
    )
}

fn authority_binding_for_record(
    install_destination: &str,
    registration_destination: &str,
    registration_file_digest_sha256: &str,
    record: &McpAgentRegistration,
) -> Result<AuthorityRegistration> {
    ensure!(
        installation_destination(record)?.to_str() == Some(install_destination),
        "collaboration_mcp_registration_destination_changed"
    );
    Ok(AuthorityRegistration {
        registration_id: record.registration_id.clone(),
        agent_id: record.agent_id.clone(),
        package_digest_sha256: record.package_digest_sha256.clone(),
        registration_file_digest_sha256: registration_file_digest_sha256.to_owned(),
        registration_record_digest_sha256: record.registration_digest_sha256.clone(),
        selected_plugin_inventory_digest_sha256: scoped_digest(
            b"LICOUP-MCP-SELECTED-PLUGIN-INVENTORY-V1\0",
            record
                .selected_plugin_ids
                .iter()
                .map(|value| vec![value.clone()]),
        ),
        endpoint_scope_digest_sha256: scoped_digest(
            b"LICOUP-MCP-ENDPOINT-SCOPE-V1\0",
            record.servers.iter().map(|server| {
                vec![
                    server.plugin_id.clone(),
                    server.transport.clone(),
                    server.endpoint.clone(),
                ]
            }),
        ),
        payload_inventory_digest_sha256: scoped_digest(
            b"LICOUP-MCP-PAYLOAD-INVENTORY-V1\0",
            record.payload_files.iter().map(|file| {
                vec![
                    file.plugin_id.clone(),
                    file.relative_path.clone(),
                    file.digest_sha256.clone(),
                    file.bytes.to_string(),
                ]
            }),
        ),
        agent_destination_digest_sha256: scoped_digest(
            b"LICOUP-MCP-AGENT-DESTINATION-V1\0",
            [vec![
                record.agent_id.clone(),
                install_destination.to_owned(),
            ]],
        ),
        registration_destination_digest_sha256: scoped_digest(
            b"LICOUP-MCP-REGISTRATION-DESTINATION-V1\0",
            [vec![
                record.agent_id.clone(),
                registration_destination.to_owned(),
            ]],
        ),
    })
}

#[cfg(test)]
fn loaded_authority_binding(
    store: &ClientStateStore,
    record: &McpAgentRegistration,
) -> Result<AuthorityRegistration> {
    let registration_destination = registration_root(store)
        .join(&record.agent_id)
        .join(format!("{}.json", record.registration_id));
    let bytes =
        super::package::read_file_no_follow(&registration_destination, MAX_REGISTRATION_BYTES)
            .map_err(|_| anyhow!("collaboration_mcp_registration_file_invalid"))?;
    let install_destination = installation_destination(record)?;
    authority_binding_for_record(
        install_destination
            .to_str()
            .ok_or_else(|| anyhow!("collaboration_mcp_registration_path_invalid"))?,
        registration_destination
            .to_str()
            .ok_or_else(|| anyhow!("collaboration_mcp_registration_path_invalid"))?,
        &sha256_hex(&bytes),
        record,
    )
}

fn installation_destination(record: &McpAgentRegistration) -> Result<PathBuf> {
    let mut destination: Option<PathBuf> = None;
    for root in &record.payload_roots {
        let path = Path::new(&root.path);
        ensure!(
            path.file_name().and_then(|value| value.to_str()) == Some(root.plugin_id.as_str()),
            "collaboration_mcp_registration_payload_invalid"
        );
        let parent = path
            .parent()
            .ok_or_else(|| anyhow!("collaboration_mcp_registration_payload_invalid"))?
            .to_path_buf();
        ensure!(
            destination.as_ref().is_none_or(|value| value == &parent),
            "collaboration_mcp_registration_payload_invalid"
        );
        destination = Some(parent);
    }
    destination.ok_or_else(|| anyhow!("collaboration_mcp_registration_payload_invalid"))
}

fn scoped_digest(domain: &[u8], rows: impl IntoIterator<Item = Vec<String>>) -> String {
    let mut hasher = Sha256::new();
    hasher.update(domain);
    for row in rows {
        hasher.update((row.len() as u64).to_be_bytes());
        for value in row {
            hasher.update((value.len() as u64).to_be_bytes());
            hasher.update(value.as_bytes());
        }
    }
    format!("{:x}", hasher.finalize())
}

fn collect_payload_files(root: &Path) -> Result<BTreeMap<String, usize>> {
    fn visit(root: &Path, directory: &Path, files: &mut BTreeMap<String, usize>) -> Result<()> {
        let mut entries = fs::read_dir(directory)?.collect::<std::io::Result<Vec<_>>>()?;
        entries.sort_by_key(|entry| entry.file_name());
        for entry in entries {
            let path = entry.path();
            let metadata = fs::symlink_metadata(&path)?;
            ensure!(
                !metadata.file_type().is_symlink(),
                "collaboration_mcp_registration_payload_changed"
            );
            if metadata.is_dir() {
                visit(root, &path, files)?;
            } else {
                ensure!(
                    metadata.is_file(),
                    "collaboration_mcp_registration_payload_changed"
                );
                let relative = path
                    .strip_prefix(root)
                    .map_err(|_| anyhow!("collaboration_mcp_registration_payload_changed"))?
                    .components()
                    .map(|component| {
                        component
                            .as_os_str()
                            .to_str()
                            .map(str::to_owned)
                            .ok_or_else(|| {
                                anyhow!("collaboration_mcp_registration_payload_changed")
                            })
                    })
                    .collect::<Result<Vec<_>>>()?
                    .join("/");
                ensure!(
                    files
                        .insert(
                            relative,
                            usize::try_from(metadata.len()).map_err(|_| anyhow!(
                                "collaboration_mcp_registration_payload_changed"
                            ))?,
                        )
                        .is_none(),
                    "collaboration_mcp_registration_payload_changed"
                );
            }
        }
        Ok(())
    }

    let mut files = BTreeMap::new();
    visit(root, root, &mut files)?;
    Ok(files)
}

fn validate_endpoint(value: &str) -> Result<()> {
    use std::net::IpAddr;

    let endpoint = url::Url::parse(value)
        .map_err(|_| anyhow!("collaboration_mcp_registration_endpoint_invalid"))?;
    let host = endpoint
        .host_str()
        .ok_or_else(|| anyhow!("collaboration_mcp_registration_endpoint_invalid"))?;
    let loopback = host.eq_ignore_ascii_case("localhost")
        || host
            .parse::<IpAddr>()
            .is_ok_and(|address| address.is_loopback());
    ensure!(
        endpoint.as_str() == value
            && !endpoint.cannot_be_a_base()
            && endpoint.username().is_empty()
            && endpoint.password().is_none()
            && endpoint.fragment().is_none()
            && (endpoint.scheme() == "https" || endpoint.scheme() == "http" && loopback),
        "collaboration_mcp_registration_endpoint_invalid"
    );
    Ok(())
}

fn is_uuid(value: &str) -> bool {
    Uuid::parse_str(value).is_ok_and(|parsed| parsed.to_string() == value)
}

fn is_sha256(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
}

fn is_relative_safe_path(value: &str) -> bool {
    let path = Path::new(value);
    !value.is_empty()
        && !path.is_absolute()
        && path
            .components()
            .all(|component| matches!(component, std::path::Component::Normal(_)))
}

fn path_text(path: &Path) -> Result<String> {
    path.to_str()
        .map(str::to_owned)
        .ok_or_else(|| anyhow!("collaboration_mcp_registration_path_invalid"))
}

fn sha256_hex(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}
