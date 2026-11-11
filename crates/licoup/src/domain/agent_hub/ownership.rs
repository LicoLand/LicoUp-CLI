//! Local Agent Hub ownership records. Existing installs stay external.

use super::contract::{InstallOwnership, OWNERSHIP_EXTERNAL, OWNERSHIP_NONE, OWNERSHIP_OWNED};
use crate::platform::client_state::ClientStateStore;
use anyhow::{Result, ensure};
use licoup_foundation::platform::file_security::{
    atomic_write_private_text, ensure_private_dir, read_private_text_bounded,
    remove_private_state_marker,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::path::{Path, PathBuf};

const OWNERSHIP_FILE: &str = "ownership.toml";
const SUPERSEDED_OWNERSHIP_FILE: &str = "ownership.json";
const MAX_OWNERSHIP_BYTES: usize = 256 * 1024;

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
struct OwnershipDocument {
    #[serde(default)]
    items: Vec<InstallOwnership>,
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct JsonOwnershipDocument {
    #[serde(default)]
    items: Vec<JsonInstallOwnership>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct JsonInstallOwnership {
    agent_id: String,
    channel_id: String,
    channel_kind: String,
    package_coordinate: String,
    installed_version: String,
    ownership: String,
    lifecycle: String,
}

impl From<JsonInstallOwnership> for InstallOwnership {
    fn from(item: JsonInstallOwnership) -> Self {
        Self {
            agent_id: item.agent_id,
            channel_id: item.channel_id,
            channel_kind: item.channel_kind,
            package_coordinate: item.package_coordinate,
            installed_version: item.installed_version,
            ownership: item.ownership,
            lifecycle: item.lifecycle,
        }
    }
}

pub fn hub_dir(store: &ClientStateStore) -> PathBuf {
    store.root().join("agent-hub")
}

fn ownership_path(store: &ClientStateStore) -> PathBuf {
    hub_dir(store).join(OWNERSHIP_FILE)
}

fn superseded_ownership_path(store: &ClientStateStore) -> PathBuf {
    hub_dir(store).join(SUPERSEDED_OWNERSHIP_FILE)
}

pub fn load(store: &ClientStateStore) -> Result<Vec<InstallOwnership>> {
    let path = ownership_path(store);
    if path.is_file() {
        let items = read_toml(&path)?;
        let _ = remove_private_state_marker(&superseded_ownership_path(store));
        return Ok(items);
    }
    let json_path = superseded_ownership_path(store);
    if !json_path.is_file() {
        return Ok(Vec::new());
    }
    let items = read_json(&json_path)?;
    persist(store, items.clone())?;
    Ok(items)
}

pub fn get(store: &ClientStateStore, agent_id: &str) -> Result<Option<InstallOwnership>> {
    Ok(load(store)?
        .into_iter()
        .find(|item| item.agent_id == agent_id))
}

pub fn save(store: &ClientStateStore, record: InstallOwnership) -> Result<()> {
    ensure_private_dir(&hub_dir(store))?;
    let mut items = load(store)?;
    items.retain(|item| item.agent_id != record.agent_id);
    items.push(record);
    persist(store, items)
}

pub fn remove(store: &ClientStateStore, agent_id: &str) -> Result<()> {
    let mut items = load(store)?;
    items.retain(|item| item.agent_id != agent_id);
    persist(store, items)
}

fn persist(store: &ClientStateStore, items: Vec<InstallOwnership>) -> Result<()> {
    ensure_private_dir(&hub_dir(store))?;
    let document = OwnershipDocument { items };
    let raw = toml::to_string_pretty(&document)
        .map_err(|error| anyhow::anyhow!("agent hub ownership could not be encoded: {error}"))?;
    ensure!(
        raw.len() <= MAX_OWNERSHIP_BYTES,
        "agent hub ownership document exceeds its bounded size"
    );
    atomic_write_private_text(&ownership_path(store), &raw)?;
    let _ = remove_private_state_marker(&superseded_ownership_path(store));
    Ok(())
}

fn read_toml(path: &Path) -> Result<Vec<InstallOwnership>> {
    let Some(raw) = read_private_text_bounded(path, MAX_OWNERSHIP_BYTES)? else {
        return Ok(Vec::new());
    };
    let document: OwnershipDocument = toml::from_str(&raw).unwrap_or_default();
    Ok(document.items)
}

fn read_json(path: &Path) -> Result<Vec<InstallOwnership>> {
    let Some(raw) = read_private_text_bounded(path, MAX_OWNERSHIP_BYTES)? else {
        return Ok(Vec::new());
    };
    let document: JsonOwnershipDocument = serde_json::from_str(&raw).unwrap_or_default();
    Ok(document
        .items
        .into_iter()
        .map(InstallOwnership::from)
        .collect())
}

pub fn resolve_ownership(record: Option<&InstallOwnership>, present: bool) -> &'static str {
    if record.map(|item| item.ownership.as_str()) == Some(OWNERSHIP_OWNED) {
        OWNERSHIP_OWNED
    } else if present {
        OWNERSHIP_EXTERNAL
    } else {
        OWNERSHIP_NONE
    }
}

pub fn store_from_params(params: &Value) -> Result<ClientStateStore> {
    if let Some(root) = params
        .get("stateRoot")
        .or_else(|| params.get("clientStateRoot"))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        return ClientStateStore::new(PathBuf::from(root));
    }
    if let Some(portable_dir) = params
        .get("portableDir")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        return ClientStateStore::new(PathBuf::from(portable_dir).join("client-state"));
    }
    ClientStateStore::portable()
}
