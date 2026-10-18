use std::fs;
use std::path::{Component, Path, PathBuf};

use anyhow::{Result, anyhow, ensure};
use serde::Deserialize;
use serde_json::Value;

use super::super::super::registration::PlannedAgentRegistration;
use super::super::super::registration::{AgentDestination, canonical_agent_id};
use super::validation::required_text;

const MAX_AGENT_DESTINATIONS: usize = 32;
const MAX_DESTINATION_BYTES: usize = 4096;

#[derive(Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct AgentDestinationInput {
    agent_id: String,
    install_destination: String,
    confirmed: bool,
}

pub(super) fn agent_destinations(params: &Value) -> Result<Vec<AgentDestination>> {
    let raw = params
        .get("agentDestinations")
        .ok_or_else(|| anyhow!("collaboration_workflow_agent_destinations_required"))?;
    let value = match raw {
        Value::String(raw) => serde_json::from_str(raw)
            .map_err(|_| anyhow!("collaboration_workflow_agent_destinations_invalid"))?,
        value => value.clone(),
    };
    let inputs: Vec<AgentDestinationInput> = serde_json::from_value(value)
        .map_err(|_| anyhow!("collaboration_workflow_agent_destinations_invalid"))?;
    ensure!(
        !inputs.is_empty() && inputs.len() <= MAX_AGENT_DESTINATIONS,
        "collaboration_workflow_agent_destinations_required"
    );
    let mut destinations = Vec::with_capacity(inputs.len());
    for input in inputs {
        ensure!(
            input.confirmed,
            "collaboration_workflow_agent_destination_confirmation_required"
        );
        let adapter = crate::platform::runtime_adapters::adapter_for_agent_public(&input.agent_id)
            .ok_or_else(|| anyhow!("collaboration_workflow_agent_unknown"))?;
        ensure!(
            adapter.id() == input.agent_id,
            "collaboration_workflow_agent_id_must_be_canonical"
        );
        ensure!(
            canonical_agent_id(&input.agent_id).as_deref() == Some(input.agent_id.as_str()),
            "collaboration_workflow_agent_mcp_bridge_unsupported"
        );
        let install = parse_absolute_path(&input.install_destination)?;
        destinations.push(AgentDestination {
            agent_id: input.agent_id,
            install_destination: path_text(&install)?,
        });
    }
    destinations.sort_by(|left, right| left.agent_id.cmp(&right.agent_id));
    ensure!(
        destinations
            .windows(2)
            .all(|items| items[0].agent_id != items[1].agent_id),
        "collaboration_workflow_agent_duplicate"
    );
    Ok(destinations)
}

pub(super) fn validate_agent_destinations(destinations: &[AgentDestination]) -> Result<()> {
    let mut paths = Vec::with_capacity(destinations.len());
    for destination in destinations {
        let install = PathBuf::from(&destination.install_destination);
        validate_new_destination(&install)?;
        paths.push(install);
    }
    for (index, path) in paths.iter().enumerate() {
        ensure!(
            paths.iter().enumerate().all(|(other_index, other)| {
                index == other_index
                    || !(path == other || path.starts_with(other) || other.starts_with(path))
            }),
            "collaboration_workflow_destination_overlap"
        );
    }
    Ok(())
}

pub(super) fn validate_registration_destinations(
    agents: &[AgentDestination],
    registrations: &[PlannedAgentRegistration],
) -> Result<()> {
    ensure!(
        agents.len() == registrations.len(),
        "collaboration_mcp_registration_count_changed"
    );
    let mut paths = agents
        .iter()
        .map(|agent| PathBuf::from(&agent.install_destination))
        .collect::<Vec<_>>();
    for registration in registrations {
        ensure!(
            agents
                .iter()
                .any(|agent| agent.agent_id == registration.agent_id),
            "collaboration_mcp_registration_agent_invalid"
        );
        let path = PathBuf::from(&registration.destination);
        validate_new_destination(&path)?;
        paths.push(path);
    }
    for (index, path) in paths.iter().enumerate() {
        ensure!(
            paths.iter().enumerate().all(|(other_index, other)| {
                index == other_index
                    || !(path == other || path.starts_with(other) || other.starts_with(path))
            }),
            "collaboration_workflow_destination_overlap"
        );
    }
    Ok(())
}

pub(super) fn absolute_path_param(params: &Value, key: &str) -> Result<PathBuf> {
    parse_absolute_path(required_text(
        params,
        key,
        "collaboration_workflow_destination_required",
    )?)
}

pub(super) fn parse_absolute_path(value: &str) -> Result<PathBuf> {
    ensure!(
        value == value.trim() && !value.is_empty() && value.len() <= MAX_DESTINATION_BYTES,
        "collaboration_workflow_destination_invalid"
    );
    let path = PathBuf::from(value);
    ensure!(
        path.is_absolute()
            && path
                .components()
                .all(|component| !matches!(component, Component::CurDir | Component::ParentDir)),
        "collaboration_workflow_destination_must_be_absolute"
    );
    ensure!(
        path.to_str() == Some(value),
        "collaboration_workflow_destination_encoding_invalid"
    );
    Ok(path)
}

pub(super) fn validate_new_destination(path: &Path) -> Result<()> {
    let parent = path
        .parent()
        .ok_or_else(|| anyhow!("collaboration_workflow_destination_parent_missing"))?;
    #[cfg(unix)]
    drop(super::super::super::package::open_directory_path_no_follow(
        parent,
    )?);
    #[cfg(not(unix))]
    licoup_foundation::platform::file_security::validate_no_symlink_ancestors(path)?;
    licoup_foundation::platform::file_security::validate_export_destination(path)?;
    let parent_metadata = fs::symlink_metadata(parent)
        .map_err(|_| anyhow!("collaboration_workflow_destination_parent_missing"))?;
    ensure!(
        parent_metadata.is_dir() && !parent_metadata.file_type().is_symlink(),
        "collaboration_workflow_destination_parent_invalid"
    );
    ensure!(
        fs::symlink_metadata(path).is_err(),
        "collaboration_workflow_destination_must_be_new"
    );
    Ok(())
}

pub(super) fn relative_path_text(path: &Path) -> Result<String> {
    path.components()
        .map(|component| match component {
            Component::Normal(value) => value
                .to_str()
                .map(str::to_owned)
                .ok_or_else(|| anyhow!("collaboration_workflow_path_encoding_invalid")),
            _ => Err(anyhow!("collaboration_workflow_relative_path_invalid")),
        })
        .collect::<Result<Vec<_>>>()
        .map(|parts| parts.join("/"))
}

pub(super) fn path_text(path: &Path) -> Result<String> {
    path.to_str()
        .map(str::to_owned)
        .ok_or_else(|| anyhow!("collaboration_workflow_destination_encoding_invalid"))
}
