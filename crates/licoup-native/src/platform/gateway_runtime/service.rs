//! Managed lifecycle facade for the unified Gateway Runtime.

use anyhow::Result;
use licoup_foundation::platform::file_security::ensure_private_dir;
use licoup_foundation::platform::paths;
use licoup_public_gateway::telegram;
use serde_json::{Value, json};
use std::path::PathBuf;

pub const REPORT_SCHEMA: &str = "licoup.gateway-runtime.v1";
pub const DEFAULT_PORT: u16 = 15_722;
const RUNTIME_STATE: &str = "gateway";

pub fn state_directory() -> Result<PathBuf> {
    let root = paths::portable_data_dir()?.join(RUNTIME_STATE);
    ensure_private_dir(&root)?;
    Ok(root)
}

/// Start the private gateway; the Telegram channel package process is started
/// alongside it when that package is installed and a bot token is configured —
/// the same pairing the unified runtime used to express with one process.
pub fn service_start(port: u16) -> Result<Value> {
    let _ = state_directory()?;
    let started = crate::platform::llm_gateway_service::service_start(port)?;
    let _ = crate::platform::telegram_channel_service::ensure_started();
    layered_status(port, started)
}

pub fn service_stop(port: u16) -> Result<Value> {
    let stopped = crate::platform::llm_gateway_service::service_stop(port)?;
    let _ = crate::platform::telegram_channel_service::service_stop();
    let _ = telegram::clear_ready();
    layered_status(port, stopped)
}

/// Stop an application-managed Gateway using its recorded port, without
/// touching an unrelated listener on a guessed port.
pub fn service_stop_managed() -> Result<Value> {
    let stopped = crate::platform::llm_gateway_service::service_stop_managed()?;
    let port = stopped
        .get("port")
        .and_then(Value::as_u64)
        .and_then(|value| u16::try_from(value).ok())
        .unwrap_or(DEFAULT_PORT);
    let _ = crate::platform::telegram_channel_service::service_stop();
    let _ = telegram::clear_ready();
    layered_status(port, stopped)
}

pub fn service_status(port: u16) -> Result<Value> {
    let base = crate::platform::llm_gateway_service::service_status(port)?;
    layered_status(port, base)
}

pub fn service_initialize(port: u16) -> Result<Value> {
    let _ = state_directory()?;
    let initialized = crate::platform::llm_gateway_service::service_initialize(port)?;
    layered_status(port, initialized)
}

/// Hot-reload verified conversation readiness into the running Gateway Runtime.
pub fn reload_conversation_inventory(readiness_json: &str) -> Result<Value> {
    let _ = state_directory()?;
    crate::platform::llm_gateway_service::reload_conversation_inventory(readiness_json)
}

fn layered_status(port: u16, base: Value) -> Result<Value> {
    let channels = telegram::channel_layer_status()?;
    let llm_state = base
        .get("state")
        .and_then(Value::as_str)
        .unwrap_or("stopped");
    Ok(json!({
        "ok": true,
        "schemaVersion": REPORT_SCHEMA,
        "state": base.get("state").cloned().unwrap_or(json!("stopped")),
        "managed": base.get("managed").cloned().unwrap_or(json!(false)),
        "pid": base.get("pid").cloned().unwrap_or(Value::Null),
        "processName": base.get("processName").cloned().unwrap_or(Value::Null),
        "port": port,
        "credentialsLoaded": base.get("credentialsLoaded").cloned().unwrap_or(json!(false)),
        "credentialsApplied": base.get("credentialsApplied").cloned().unwrap_or(json!(false)),
        "modelReady": base.get("modelReady").cloned().unwrap_or(json!(false)),
        "layers": {
            "llm": {
                "layer": "llm-gateway",
                "state": llm_state,
                "port": port,
                "configPath": base.get("configPath").cloned().unwrap_or(Value::Null),
                "logPath": base.get("logPath").cloned().unwrap_or(Value::Null),
            },
            "channels": channels,
        },
        "stateDirectory": state_directory()?,
        "message": base.get("message").cloned().unwrap_or(Value::Null),
    }))
}
