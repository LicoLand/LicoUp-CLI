use super::super::package::inspect_package;
use super::state::{installed_projection, plugins_root, read_state};
use anyhow::{Result, anyhow, ensure};
use serde_json::{Value, json};
use std::fs;
use std::path::Path;

use crate::platform::client_state::ClientStateStore;

pub(super) fn workflow_catalog_in(store: &ClientStateStore) -> Result<Value> {
    let state = read_state(store)?;
    ensure!(
        state.capability_enabled,
        "collaboration_plugin_capability_disabled"
    );
    let installed = state
        .installed
        .as_ref()
        .ok_or_else(|| anyhow!("collaboration_plugin_not_installed"))?;
    let package_root = plugins_root(store)?.join(&installed.plugin_id);
    let package = inspect_package(&package_root)?;
    ensure!(
        package.digest_sha256 == installed.digest_sha256,
        "collaboration_plugin_installed_digest_mismatch"
    );
    let local_deployment =
        read_descriptor(&package_root.join(&package.manifest.local_deployment_descriptor))?;
    let mcp_install =
        read_descriptor(&package_root.join(&package.manifest.mcp_install_descriptor))?;
    Ok(json!({
        "ok": true,
        "pluginLoaded": true,
        "loadPolicy": "explicit-command-only",
        "plugin": installed_projection(installed),
        "workflows": {
            "localDeployment": local_deployment,
            "mcpInstall": mcp_install
        },
        "externalTransferPolicy": "direct-exact-operation-approval-required"
    }))
}

fn read_descriptor(path: &Path) -> Result<Value> {
    let bytes = fs::read(path)
        .map_err(|_| anyhow!("collaboration_plugin_workflow_descriptor_unavailable"))?;
    ensure!(
        bytes.len() <= 1024 * 1024,
        "collaboration_plugin_workflow_descriptor_too_large"
    );
    serde_json::from_slice(&bytes)
        .map_err(|_| anyhow!("collaboration_plugin_workflow_descriptor_invalid"))
}
