use anyhow::{Result, anyhow};
use serde_json::{Value, json};
use std::process::Command;
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use super::bounds::PROCESS_POLL_INTERVAL;
use super::endpoint::ServeEndpoint;
use super::endpoint::{ServeAttachment, ServeReadiness};
use super::executable;
use super::http::{self, HttpFailure};
use super::params;
use super::port;
use super::process::{self, CommandServeRunner, ServeRunner, SpawnFailure};
use super::state::{self, OperationLock, ServicePaths};

#[derive(Clone, Copy)]
pub struct ServeErrorCodes {
    pub executable_missing: &'static str,
    pub port_exhausted: &'static str,
    pub start_failed: &'static str,
    pub health_failed: &'static str,
    pub attach_probe_failed: &'static str,
    pub not_found: &'static str,
    pub request_failed: &'static str,
    pub invalid_json: &'static str,
    pub invalid_state: &'static str,
    pub stop_failed: &'static str,
}

#[derive(Clone, Copy)]
pub struct ServeSpec {
    pub identity: &'static str,
    pub default_port: u16,
    pub port_range_span: u16,
    pub default_host: &'static str,
    pub health_path: &'static str,
    pub session_probe_path: &'static str,
    pub config_path: &'static str,
    pub provider_path: &'static str,
    pub state_dir: &'static str,
    pub state_schema_version: &'static str,
    pub default_health_timeout_ms: u64,
    pub reserved_ports: &'static [u16],
    pub executable_environment: &'static [&'static str],
    pub default_executable: &'static str,
    pub configure_command: fn(&mut Command, &str, u16),
    pub parse_readiness: fn(&Value, &Value, &Value, &Value) -> Option<ServeReadiness>,
    pub errors: ServeErrorCodes,
}

pub fn ensure(spec: ServeSpec, params: &Value) -> Result<Value> {
    operate(spec, |paths| {
        start_with_paths(spec, params, paths, true, &CommandServeRunner)
    })
}

pub fn start(spec: ServeSpec, params: &Value) -> Result<Value> {
    operate(spec, |paths| {
        start_with_paths(spec, params, paths, false, &CommandServeRunner)
    })
}

pub fn restart(spec: ServeSpec, params: &Value) -> Result<Value> {
    operate(spec, |paths| {
        let service_state = state::read_json(&paths.state_path, spec.errors.invalid_state)?;
        if service_state.get("owned").and_then(Value::as_bool) == Some(true) {
            process::stop(paths, spec.errors.stop_failed)?;
        }
        start_with_paths(spec, params, paths, false, &CommandServeRunner)
    })
}

pub fn stop(spec: ServeSpec) -> Result<Value> {
    operate(spec, |paths| {
        let mut service_state = state::read_json(&paths.state_path, spec.errors.invalid_state)?;
        let stopped = if service_state.get("owned").and_then(Value::as_bool) == Some(true) {
            process::stop(paths, spec.errors.stop_failed)?
        } else {
            json!({"ok": true, "status": "not-owned", "stopped": false})
        };
        service_state["status"] = json!("stopped");
        service_state["running"] = json!(false);
        service_state["updatedAtUnix"] = json!(unix_seconds());
        state::write_json(&paths.state_path, &service_state)?;
        Ok(json!({
            "ok": true,
            "status": "stopped",
            "attachUrl": service_state.get("attachUrl").cloned().unwrap_or(json!("")),
            "port": service_state.get("port").cloned().unwrap_or(json!(spec.default_port)),
            "stopped": stopped
        }))
    })
}

pub fn status(spec: ServeSpec) -> Result<Value> {
    operate(spec, |paths| status_with_paths(spec, paths))
}

pub fn ensure_attach_endpoint(spec: ServeSpec, executable: &str) -> Result<ServeEndpoint> {
    let result = ensure(
        spec,
        &json!({
            "executable": executable,
            "binary": executable,
            "host": spec.default_host,
            "port": spec.default_port
        }),
    )?;
    if result.get("ok").and_then(Value::as_bool) != Some(true)
        || result.get("healthy").and_then(Value::as_bool) != Some(true)
    {
        return Err(anyhow!(
            result
                .get("errorCode")
                .and_then(Value::as_str)
                .unwrap_or(spec.errors.health_failed)
                .to_string()
        ));
    }
    let host = result
        .get("host")
        .and_then(Value::as_str)
        .unwrap_or(spec.default_host)
        .to_string();
    let port = result
        .get("port")
        .and_then(Value::as_u64)
        .and_then(|value| u16::try_from(value).ok())
        .unwrap_or(spec.default_port);
    let endpoint = ServeEndpoint::new(host, port);
    verify_attach_ready(spec, &endpoint.attach_url)?;
    Ok(endpoint)
}

pub fn ensure_attachment(spec: ServeSpec, executable: &str) -> Result<ServeAttachment> {
    operate(spec, |paths| {
        let result = start_with_paths(
            spec,
            &json!({
                "executable": executable,
                "binary": executable,
                "host": spec.default_host,
                "port": spec.default_port
            }),
            paths,
            true,
            &CommandServeRunner,
        )?;
        if result.get("ok").and_then(Value::as_bool) != Some(true)
            || result.get("healthy").and_then(Value::as_bool) != Some(true)
        {
            return Err(anyhow!(
                result
                    .get("errorCode")
                    .and_then(Value::as_str)
                    .unwrap_or(spec.errors.health_failed)
                    .to_string()
            ));
        }
        let endpoint = ServeEndpoint::new(
            result
                .get("host")
                .and_then(Value::as_str)
                .unwrap_or(spec.default_host),
            result
                .get("port")
                .and_then(Value::as_u64)
                .and_then(|value| u16::try_from(value).ok())
                .unwrap_or(spec.default_port),
        );
        let readiness = probe_ready(spec, &endpoint.attach_url, Duration::from_secs(5))?;
        let lease = super::turn_control::pin_endpoint(&endpoint.attach_url)
            .map_err(|_| anyhow!(spec.errors.attach_probe_failed))?;
        Ok(ServeAttachment {
            endpoint,
            catalog: readiness.catalog,
            _lease: lease,
        })
    })
}

pub fn select_available_port(spec: ServeSpec, preferred: u16) -> Result<u16> {
    port::select(
        preferred,
        spec.port_range_span,
        spec.reserved_ports,
        spec.errors.port_exhausted,
    )
}

pub fn select_available_port_with<F>(spec: ServeSpec, preferred: u16, is_bindable: F) -> Result<u16>
where
    F: Fn(u16) -> bool,
{
    port::select_with(
        preferred,
        spec.port_range_span,
        spec.reserved_ports,
        spec.errors.port_exhausted,
        is_bindable,
    )
}

pub fn is_reserved_port(spec: ServeSpec, candidate: u16) -> bool {
    port::is_reserved(candidate, spec.reserved_ports)
}

pub fn get_json(spec: ServeSpec, url: &str, source: Option<&str>) -> Result<Value> {
    match source {
        Some(source) => http::get_json_observed(url, Duration::from_secs(5), source),
        None => http::get_json(url, Duration::from_secs(5)),
    }
    .map_err(|failure| http_error(spec, failure))
}

pub fn post_json(spec: ServeSpec, url: &str, body: &Value, source: &str) -> Result<Value> {
    http::post_json_observed(url, body, Some(Duration::from_secs(120)), source)
        .map_err(|failure| http_error(spec, failure))
}

fn operate<T, F>(spec: ServeSpec, operation: F) -> Result<T>
where
    F: FnOnce(&ServicePaths) -> Result<T>,
{
    let paths = ServicePaths::resolve(spec.state_dir, "serve.pid")?;
    let _lock = OperationLock::acquire(&paths.lock_path)?;
    operation(&paths)
}

fn status_with_paths(spec: ServeSpec, paths: &ServicePaths) -> Result<Value> {
    let mut service_state = state::read_json(&paths.state_path, spec.errors.invalid_state)?;
    let pid = state::read_pid(&paths.pid_path)?;
    let pid_alive = process::alive(pid);
    let attach_url = service_state
        .get("attachUrl")
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
        .unwrap_or_else(|| ServeEndpoint::new(spec.default_host, spec.default_port).attach_url);
    let owned = service_state.get("owned").and_then(Value::as_bool) == Some(true);
    let health = owned
        .then(|| one_health_check(spec, &attach_url))
        .transpose()
        .ok()
        .flatten();
    let healthy = owned && pid_alive && health_is_ready(health.as_ref());
    let adopted = false;
    let running = owned && pid_alive;
    if !running
        && service_state
            .get("running")
            .and_then(Value::as_bool)
            .unwrap_or(false)
    {
        service_state["status"] = json!("stopped");
        service_state["running"] = json!(false);
        service_state["updatedAtUnix"] = json!(unix_seconds());
        state::write_json(&paths.state_path, &service_state)?;
    }
    let status = if running && healthy {
        "running"
    } else if pid_alive {
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
        "adopted": adopted,
        "pid": pid.unwrap_or(0),
        "attachUrl": attach_url,
        "host": service_state.get("host").cloned().unwrap_or_else(|| json!(spec.default_host)),
        "port": service_state.get("port").cloned().unwrap_or_else(|| json!(spec.default_port)),
        "preferredPort": service_state.get("preferredPort").cloned().unwrap_or_else(|| json!(spec.default_port)),
        "portConflict": service_state.get("portConflict").cloned().unwrap_or(json!(false)),
        "executableAvailable": executable::available(spec.default_executable),
        "errorCode": service_state.get("errorCode").cloned().unwrap_or(Value::Null),
        "health": health.unwrap_or_else(|| json!({"healthy": false}))
    }))
}

fn start_with_paths(
    spec: ServeSpec,
    params_value: &Value,
    paths: &ServicePaths,
    reuse_healthy: bool,
    runner: &dyn ServeRunner,
) -> Result<Value> {
    let preferred =
        params::u16_value(params_value, &["port", "preferredPort"]).unwrap_or(spec.default_port);
    let host = params::text(params_value, &["host", "hostname"])
        .unwrap_or_else(|| spec.default_host.to_string());
    if !matches!(host.as_str(), "127.0.0.1" | "localhost" | "::1") {
        if let Some(attach_url) = params::text(params_value, &["attachUrl", "remoteUrl"]) {
            return status_for_remote(spec, &attach_url);
        }
        return Err(anyhow!(format!("{}_remote_attach_required", spec.identity)));
    }
    let executable = executable::resolve(
        params_value,
        spec.executable_environment,
        spec.default_executable,
    );
    let resolved_executable = executable::resolve_file(&executable);
    let preferred_endpoint = ServeEndpoint::new(&host, preferred);
    let existing_state = state::read_json(&paths.state_path, spec.errors.invalid_state)?;
    let mut draining =
        cleanup_draining_generations(existing_state.get("draining").and_then(Value::as_array));
    let mut preserve_current = false;

    if reuse_healthy {
        let owned = existing_state.get("owned").and_then(Value::as_bool) == Some(true);
        let current_pid = existing_state
            .get("pid")
            .and_then(Value::as_u64)
            .and_then(|value| u32::try_from(value).ok())
            .filter(|pid| process::alive(Some(*pid)));
        let attach_url = existing_state
            .get("attachUrl")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        let readiness = (!attach_url.is_empty())
            .then(|| probe_ready(spec, &attach_url, Duration::from_secs(5)))
            .transpose()
            .ok()
            .flatten();
        if owned
            && current_pid.is_some()
            && let (Some(selected), Some(readiness)) =
                (resolved_executable.as_ref(), readiness.as_ref())
            && launch_identity_matches(&existing_state, selected, &readiness.version)
        {
            let endpoint = ServeEndpoint::new(
                existing_state
                    .get("host")
                    .and_then(Value::as_str)
                    .unwrap_or(&host),
                existing_state
                    .get("port")
                    .and_then(Value::as_u64)
                    .and_then(|value| u16::try_from(value).ok())
                    .unwrap_or(preferred),
            );
            return persist_reused_endpoint(
                spec,
                paths,
                &endpoint,
                preferred,
                readiness.health.clone(),
                &existing_state,
                draining,
            );
        }
        if owned && current_pid.is_some() {
            let active = !attach_url.is_empty()
                && super::turn_control::endpoint_has_active_turn(&attach_url);
            if active {
                draining.push(generation_from_state(&existing_state));
                preserve_current = true;
            } else if let Some(pid) = current_pid {
                process::terminate_owned(pid);
                let _ = state::remove_pid(&paths.pid_path);
            }
        }
    }

    let Some(resolved_executable) = resolved_executable else {
        if preserve_current {
            return Ok(transient_failure(
                spec,
                &host,
                preferred,
                &preferred_endpoint,
                spec.errors.executable_missing,
                false,
                false,
            ));
        }
        return persist_failure(
            spec,
            paths,
            &host,
            preferred,
            preferred,
            &preferred_endpoint,
            &executable,
            "unavailable",
            spec.errors.executable_missing,
            false,
            false,
        );
    };

    let selected = match select_available_port(spec, preferred) {
        Ok(port) => port,
        Err(_) => {
            if preserve_current {
                return Ok(transient_failure(
                    spec,
                    &host,
                    preferred,
                    &preferred_endpoint,
                    spec.errors.port_exhausted,
                    true,
                    true,
                ));
            }
            return persist_failure(
                spec,
                paths,
                &host,
                preferred,
                preferred,
                &preferred_endpoint,
                &executable,
                "blocked",
                spec.errors.port_exhausted,
                true,
                true,
            );
        }
    };
    let endpoint = ServeEndpoint::new(&host, selected);
    let port_conflict = selected != preferred;
    let timeout = Duration::from_millis(
        params::u64_value(params_value, &["healthTimeoutMs"])
            .unwrap_or(spec.default_health_timeout_ms),
    );
    let pid = match runner.spawn(
        &resolved_executable.path,
        &host,
        selected,
        spec.configure_command,
    ) {
        Ok(pid) => pid,
        Err(failure) => {
            let missing = failure == SpawnFailure::Missing;
            let code = if missing {
                spec.errors.executable_missing
            } else {
                spec.errors.start_failed
            };
            if preserve_current {
                return Ok(transient_failure(
                    spec,
                    &host,
                    preferred,
                    &endpoint,
                    code,
                    port_conflict,
                    !missing,
                ));
            }
            return persist_failure(
                spec,
                paths,
                &host,
                preferred,
                selected,
                &endpoint,
                &executable,
                "unavailable",
                code,
                port_conflict,
                !missing,
            );
        }
    };
    match wait_for_ready(spec, &endpoint.attach_url, timeout) {
        Ok(readiness) => {
            let service_state = json!({
                "schemaVersion": spec.state_schema_version,
                "status": "running",
                "running": true,
                "preferredPort": preferred,
                "host": host,
                "port": selected,
                "attachUrl": endpoint.attach_url,
                "portConflict": port_conflict,
                "pid": pid,
                "owned": true,
                "launchIdentity": {
                    "file": resolved_executable.private_file_identity,
                    "version": readiness.version,
                },
                "draining": draining,
                "updatedAtUnix": unix_seconds()
            });
            if state::write_json(&paths.state_path, &service_state).is_err()
                || state::write_pid(&paths.pid_path, pid).is_err()
            {
                process::terminate_owned(pid);
                return Err(anyhow!(spec.errors.start_failed));
            }
            Ok(json!({
                "ok": true,
                "status": "running",
                "reused": false,
                "running": true,
                "healthy": true,
                "pid": pid,
                "host": host,
                "port": selected,
                "preferredPort": preferred,
                "attachUrl": endpoint.attach_url,
                "portConflict": port_conflict,
                "executableAvailable": true,
                "health": readiness.health
            }))
        }
        Err(_) => {
            process::terminate_owned(pid);
            if preserve_current {
                return Ok(transient_failure(
                    spec,
                    &host,
                    preferred,
                    &endpoint,
                    spec.errors.health_failed,
                    port_conflict,
                    true,
                ));
            }
            persist_failure(
                spec,
                paths,
                &host,
                preferred,
                selected,
                &endpoint,
                &executable,
                "blocked",
                spec.errors.health_failed,
                port_conflict,
                true,
            )
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn persist_failure(
    spec: ServeSpec,
    paths: &ServicePaths,
    host: &str,
    preferred: u16,
    selected: u16,
    endpoint: &ServeEndpoint,
    executable: &str,
    status: &'static str,
    error_code: &'static str,
    port_conflict: bool,
    executable_available: bool,
) -> Result<Value> {
    let service_state = json!({
        "schemaVersion": spec.state_schema_version,
        "status": status,
        "running": false,
        "errorCode": error_code,
        "preferredPort": preferred,
        "host": host,
        "port": selected,
        "attachUrl": endpoint.attach_url,
        "portConflict": port_conflict,
        "executable": executable,
        "updatedAtUnix": unix_seconds()
    });
    state::write_json(&paths.state_path, &service_state)?;
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
        "portConflict": port_conflict,
        "executableAvailable": executable_available
    }))
}

fn persist_reused_endpoint(
    spec: ServeSpec,
    paths: &ServicePaths,
    endpoint: &ServeEndpoint,
    preferred: u16,
    health: Value,
    existing_state: &Value,
    draining: Vec<Value>,
) -> Result<Value> {
    let port_conflict = endpoint.port != preferred;
    let mut service_state = existing_state.clone();
    service_state["schemaVersion"] = json!(spec.state_schema_version);
    service_state["status"] = json!("running");
    service_state["running"] = json!(true);
    service_state["preferredPort"] = json!(preferred);
    service_state["portConflict"] = json!(port_conflict);
    service_state["draining"] = json!(draining);
    service_state["updatedAtUnix"] = json!(unix_seconds());
    state::write_json(&paths.state_path, &service_state)?;
    Ok(json!({
        "ok": true,
        "status": "running",
        "reused": true,
        "adopted": false,
        "running": true,
        "healthy": true,
        "pid": state::read_pid(&paths.pid_path)?.unwrap_or(0),
        "host": endpoint.host,
        "port": endpoint.port,
        "preferredPort": preferred,
        "attachUrl": endpoint.attach_url,
        "portConflict": port_conflict,
        "executableAvailable": true,
        "health": health
    }))
}

fn launch_identity_matches(
    state: &Value,
    executable: &executable::ResolvedExecutable,
    health_version: &str,
) -> bool {
    state
        .pointer("/launchIdentity/file")
        .and_then(Value::as_str)
        == Some(executable.private_file_identity.as_str())
        && state
            .pointer("/launchIdentity/version")
            .and_then(Value::as_str)
            == Some(health_version)
}

#[cfg(test)]
pub fn launch_identity_matches_for_test(
    state: &Value,
    executable: &executable::ResolvedExecutable,
    health_version: &str,
) -> bool {
    launch_identity_matches(state, executable, health_version)
}

#[cfg(test)]
pub fn start_with_paths_for_test(
    spec: ServeSpec,
    params_value: &Value,
    paths: &ServicePaths,
    reuse_healthy: bool,
    runner: &dyn ServeRunner,
) -> Result<Value> {
    start_with_paths(spec, params_value, paths, reuse_healthy, runner)
}

#[cfg(test)]
pub fn probe_ready_for_test(
    spec: ServeSpec,
    attach_url: &str,
    timeout: Duration,
) -> Result<ServeReadiness> {
    probe_ready(spec, attach_url, timeout)
}

fn generation_from_state(state: &Value) -> Value {
    json!({
        "pid": state.get("pid").cloned().unwrap_or(Value::Null),
        "host": state.get("host").cloned().unwrap_or(Value::Null),
        "port": state.get("port").cloned().unwrap_or(Value::Null),
        "attachUrl": state.get("attachUrl").cloned().unwrap_or(Value::Null),
        "launchIdentity": state.get("launchIdentity").cloned().unwrap_or(Value::Null),
    })
}

fn cleanup_draining_generations(existing: Option<&Vec<Value>>) -> Vec<Value> {
    let mut retained = Vec::new();
    for generation in existing.into_iter().flatten() {
        let pid = generation
            .get("pid")
            .and_then(Value::as_u64)
            .and_then(|value| u32::try_from(value).ok());
        let attach_url = generation
            .get("attachUrl")
            .and_then(Value::as_str)
            .unwrap_or_default();
        if !process::alive(pid) {
            continue;
        }
        if !attach_url.is_empty() && super::turn_control::endpoint_has_active_turn(attach_url) {
            retained.push(generation.clone());
        } else if let Some(pid) = pid {
            process::terminate_owned(pid);
        }
    }
    retained
}

fn transient_failure(
    _spec: ServeSpec,
    host: &str,
    selected: u16,
    endpoint: &ServeEndpoint,
    error_code: &'static str,
    port_conflict: bool,
    executable_available: bool,
) -> Value {
    json!({
        "ok": false,
        "status": "blocked",
        "running": false,
        "healthy": false,
        "errorCode": error_code,
        "host": host,
        "port": selected,
        "preferredPort": selected,
        "attachUrl": endpoint.attach_url,
        "portConflict": port_conflict,
        "executableAvailable": executable_available,
    })
}

fn status_for_remote(spec: ServeSpec, attach_url: &str) -> Result<Value> {
    let trimmed = attach_url.trim().trim_end_matches('/');
    let readiness = probe_ready(spec, trimmed, Duration::from_secs(5)).ok();
    let healthy = readiness.is_some();
    Ok(json!({
        "ok": healthy,
        "status": if healthy { "running" } else { "blocked" },
        "running": healthy,
        "healthy": healthy,
        "attachUrl": trimmed,
        "remote": true,
        "health": readiness.map(|value| value.health).unwrap_or_else(|| json!({"healthy": false})),
        "errorCode": if healthy { Value::Null } else { json!(spec.errors.health_failed) }
    }))
}

fn wait_for_ready(spec: ServeSpec, attach_url: &str, timeout: Duration) -> Result<ServeReadiness> {
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if let Ok(health) = probe_ready(spec, attach_url, remaining.min(Duration::from_secs(5))) {
            return Ok(health);
        }
        thread::sleep(PROCESS_POLL_INTERVAL.min(remaining));
    }
    Err(anyhow!(spec.errors.health_failed))
}

fn probe_ready(spec: ServeSpec, attach_url: &str, timeout: Duration) -> Result<ServeReadiness> {
    let deadline = Instant::now() + timeout;
    let health = one_health_check_with_timeout(spec, attach_url, timeout)?;
    let remaining = deadline.saturating_duration_since(Instant::now());
    if remaining.is_zero() {
        return Err(anyhow!(spec.errors.attach_probe_failed));
    }
    let sessions_url = format!(
        "{}{}",
        attach_url.trim_end_matches('/'),
        spec.session_probe_path
    );
    let sessions = http::get_json(&sessions_url, remaining)
        .map_err(|_| anyhow!(spec.errors.attach_probe_failed))?;
    let remaining = deadline.saturating_duration_since(Instant::now());
    if remaining.is_zero() {
        return Err(anyhow!(spec.errors.attach_probe_failed));
    }
    let config = http::get_json(
        &format!("{}{}", attach_url.trim_end_matches('/'), spec.config_path),
        remaining,
    )
    .map_err(|_| anyhow!(spec.errors.attach_probe_failed))?;
    let remaining = deadline.saturating_duration_since(Instant::now());
    if remaining.is_zero() {
        return Err(anyhow!(spec.errors.attach_probe_failed));
    }
    let providers = http::get_json(
        &format!("{}{}", attach_url.trim_end_matches('/'), spec.provider_path),
        remaining,
    )
    .map_err(|_| anyhow!(spec.errors.attach_probe_failed))?;
    (spec.parse_readiness)(&health, &sessions, &config, &providers)
        .ok_or_else(|| anyhow!(spec.errors.attach_probe_failed))
}

fn one_health_check(spec: ServeSpec, attach_url: &str) -> Result<Value> {
    one_health_check_with_timeout(spec, attach_url, Duration::from_secs(5))
}

fn one_health_check_with_timeout(
    spec: ServeSpec,
    attach_url: &str,
    timeout: Duration,
) -> Result<Value> {
    let url = format!("{}{}", attach_url.trim_end_matches('/'), spec.health_path);
    let payload = http::get_json(&url, timeout).map_err(|failure| http_error(spec, failure))?;
    Ok(payload)
}

fn health_is_ready(payload: Option<&Value>) -> bool {
    payload
        .and_then(|value| value.get("healthy"))
        .and_then(Value::as_bool)
        .unwrap_or(false)
}

fn verify_attach_ready(spec: ServeSpec, attach_url: &str) -> Result<()> {
    probe_ready(spec, attach_url, Duration::from_secs(5)).map(|_| ())
}

fn http_error(spec: ServeSpec, failure: HttpFailure) -> anyhow::Error {
    let code = match failure {
        HttpFailure::NotFound => spec.errors.not_found,
        HttpFailure::InvalidJson => spec.errors.invalid_json,
        _ => spec.errors.request_failed,
    };
    anyhow!(code)
}

fn unix_seconds() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}
