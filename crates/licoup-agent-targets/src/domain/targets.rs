//! The Agent inventory: the target declarations and the discovery that decides
//! which of them exist on this machine and where they live.

mod binaries;
mod catalog;
mod discovery;
mod manual;
mod model_catalog;
mod parameters;
mod platform_paths;
mod probe_pool;
mod processes;
mod runtime_binding;
mod scan_merge;
pub mod scan_paths;
mod support;
mod target_cache;
mod virtual_machine_discovery;

use crate::port::AgentTargetPort;
use anyhow::Result;
use serde_json::Value;
use std::path::PathBuf;

pub use binaries::find_binary;
pub use catalog::{AdapterCapabilities, TargetCandidate, TargetDef};
pub use catalog::{normalize_target, target_def, target_defs};

pub fn scan_targets(port: &AgentTargetPort) -> Result<Value> {
    discovery::scan_targets(port)
}

pub fn scan_targets_with_params(port: &AgentTargetPort, params: &Value) -> Result<Value> {
    discovery::scan_targets_with_params(port, params)
}

pub fn available_runtime_executable(port: &AgentTargetPort, target: &str) -> Option<PathBuf> {
    runtime_binding::available_runtime_executable(port, target)
}

/// The local executable a user saved for one Agent, read from the portable
/// client-state store. This is the manual override the launcher validates
/// before any automatic route is considered.
pub fn manual_runtime_executable(
    target: &str,
) -> Result<Option<PathBuf>, runtime_binding::ManualRuntimeError> {
    runtime_binding::manual_runtime_executable(target)
}

/// The saved local executable read from a caller-supplied store, so the
/// launcher and the crate's tests read the same records the scan wrote.
pub fn manual_runtime_executable_from_store(
    store: &licoup_client_state::ClientStateStore,
    target: &str,
) -> Result<Option<PathBuf>, runtime_binding::ManualRuntimeError> {
    runtime_binding::manual_runtime_executable_from_store(store, target)
}

pub use runtime_binding::ManualRuntimeError;

/// CLI/runtime executable presence for the adapter management catalog: the
/// agent's official binary names on the automatic search dirs, or a verified
/// product-bundled executable (editor extension or desktop bundle).
///
/// Presence is discovery only: it runs no conversation probe, so it reads no
/// fact owned above this crate and takes no port.
pub fn agent_cli_executable(agent_id: &str) -> Option<PathBuf> {
    let def = catalog::target_def(agent_id).ok()?;
    binaries::find_target_binary(&def, &Value::Null)
        .or_else(|| binaries::find_extension_bundled_binary(&def))
}

/// Desktop application presence for the adapter management catalog. Only
/// agents with a verified desktop bundle mapping can report detection.
pub fn agent_desktop_app_detected(agent_id: &str) -> bool {
    binaries::desktop_app_executable(agent_id).is_some()
}

pub fn add_target(port: &AgentTargetPort, params: &Value) -> Result<Value> {
    manual::add_target(port, params)
}

pub fn inspect_target(port: &AgentTargetPort, target: &str) -> Result<Value> {
    discovery::inspect_target(port, target)
}

/// Inspect one target without executing its binary, reading its model or
/// history stores, or refreshing persisted discovery state.
pub fn inspect_target_read_only(port: &AgentTargetPort, target: &str) -> Result<Value> {
    discovery::inspect_target_read_only(port, target)
}

pub fn inspect_target_with_params(port: &AgentTargetPort, params: &Value) -> Result<Value> {
    discovery::inspect_target_with_params(port, params)
}

#[cfg(test)]
mod tests;
