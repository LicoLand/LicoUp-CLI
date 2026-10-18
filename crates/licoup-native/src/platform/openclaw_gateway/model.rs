use anyhow::Result;
use std::path::PathBuf;

use super::super::local_service::state::ServicePaths;
use super::policy;

// The paired HTTP/WebSocket endpoint model is OpenClaw's own protocol
// vocabulary, so it is owned by the OpenClaw adapter package and re-exported
// here at its former path. What stays in this module is the client's half: the
// service state paths it resolves and the projection of its own state document
// onto that model.
pub use licoup_agent_openclaw::gateway::GatewayEndpoint;

#[derive(Clone, Debug)]
pub(super) struct GatewayPaths {
    pub(super) service: ServicePaths,
    pub(super) runtime_dir: PathBuf,
    pub(super) config_path: PathBuf,
}

impl GatewayPaths {
    pub(super) fn resolve() -> Result<Self> {
        let service = ServicePaths::resolve(policy::STATE_DIR, "gateway.pid")?;
        Ok(Self {
            runtime_dir: service.root.join("runtime"),
            config_path: service.root.join("config.json"),
            service,
        })
    }
}

pub(super) fn endpoint_from_state(state: &serde_json::Value) -> GatewayEndpoint {
    let host = state
        .get("host")
        .and_then(serde_json::Value::as_str)
        .unwrap_or(policy::DEFAULT_HOST)
        .to_string();
    let port = state
        .get("port")
        .and_then(serde_json::Value::as_u64)
        .and_then(|value| u16::try_from(value).ok())
        .unwrap_or(policy::DEFAULT_PORT);
    let mut endpoint = GatewayEndpoint::new(host, port);
    if let Some(ws_url) = state
        .get("wsUrl")
        .and_then(serde_json::Value::as_str)
        .filter(|value| !value.is_empty())
    {
        endpoint.ws_url = ws_url.to_string();
    }
    if let Some(attach_url) = state
        .get("attachUrl")
        .and_then(serde_json::Value::as_str)
        .filter(|value| !value.is_empty())
    {
        endpoint.attach_url = attach_url.to_string();
    }
    endpoint
}
