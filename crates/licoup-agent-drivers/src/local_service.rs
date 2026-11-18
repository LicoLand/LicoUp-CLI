//! Target-neutral, bounded primitives for client-owned local agent services.
//!
//! HTTP/SSE transport and detached-service lifecycle live here. ACP JSONL is
//! intentionally owned by `core::acp` and must not be coupled to this module.

mod bounds;
mod concurrency;
mod endpoint;
pub mod executable;
pub mod http;
pub mod params;
pub mod port;
pub mod process;
pub mod serve;
pub mod sse;
pub mod state;
pub mod turn_control;

pub use endpoint::ServeEndpoint;
pub use endpoint::{ServeAttachment, ServeModel, ServeModelCatalog, ServeReadiness};
pub use serve::{ServeErrorCodes, ServeSpec};

/// The private state of one detached service, under the client's state root.
pub fn service_paths(state_dir: &str) -> anyhow::Result<state::ServicePaths> {
    state::ServicePaths::resolve(state_dir, "serve.pid")
}

/// The last state one detached service wrote.
pub fn read_service_state(paths: &state::ServicePaths) -> anyhow::Result<serde_json::Value> {
    state::read_json(&paths.state_path, "local_service_state_invalid")
}

/// The process id one detached service recorded, when it recorded one.
pub fn service_pid(paths: &state::ServicePaths) -> anyhow::Result<Option<u32>> {
    state::read_pid(&paths.pid_path)
}

/// Whether a recorded process id is still alive.
pub fn process_alive(pid: u32) -> bool {
    process::alive(Some(pid))
}

/// The in-flight turns one endpoint is running right now. The confirmation
/// dialog names these tasks; force stop never guesses them.
pub fn active_endpoint_turns(attach_url: &str) -> Vec<(String, String)> {
    turn_control::active_turns_for_endpoint(attach_url)
}

#[cfg(test)]
mod tests;
