//! Disabled-by-default host contract for an optional, declarative LicoMesh
//! collaboration plugin.
//!
//! The host installs only bounded, non-executable package data from a directly
//! selected GitHub repository. It never loads plugin data during startup and
//! exposes workflow descriptors only through an explicit command. MCP payloads
//! may be installed and authority-bound locally, but ACP injection and the
//! outbound bridge remain disabled until an authenticated exact-review broker
//! is implemented by LicoUp.

mod assembly;
mod authority;
#[cfg(test)]
mod bridge;
mod lifecycle;
mod manifest;
mod package;
mod registration;
mod runner_signature;
mod source;
mod transaction;

#[cfg(test)]
mod test_support;
mod workflow;

pub use lifecycle::{
    cleanup, disable, enable, install_apply, install_cancel, install_plan, runner_trust_import,
    runner_trust_remove, status, uninstall, workflow_catalog,
};
pub use workflow::{
    cancel as workflow_cancel, local_deployment_apply, local_deployment_plan, mcp_install_apply,
    mcp_install_plan,
};

pub fn local_server_status(params: &serde_json::Value) -> anyhow::Result<serde_json::Value> {
    assembly::status(params)
}

pub fn local_server_start(params: &serde_json::Value) -> anyhow::Result<serde_json::Value> {
    assembly::start(params)
}

pub fn local_server_stop(params: &serde_json::Value) -> anyhow::Result<serde_json::Value> {
    assembly::stop(params)
}

pub fn local_server_uninstall(params: &serde_json::Value) -> anyhow::Result<serde_json::Value> {
    assembly::uninstall(params)
}

#[cfg(test)]
pub(crate) use bridge::serve_mcp_bridge;
pub(crate) use registration::acp_servers_for_runtime;
