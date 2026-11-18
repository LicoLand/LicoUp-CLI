use anyhow::{Result, anyhow};
use serde_json::{Value, json};
use std::thread;
use std::time::{Duration, Instant};

use super::super::local_service::{executable, http};
use super::model::GatewayEndpoint;
use super::policy;

pub(super) struct VendorAttach {
    pub(super) state: Value,
    pub(super) response: Value,
}

pub(super) fn status_for_remote(attach: &str) -> Result<Value> {
    let (http_url, ws_url) = normalize_remote(attach)?;
    let health = one_health_check(&http_url).ok();
    let healthy = health_ready(health.as_ref());
    Ok(json!({
        "ok": healthy,
        "status": if healthy { "running" } else { "unavailable" },
        "running": healthy,
        "healthy": healthy,
        "reused": true,
        "attachMode": "remote",
        "attachUrl": http_url,
        "wsUrl": ws_url,
        "errorCode": if healthy { Value::Null } else { json!(policy::HEALTH_FAILED) },
        "health": health.unwrap_or_else(|| json!({"healthy": false}))
    }))
}

pub(super) fn probe_vendor_default() -> Option<VendorAttach> {
    let endpoint = GatewayEndpoint::new(policy::DEFAULT_HOST, policy::VENDOR_DEFAULT_PORT);
    let health = one_health_check(&endpoint.attach_url).ok()?;
    if !health_ready(Some(&health)) {
        return None;
    }
    let state = json!({
        "schemaVersion": policy::STATE_SCHEMA_VERSION,
        "status": "running",
        "running": true,
        "attachMode": "vendor-default",
        "preferredPort": policy::DEFAULT_PORT,
        "vendorDefaultPort": policy::VENDOR_DEFAULT_PORT,
        "host": policy::DEFAULT_HOST,
        "port": policy::VENDOR_DEFAULT_PORT,
        "attachUrl": endpoint.attach_url,
        "wsUrl": endpoint.ws_url,
        "portConflict": false,
        "pid": 0,
        "updatedAtUnix": super::lifecycle::unix_seconds()
    });
    let response = json!({
        "ok": true,
        "status": "running",
        "reused": true,
        "attachMode": "vendor-default",
        "running": true,
        "healthy": true,
        "pid": 0,
        "host": policy::DEFAULT_HOST,
        "port": policy::VENDOR_DEFAULT_PORT,
        "preferredPort": policy::DEFAULT_PORT,
        "vendorDefaultPort": policy::VENDOR_DEFAULT_PORT,
        "attachUrl": endpoint.attach_url,
        "wsUrl": endpoint.ws_url,
        "portConflict": false,
        "executableAvailable": executable::which("openclaw"),
        "health": health
    });
    Some(VendorAttach { state, response })
}

pub(super) fn wait_for_health(attach_url: &str, timeout: Duration) -> Result<Value> {
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if let Ok(payload) =
            one_health_check_with_timeout(attach_url, remaining.min(Duration::from_secs(3)))
            && health_ready(Some(&payload))
        {
            return Ok(payload);
        }
        thread::sleep(Duration::from_millis(250).min(remaining));
    }
    Err(anyhow!(policy::HEALTH_FAILED))
}

pub(super) fn one_health_check(attach_url: &str) -> Result<Value> {
    one_health_check_with_timeout(attach_url, Duration::from_secs(3))
}

fn one_health_check_with_timeout(attach_url: &str, timeout: Duration) -> Result<Value> {
    let base = attach_url.trim_end_matches('/');
    for path in ["/v1/models", "/"] {
        let url = format!("{base}{path}");
        if let Ok(status) = http::probe_status(&url, timeout) {
            return Ok(json!({
                "healthy": true,
                "httpStatus": status,
                "probePath": path
            }));
        }
    }
    Err(anyhow!(policy::HEALTH_FAILED))
}

pub(super) fn health_ready(health: Option<&Value>) -> bool {
    health
        .and_then(|value| value.get("healthy"))
        .and_then(Value::as_bool)
        .unwrap_or(false)
}

pub(super) fn normalize_remote(attach: &str) -> Result<(String, String)> {
    let trimmed = attach.trim().trim_end_matches('/');
    let (http_url, ws_url) = if let Some(rest) = trimmed.strip_prefix("ws://") {
        (format!("http://{rest}"), trimmed.to_string())
    } else if let Some(rest) = trimmed.strip_prefix("wss://") {
        (format!("https://{rest}"), trimmed.to_string())
    } else if let Some(rest) = trimmed.strip_prefix("https://") {
        (trimmed.to_string(), format!("wss://{rest}"))
    } else if let Some(rest) = trimmed.strip_prefix("http://") {
        (trimmed.to_string(), format!("ws://{rest}"))
    } else {
        return Err(anyhow!("openclaw_gateway_remote_url_invalid"));
    };
    http::validate_url(&http_url).map_err(|_| anyhow!("openclaw_gateway_remote_url_invalid"))?;
    Ok((http_url, ws_url))
}
