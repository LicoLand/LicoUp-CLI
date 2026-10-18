use anyhow::{Result, anyhow};
use licoup_foundation::platform::file_security::ensure_private_dir;
use serde_json::{Value, json};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use super::super::local_service::{executable, params, port, process, state};
use super::command::{CommandGatewayRunner, GatewayRunner};
use super::health;
use super::model::{GatewayEndpoint, GatewayPaths, endpoint_from_state};
use super::policy;

pub(super) fn ensure(params_value: &Value) -> Result<Value> {
    operate(|paths| start_with_paths(params_value, paths, true, &CommandGatewayRunner))
}

pub(super) fn start(params_value: &Value) -> Result<Value> {
    operate(|paths| start_with_paths(params_value, paths, false, &CommandGatewayRunner))
}

pub(super) fn restart(params_value: &Value) -> Result<Value> {
    operate(|paths| {
        process::stop(&paths.service, policy::STOP_FAILED)?;
        start_with_paths(params_value, paths, false, &CommandGatewayRunner)
    })
}

pub(super) fn stop(_params: &Value) -> Result<Value> {
    operate(|paths| {
        let mut service_state = state::read_json(&paths.service.state_path, policy::INVALID_STATE)?;
        if service_state.get("attachMode").and_then(Value::as_str) == Some("vendor-default") {
            let _ = state::remove_pid(&paths.service.pid_path);
            service_state["status"] = json!("detached");
            service_state["running"] = json!(false);
            service_state["attachMode"] = json!("none");
            service_state["updatedAtUnix"] = json!(unix_seconds());
            state::write_json(&paths.service.state_path, &service_state)?;
            return Ok(json!({
                "ok": true,
                "status": "detached",
                "attachMode": "vendor-default",
                "stoppedOwnedProcess": false,
                "attachUrl": service_state.get("attachUrl").cloned().unwrap_or(json!("")),
                "wsUrl": service_state.get("wsUrl").cloned().unwrap_or(json!("")),
                "port": policy::VENDOR_DEFAULT_PORT
            }));
        }
        let stopped = process::stop(&paths.service, policy::STOP_FAILED)?;
        service_state = state::read_json(&paths.service.state_path, policy::INVALID_STATE)?;
        service_state["status"] = json!("stopped");
        service_state["running"] = json!(false);
        service_state["attachMode"] = json!("none");
        service_state["updatedAtUnix"] = json!(unix_seconds());
        state::write_json(&paths.service.state_path, &service_state)?;
        Ok(json!({
            "ok": true,
            "status": "stopped",
            "attachMode": "none",
            "attachUrl": service_state.get("attachUrl").cloned().unwrap_or(json!("")),
            "wsUrl": service_state.get("wsUrl").cloned().unwrap_or(json!("")),
            "port": service_state.get("port").cloned().unwrap_or(json!(policy::DEFAULT_PORT)),
            "stopped": stopped
        }))
    })
}

pub(super) fn status(_params: &Value) -> Result<Value> {
    operate(status_with_paths)
}

pub(super) fn ensure_attach_endpoint(executable_value: &str) -> Result<GatewayEndpoint> {
    let result = ensure(&json!({
        "executable": executable_value,
        "binary": executable_value,
        "host": policy::DEFAULT_HOST,
        "port": policy::DEFAULT_PORT
    }))?;
    if result.get("ok").and_then(Value::as_bool) != Some(true) {
        return Err(anyhow!(
            result
                .get("errorCode")
                .and_then(Value::as_str)
                .unwrap_or("openclaw_gateway_unavailable")
                .to_string()
        ));
    }
    let host = result
        .get("host")
        .and_then(Value::as_str)
        .unwrap_or(policy::DEFAULT_HOST)
        .to_string();
    let port = result
        .get("port")
        .and_then(Value::as_u64)
        .and_then(|value| u16::try_from(value).ok())
        .unwrap_or(policy::DEFAULT_PORT);
    let mut endpoint = GatewayEndpoint::new(host, port);
    if let Some(attach_url) = result.get("attachUrl").and_then(Value::as_str) {
        endpoint.attach_url = attach_url.to_string();
    }
    if let Some(ws_url) = result.get("wsUrl").and_then(Value::as_str) {
        endpoint.ws_url = ws_url.to_string();
    }
    Ok(endpoint)
}

pub(super) fn select_available_port(preferred: u16) -> Result<u16> {
    port::select(
        preferred,
        policy::PORT_RANGE_SPAN,
        policy::RESERVED_PORTS,
        policy::PORT_EXHAUSTED,
    )
}

pub(super) fn select_available_port_with<F>(preferred: u16, is_bindable: F) -> Result<u16>
where
    F: Fn(u16) -> bool,
{
    port::select_with(
        preferred,
        policy::PORT_RANGE_SPAN,
        policy::RESERVED_PORTS,
        policy::PORT_EXHAUSTED,
        is_bindable,
    )
}

pub(super) fn is_reserved_conflict_port(candidate: u16) -> bool {
    port::is_reserved(candidate, policy::RESERVED_PORTS)
}

fn operate<F>(operation: F) -> Result<Value>
where
    F: FnOnce(&GatewayPaths) -> Result<Value>,
{
    let paths = GatewayPaths::resolve()?;
    let _lock = state::OperationLock::acquire(&paths.service.lock_path)?;
    operation(&paths)
}

fn status_with_paths(paths: &GatewayPaths) -> Result<Value> {
    let mut service_state = state::read_json(&paths.service.state_path, policy::INVALID_STATE)?;
    let pid = state::read_pid(&paths.service.pid_path)?;
    let owned_running = process::alive(pid);
    let endpoint = endpoint_from_state(&service_state);
    let attach_mode_hint = service_state
        .get("attachMode")
        .and_then(Value::as_str)
        .unwrap_or("none")
        .to_owned();
    let health = if owned_running || attach_mode_hint == "vendor-default" {
        health::one_health_check(&endpoint.attach_url).ok()
    } else {
        None
    };
    let healthy = health::health_ready(health.as_ref());
    if !owned_running
        && attach_mode_hint != "vendor-default"
        && service_state
            .get("running")
            .and_then(Value::as_bool)
            .unwrap_or(false)
    {
        service_state["status"] = json!("stopped");
        service_state["running"] = json!(false);
        service_state["updatedAtUnix"] = json!(unix_seconds());
        state::write_json(&paths.service.state_path, &service_state)?;
    }
    let attach_mode = if attach_mode_hint == "vendor-default" && healthy {
        "vendor-default"
    } else if owned_running {
        "managed"
    } else {
        "none"
    };
    let running = if attach_mode == "vendor-default" {
        healthy
    } else {
        owned_running
    };
    let status = if running && healthy {
        "running"
    } else if owned_running {
        "blocked"
    } else if service_state
        .get("errorCode")
        .and_then(Value::as_str)
        .is_some()
    {
        "unavailable"
    } else {
        "stopped"
    };
    Ok(json!({
        "ok": true,
        "status": status,
        "running": running,
        "healthy": healthy,
        "attachMode": attach_mode,
        "pid": pid.unwrap_or(0),
        "attachUrl": endpoint.attach_url,
        "wsUrl": endpoint.ws_url,
        "host": service_state.get("host").cloned().unwrap_or_else(|| json!(policy::DEFAULT_HOST)),
        "port": service_state.get("port").cloned().unwrap_or_else(|| json!(policy::DEFAULT_PORT)),
        "preferredPort": service_state.get("preferredPort").cloned().unwrap_or_else(|| json!(policy::DEFAULT_PORT)),
        "vendorDefaultPort": policy::VENDOR_DEFAULT_PORT,
        "portConflict": service_state.get("portConflict").cloned().unwrap_or(json!(false)),
        "executableAvailable": executable::available(
            service_state
                .get("executable")
                .and_then(Value::as_str)
                .unwrap_or("openclaw")
        ),
        "errorCode": service_state.get("errorCode").cloned().unwrap_or(Value::Null),
        "health": health.unwrap_or_else(|| json!({"healthy": false}))
    }))
}

fn start_with_paths(
    params_value: &Value,
    paths: &GatewayPaths,
    reuse_healthy: bool,
    runner: &dyn GatewayRunner,
) -> Result<Value> {
    let preferred_raw =
        params::u16_value(params_value, &["port", "preferredPort"]).unwrap_or(policy::DEFAULT_PORT);
    let preferred = if preferred_raw == policy::VENDOR_DEFAULT_PORT {
        policy::DEFAULT_PORT
    } else {
        preferred_raw
    };
    let host = params::text(params_value, &["host", "hostname"])
        .unwrap_or_else(|| policy::DEFAULT_HOST.to_string());
    if !matches!(host.as_str(), "127.0.0.1" | "localhost" | "::1") {
        if let Some(attach) = params::text(params_value, &["attachUrl", "remoteUrl", "wsUrl"]) {
            return health::status_for_remote(&attach);
        }
        return Err(anyhow!("openclaw_gateway_remote_attach_required"));
    }
    let executable_value = executable::resolve(params_value, &["OPENCLAW_BIN"], "openclaw");

    if reuse_healthy {
        let existing = status_with_paths(paths)?;
        if existing.get("running").and_then(Value::as_bool) == Some(true)
            && existing.get("healthy").and_then(Value::as_bool) == Some(true)
        {
            return Ok(json!({
                "ok": true,
                "status": "running",
                "reused": true,
                "attachMode": existing.get("attachMode").cloned().unwrap_or_else(|| json!("managed")),
                "running": true,
                "healthy": true,
                "pid": existing.get("pid").cloned().unwrap_or(json!(0)),
                "host": existing.get("host").cloned().unwrap_or_else(|| json!(host)),
                "port": existing.get("port").cloned().unwrap_or_else(|| json!(preferred)),
                "attachUrl": existing.get("attachUrl").cloned().unwrap_or(json!("")),
                "wsUrl": existing.get("wsUrl").cloned().unwrap_or(json!("")),
                "portConflict": false,
                "preferredPort": preferred,
                "vendorDefaultPort": policy::VENDOR_DEFAULT_PORT,
                "executableAvailable": true
            }));
        }
        if let Some(vendor) = health::probe_vendor_default() {
            let _ = state::remove_pid(&paths.service.pid_path);
            state::write_json(&paths.service.state_path, &vendor.state)?;
            return Ok(vendor.response);
        }
        if existing.get("running").and_then(Value::as_bool) == Some(true)
            && existing.get("attachMode").and_then(Value::as_str) != Some("vendor-default")
        {
            process::stop(&paths.service, policy::STOP_FAILED)?;
        }
    }

    if !executable::available(&executable_value) {
        let endpoint = GatewayEndpoint::new(&host, preferred);
        return persist_failure(
            paths,
            &host,
            preferred,
            preferred,
            &endpoint,
            &executable_value,
            "unavailable",
            policy::EXECUTABLE_MISSING,
            false,
            false,
        );
    }

    let selected = match select_available_port(preferred) {
        Ok(port) => port,
        Err(_) => {
            let endpoint = GatewayEndpoint::new(&host, preferred);
            return persist_failure(
                paths,
                &host,
                preferred,
                preferred,
                &endpoint,
                &executable_value,
                "blocked",
                policy::PORT_EXHAUSTED,
                true,
                true,
            );
        }
    };
    let endpoint = GatewayEndpoint::new(&host, selected);
    let port_conflict = selected != preferred;
    let timeout = Duration::from_millis(
        params::u64_value(params_value, &["healthTimeoutMs"])
            .unwrap_or(policy::DEFAULT_HEALTH_TIMEOUT_MS),
    );
    ensure_private_dir(&paths.runtime_dir)?;
    let pid = match runner.spawn(
        &executable_value,
        selected,
        &paths.runtime_dir,
        &paths.config_path,
    ) {
        Ok(pid) => pid,
        Err(failure) => {
            let missing = failure == process::SpawnFailure::Missing;
            let error_code = if missing {
                policy::EXECUTABLE_MISSING
            } else {
                policy::START_FAILED
            };
            return persist_failure(
                paths,
                &host,
                preferred,
                selected,
                &endpoint,
                &executable_value,
                "unavailable",
                error_code,
                port_conflict,
                !missing,
            );
        }
    };
    if state::write_pid(&paths.service.pid_path, pid).is_err() {
        process::terminate_owned(pid);
        return Err(anyhow!(policy::START_FAILED));
    }

    match health::wait_for_health(&endpoint.attach_url, timeout) {
        Ok(health) => {
            let service_state = json!({
                "schemaVersion": policy::STATE_SCHEMA_VERSION,
                "status": "running",
                "running": true,
                "attachMode": "managed",
                "preferredPort": preferred,
                "vendorDefaultPort": policy::VENDOR_DEFAULT_PORT,
                "host": host,
                "port": selected,
                "attachUrl": endpoint.attach_url,
                "wsUrl": endpoint.ws_url,
                "portConflict": port_conflict,
                "pid": pid,
                "executable": executable_value,
                "updatedAtUnix": unix_seconds()
            });
            if state::write_json(&paths.service.state_path, &service_state).is_err() {
                let _ = process::stop(&paths.service, policy::STOP_FAILED);
                return Err(anyhow!(policy::START_FAILED));
            }
            Ok(json!({
                "ok": true,
                "status": "running",
                "reused": false,
                "attachMode": "managed",
                "running": true,
                "healthy": true,
                "pid": pid,
                "host": host,
                "port": selected,
                "preferredPort": preferred,
                "vendorDefaultPort": policy::VENDOR_DEFAULT_PORT,
                "attachUrl": endpoint.attach_url,
                "wsUrl": endpoint.ws_url,
                "portConflict": port_conflict,
                "executableAvailable": true,
                "health": health
            }))
        }
        Err(_) => {
            let _ = process::stop(&paths.service, policy::STOP_FAILED);
            persist_failure(
                paths,
                &host,
                preferred,
                selected,
                &endpoint,
                &executable_value,
                "blocked",
                policy::HEALTH_FAILED,
                port_conflict,
                true,
            )
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn persist_failure(
    paths: &GatewayPaths,
    host: &str,
    preferred: u16,
    selected: u16,
    endpoint: &GatewayEndpoint,
    executable_value: &str,
    status: &'static str,
    error_code: &'static str,
    port_conflict: bool,
    executable_available: bool,
) -> Result<Value> {
    let service_state = json!({
        "schemaVersion": policy::STATE_SCHEMA_VERSION,
        "status": status,
        "running": false,
        "errorCode": error_code,
        "preferredPort": preferred,
        "host": host,
        "port": selected,
        "attachUrl": endpoint.attach_url,
        "wsUrl": endpoint.ws_url,
        "portConflict": port_conflict,
        "executable": executable_value,
        "updatedAtUnix": unix_seconds()
    });
    state::write_json(&paths.service.state_path, &service_state)?;
    Ok(json!({
        "ok": false,
        "status": status,
        "running": false,
        "healthy": false,
        "errorCode": error_code,
        "host": host,
        "port": selected,
        "preferredPort": preferred,
        "attachUrl": endpoint.attach_url,
        "wsUrl": endpoint.ws_url,
        "portConflict": port_conflict,
        "executableAvailable": executable_available
    }))
}

pub(super) fn unix_seconds() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}
