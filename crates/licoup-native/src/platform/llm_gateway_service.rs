//! Managed lifecycle for the loopback LLM gateway sidecar process.
//!
//! The CLI owns a private state directory under the portable data root with
//! the generated provider/routing configuration, the pid record, the private
//! credentials control socket, and the append-only sidecar log. Secrets are
//! never written here. The sidecar starts disconnected and healthy. Explicit
//! authorization loads credentials into the long-lived native process and
//! hot-applies them to a running managed sidecar over the private control
//! socket; spawn-time fd handoff remains the cold-start path.

#[cfg(unix)]
use crate::core::secure_mesh_secret_store::SecretBytes;
#[cfg(unix)]
use crate::core::secure_mesh_secret_store::SecretStoreAuthorizationSession;
use anyhow::{Result, anyhow, ensure};
use licoup_foundation::platform::file_security::{
    atomic_write_private_text, ensure_private_dir, read_private_text_bounded,
    remove_private_state_marker,
};
use licoup_foundation::platform::paths;
use licoup_private_gateway::model::llm_gateway::{
    CompiledGateway, CredentialStyle, GatewayConfig, GatewayProvider, MAX_GATEWAY_BODY_BYTES,
    UpstreamProtocol,
};
use licoup_vault::api_key::LlmApiKeyProvider;
#[cfg(unix)]
use licoup_vault::api_key::{GatewayCredentialHandoff, LlmApiKeyInventory};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::BTreeSet;
use std::fs::OpenOptions;
use std::io::{Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
#[cfg(unix)]
use std::sync::{Mutex, OnceLock};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

pub const DEFAULT_PORT: u16 = 15_722;
pub const REPORT_SCHEMA: &str = "licoup.llm-gateway-service.v1";

const STATE_DIRECTORY: &str = "llm-gateway";
/// Unified Gateway Runtime binary (LLM lower layer + Communication Channels).
const SIDECAR_BINARY: &str = "lico-gateway";
const LEGACY_SIDECAR_BINARY: &str = "lico-llm-gateway";
const MAX_CONFIG_BYTES: usize = 64 * 1024;
const MAX_PID_BYTES: usize = 1024;
const HEALTH_PROBE_BYTES: usize = 4 * 1024;
const HEALTH_PROBE_TIMEOUT: Duration = Duration::from_millis(300);
const START_TIMEOUT: Duration = Duration::from_secs(10);
const POLL_INTERVAL: Duration = Duration::from_millis(100);

/// Secret material unlocked by the owner once for the lifetime of the native
/// app process. The cache is never serialized to disk or projected through a
/// command result. A restarted sidecar receives a fresh inherited-pipe copy.
#[cfg(unix)]
static GATEWAY_SESSION_CREDENTIALS: OnceLock<Mutex<Option<SecretBytes>>> = OnceLock::new();

/// Credential IDs currently bound into the in-memory Gateway handoff. Selection
/// is process-local and never persisted.
#[cfg(unix)]
static GATEWAY_ENABLED_CREDENTIAL_IDS: OnceLock<Mutex<BTreeSet<String>>> = OnceLock::new();

#[cfg(unix)]
fn gateway_session_credentials() -> &'static Mutex<Option<SecretBytes>> {
    GATEWAY_SESSION_CREDENTIALS.get_or_init(|| Mutex::new(None))
}

#[cfg(unix)]
fn gateway_enabled_credential_ids() -> &'static Mutex<BTreeSet<String>> {
    GATEWAY_ENABLED_CREDENTIAL_IDS.get_or_init(|| Mutex::new(BTreeSet::new()))
}

fn validate_credential_id(credential_id: &str) -> Result<()> {
    ensure!(
        uuid::Uuid::parse_str(credential_id).is_ok_and(|value| value.to_string() == credential_id),
        "llm_api_key_credential_id_invalid"
    );
    Ok(())
}

fn authorization_report(
    authorized: bool,
    providers: Vec<String>,
    credential_ids: impl IntoIterator<Item = String>,
    reason_code: Option<&str>,
    credentials_applied: Option<bool>,
) -> Value {
    let ids: Vec<String> = credential_ids.into_iter().collect();
    let mut value = json!({
        "ok": true,
        "schemaVersion": "licoup.llm-gateway-authorization.v1",
        "authorized": authorized,
        "reasonCode": reason_code,
        "providers": providers,
        "authorizedCredentialIds": ids,
    });
    if let Some(applied) = credentials_applied {
        value["credentialsApplied"] = json!(applied);
    }
    value
}

fn authorization_failure_report(error: &anyhow::Error) -> Option<Value> {
    let reason = match error.to_string().as_str() {
        "secure_mesh_keychain_classic_access_requires_user_action" => {
            "secure_mesh_keychain_classic_access_requires_user_action"
        }
        // Inventoried entries whose protected material is unreadable on this
        // build (for example an ad hoc local install that cannot see the
        // shared keychain group) must surface as their own recoverable cause,
        // not as an opaque authorization failure.
        "llm_api_key_inventory_inconsistent" => "llm_api_key_inventory_inconsistent",
        _ => return None,
    };
    Some(authorization_report(
        false,
        Vec::new(),
        Vec::new(),
        Some(reason),
        None,
    ))
}

#[cfg(unix)]
pub(crate) fn replace_gateway_session_credentials(
    handoff: Option<GatewayCredentialHandoff>,
) -> Result<()> {
    let payload = handoff
        .map(|handoff| handoff.to_json())
        .transpose()?
        .map(SecretBytes::try_from_bytes)
        .transpose()
        .map_err(|_| anyhow!("llm_gateway_session_credentials_unavailable"))?;
    *gateway_session_credentials()
        .lock()
        .map_err(|_| anyhow!("llm_gateway_session_credentials_unavailable"))? = payload;
    Ok(())
}

/// Rebuild only the credentials that were already explicitly authorized after
/// a vault mutation rotates the epoch. Saving a new key never adds it to the
/// active selection, and deleting a key removes only that identity.
#[cfg(unix)]
pub(crate) fn reconcile_gateway_session_credentials(
    vault: &crate::platform::llm_api_key_vault::PlatformLlmApiKeyVault,
    session: &SecretStoreAuthorizationSession,
    inventory: &LlmApiKeyInventory,
) -> Result<()> {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|_| anyhow!("llm_api_key_clock_invalid"))?
        .as_secs();
    let mut enabled = gateway_enabled_credential_ids()
        .lock()
        .map_err(|_| anyhow!("llm_gateway_session_credentials_unavailable"))?;
    *enabled = reconcile_enabled_credential_ids(&enabled, inventory, now);
    let ids: Vec<String> = enabled.iter().cloned().collect();
    let handoff = if ids.is_empty() {
        None
    } else {
        vault.gateway_handoff_from_authorized_inventory(session, inventory, Some(&ids))?
    };
    if handoff.is_none() {
        enabled.clear();
    }
    replace_gateway_session_credentials(handoff)?;
    drop(enabled);
    apply_session_credentials_to_managed_sidecar()?;
    Ok(())
}

fn reconcile_enabled_credential_ids(
    enabled: &BTreeSet<String>,
    inventory: &licoup_vault::api_key::LlmApiKeyInventory,
    now_epoch_seconds: u64,
) -> BTreeSet<String> {
    let available: BTreeSet<&str> = inventory
        .entries
        .iter()
        .filter(|entry| !entry.is_expired(now_epoch_seconds))
        .map(|entry| entry.credential_id.as_str())
        .collect();
    enabled
        .iter()
        .filter(|credential_id| available.contains(credential_id.as_str()))
        .cloned()
        .collect()
}

#[cfg(unix)]
fn cached_handoff_filtered_to(
    enabled: &BTreeSet<String>,
) -> Result<Option<GatewayCredentialHandoff>> {
    let cached = gateway_session_credentials()
        .lock()
        .map_err(|_| anyhow!("llm_gateway_session_credentials_unavailable"))?;
    let Some(payload) = cached.as_ref() else {
        return Ok(None);
    };
    let mut handoff = GatewayCredentialHandoff::from_json(payload.expose_bytes())?;
    handoff.retain_credential_ids(enabled);
    Ok((!handoff.is_empty()).then_some(handoff))
}

#[derive(Clone, Debug)]
struct ServicePaths {
    config: PathBuf,
    pid: PathBuf,
    log: PathBuf,
    usage: PathBuf,
    credentials_control: PathBuf,
    client_token: PathBuf,
}

impl ServicePaths {
    fn resolve() -> Result<Self> {
        let root = paths::portable_data_dir()?.join(STATE_DIRECTORY);
        ensure_private_dir(&root)?;
        Ok(Self {
            credentials_control: licoup_private_gateway::control::credentials::control_socket_path(
                &root,
            ),
            client_token: licoup_private_gateway::control::client_auth::default_token_path()?,
            config: root.join("config.json"),
            pid: root.join("gateway.pid"),
            log: root.join("gateway.log"),
            usage: root.join("usage.json"),
        })
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct PidRecord {
    pid: u32,
    port: u16,
    started_at: u64,
    #[serde(default)]
    credentials_loaded: bool,
}

/// Report the sidecar state without spawning anything or prompting.
pub fn service_status(port: u16) -> Result<Value> {
    status_report(port, HEALTH_PROBE_TIMEOUT)
}

/// Return only the private counters produced by requests that traversed the
/// local Gateway. Reading statistics never authorizes credential access.
pub fn service_usage() -> Result<Value> {
    let paths = ServicePaths::resolve()?;
    licoup_private_gateway::usage::read_usage(&paths.usage)
}

/// Return the live provider catalogs exposed by a running managed Gateway.
/// Offline planning remains possible with an empty list, but a healthy Gateway
/// that rejects or cannot parse an upstream catalog fails instead of falling
/// back to product-owned model names.
pub(crate) fn service_model_catalog(
    port: u16,
) -> Result<Vec<licoup_private_gateway::model::llm_gateway_agent_config::GatewayAgentModel>> {
    if !probe_health(port, HEALTH_PROBE_TIMEOUT) {
        return Ok(Vec::new());
    }
    let paths = ServicePaths::resolve()?;
    let token = licoup_private_gateway::control::client_auth::read_token(&paths.client_token)?;
    let token = token
        .expose_utf8()
        .map_err(|_| anyhow!("gateway_client_token_invalid"))?;
    let response = ureq::AgentBuilder::new()
        .redirects(0)
        .timeout_connect(Duration::from_secs(2))
        .timeout_read(Duration::from_secs(60))
        .timeout_write(Duration::from_secs(5))
        .build()
        .get(&format!("http://127.0.0.1:{port}/v1/models"))
        .set("authorization", &format!("Bearer {token}"))
        .set("accept", "application/json")
        .call()
        .map_err(|_| anyhow!("llm_gateway_model_catalog_unavailable"))?;
    let mut body = Vec::new();
    response
        .into_reader()
        .take((MAX_GATEWAY_BODY_BYTES as u64).saturating_add(1))
        .read_to_end(&mut body)
        .map_err(|_| anyhow!("llm_gateway_model_catalog_unavailable"))?;
    ensure!(
        body.len() <= MAX_GATEWAY_BODY_BYTES,
        "llm_gateway_model_catalog_invalid"
    );
    let document: Value =
        serde_json::from_slice(&body).map_err(|_| anyhow!("llm_gateway_model_catalog_invalid"))?;
    let rows = document
        .get("data")
        .and_then(Value::as_array)
        .ok_or_else(|| anyhow!("llm_gateway_model_catalog_invalid"))?;
    let mut seen = BTreeSet::new();
    let mut models = Vec::with_capacity(rows.len());
    for row in rows {
        let id = row
            .get("id")
            .and_then(Value::as_str)
            .filter(|id| !id.is_empty())
            .ok_or_else(|| anyhow!("llm_gateway_model_catalog_invalid"))?;
        if !seen.insert(id.to_owned()) {
            continue;
        }
        let name = row
            .get("name")
            .and_then(Value::as_str)
            .filter(|name| {
                !name.is_empty() && name.len() <= 1024 && !name.chars().any(char::is_control)
            })
            .unwrap_or(id);
        models.push(
            licoup_private_gateway::model::llm_gateway_agent_config::GatewayAgentModel {
                id: id.to_owned(),
                name: name.to_owned(),
            },
        );
    }
    Ok(models)
}

/// Start the local service during client initialization without touching the
/// Keychain. Credential authorization remains a separate action.
pub fn service_initialize(port: u16) -> Result<Value> {
    // `service_start` is health-aware and also replaces a managed sidecar that
    // predates current protocol capabilities. Returning status directly here
    // would leave an old process running forever across client upgrades.
    service_start(port)
}

/// Request user presence and load selected system-keyring credentials into
/// the native app process. When `credential_id` is set, only that key is added
/// to the active selection; when omitted, every non-expired key is enabled.
/// When a managed Gateway is already running, the loaded session is hot-applied
/// over the private credentials control socket without restarting the process.
pub fn credentials_authorize(credential_id: Option<&str>) -> Result<Value> {
    #[cfg(unix)]
    {
        if let Some(credential_id) = credential_id {
            validate_credential_id(credential_id)?;
        }
        let vault = crate::platform::llm_api_key_vault::PlatformLlmApiKeyVault::production()?;
        let inventory = vault.list()?;
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(|_| anyhow!("llm_api_key_clock_invalid"))?
            .as_secs();
        let available: BTreeSet<String> = inventory
            .entries
            .iter()
            .filter(|entry| !entry.is_expired(now))
            .map(|entry| entry.credential_id.clone())
            .collect();
        let mut enabled = gateway_enabled_credential_ids()
            .lock()
            .map_err(|_| anyhow!("llm_gateway_session_credentials_unavailable"))?;
        let next_enabled = match credential_id {
            Some(credential_id) => {
                ensure!(
                    available.contains(credential_id),
                    "llm_api_key_credential_unavailable"
                );
                let mut next = enabled.clone();
                next.insert(credential_id.to_owned());
                next
            }
            None => available,
        };
        let ids: Vec<String> = next_enabled.iter().cloned().collect();
        let handoff = if ids.is_empty() {
            None
        } else {
            match vault.authorize_gateway_handoff_filtered(Some(&ids)) {
                Ok(handoff) => handoff,
                Err(error) => {
                    // An unsuccessful attempt does not replace an existing
                    // lease, enable any new key, or hot-apply a partial batch.
                    return authorization_failure_report(&error).map_or_else(|| Err(error), Ok);
                }
            }
        };
        let providers = handoff
            .as_ref()
            .map(|value| {
                value
                    .providers()
                    .map(|provider| provider.as_str().to_owned())
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        let authorized = handoff.is_some();
        replace_gateway_session_credentials(handoff)?;
        *enabled = if authorized {
            next_enabled
        } else {
            BTreeSet::new()
        };
        let ids: Vec<String> = enabled.iter().cloned().collect();
        let credentials_applied = apply_session_credentials_to_managed_sidecar()?;
        Ok(authorization_report(
            authorized,
            providers,
            ids,
            if authorized {
                None
            } else {
                Some("no_credentials")
            },
            credentials_applied,
        ))
    }
    #[cfg(not(unix))]
    {
        let _ = credential_id;
        Err(anyhow!("llm_gateway_credential_authorization_unsupported"))
    }
}

/// Drop selected or all in-memory Gateway credentials without deleting vault
/// entries and without starting or stopping the sidecar. When `credential_id`
/// is set, only that key leaves the active selection. A running managed
/// sidecar receives the updated (or cleared) lease by hot apply.
pub fn credentials_clear(credential_id: Option<&str>) -> Result<Value> {
    #[cfg(unix)]
    {
        if let Some(credential_id) = credential_id {
            validate_credential_id(credential_id)?;
        }
        let mut enabled = gateway_enabled_credential_ids()
            .lock()
            .map_err(|_| anyhow!("llm_gateway_session_credentials_unavailable"))?;
        match credential_id {
            None => {
                enabled.clear();
                replace_gateway_session_credentials(None)?;
                let credentials_applied = apply_session_credentials_to_managed_sidecar()?;
                return Ok(authorization_report(
                    false,
                    Vec::new(),
                    Vec::<String>::new(),
                    None,
                    credentials_applied,
                ));
            }
            Some(credential_id) => {
                enabled.remove(credential_id);
            }
        }
        if enabled.is_empty() {
            replace_gateway_session_credentials(None)?;
            let credentials_applied = apply_session_credentials_to_managed_sidecar()?;
            return Ok(authorization_report(
                false,
                Vec::new(),
                Vec::<String>::new(),
                None,
                credentials_applied,
            ));
        }
        let handoff = cached_handoff_filtered_to(&enabled)?;
        let providers = handoff
            .as_ref()
            .map(|value| {
                value
                    .providers()
                    .map(|provider| provider.as_str().to_owned())
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        let authorized = handoff.is_some();
        replace_gateway_session_credentials(handoff)?;
        if !authorized {
            enabled.clear();
        }
        let remaining: Vec<String> = enabled.iter().cloned().collect();
        let credentials_applied = apply_session_credentials_to_managed_sidecar()?;
        Ok(authorization_report(
            authorized,
            providers,
            remaining,
            if authorized {
                None
            } else {
                Some("no_credentials")
            },
            credentials_applied,
        ))
    }
    #[cfg(not(unix))]
    {
        let _ = credential_id;
        Err(anyhow!("llm_gateway_credential_authorization_unsupported"))
    }
}

/// Start the sidecar detached, generating and validating the default
/// configuration first when it does not exist yet.
pub fn service_start(port: u16) -> Result<Value> {
    if let Some(health) = probe_health_response(port, HEALTH_PROBE_TIMEOUT) {
        let paths = ServicePaths::resolve()?;
        let managed = read_pid_record(&paths.pid)?.filter(|record| pid_alive(record.pid));
        let should_apply_loaded_credentials = managed
            .as_ref()
            .is_some_and(|record| session_credentials_loaded() && !record.credentials_loaded);
        let should_apply_current_protocols = !health_supports_current_protocols(&health);
        if should_apply_loaded_credentials {
            if let Ok(Some(applied)) = hot_apply_session_credentials(port) {
                if applied {
                    return Ok(report(
                        "running",
                        true,
                        managed.map(|record| record.pid),
                        port,
                        &paths,
                        Some("credentials_hot_applied"),
                    ));
                }
            }
        }
        if !should_apply_loaded_credentials && !should_apply_current_protocols {
            return Ok(report(
                "running",
                managed.is_some(),
                managed.map(|record| record.pid),
                port,
                &paths,
                Some("already_running"),
            ));
        }
        service_stop(port)?;
    }
    let paths = ServicePaths::resolve()?;
    let _ = licoup_private_gateway::control::client_auth::ensure_default_token()?;
    if let Some(record) = read_pid_record(&paths.pid)? {
        if pid_alive(record.pid) {
            return Err(anyhow!("llm_gateway_already_unhealthy"));
        }
        clear_pid_record(&paths.pid)?;
    }
    ensure_default_config(&paths)?;
    let sidecar = sidecar_path()?;
    // Starting the local service never authorizes. If the user already loaded
    // credentials into this app process, a restart reuses that in-memory
    // handoff; otherwise the sidecar starts disconnected but healthy.
    #[cfg(unix)]
    let mut child = spawn_sidecar_with_cached_credentials(&sidecar, &paths, port)?;
    #[cfg(not(unix))]
    let mut child = spawn_sidecar(&sidecar, &paths, port)?;
    let record = PidRecord {
        pid: child.id(),
        port,
        started_at: unix_seconds(),
        credentials_loaded: session_credentials_loaded(),
    };
    write_pid_record(&paths.pid, &record)?;
    let deadline = Instant::now() + START_TIMEOUT;
    loop {
        if child
            .try_wait()
            .map_err(|_| anyhow!("llm_gateway_start_failed"))?
            .is_some()
        {
            let _ = clear_pid_record(&paths.pid);
            return Err(anyhow!("llm_gateway_start_failed"));
        }
        if probe_health(port, HEALTH_PROBE_TIMEOUT) {
            return Ok(report(
                "running",
                true,
                Some(record.pid),
                port,
                &paths,
                None,
            ));
        }
        if Instant::now() >= deadline {
            let _ = terminate(record.pid);
            let _ = child.wait();
            let _ = clear_pid_record(&paths.pid);
            return Err(anyhow!("llm_gateway_start_failed"));
        }
        thread::sleep(POLL_INTERVAL);
    }
}

fn health_supports_current_protocols(health: &str) -> bool {
    health.contains("openai-chat-completions")
        && health.contains("anthropic-messages")
        && health.contains("bearer-or-x-api-key")
        && health.contains("live-provider-models")
}

#[cfg(unix)]
fn spawn_sidecar_with_cached_credentials(
    sidecar: &Path,
    paths: &ServicePaths,
    port: u16,
) -> Result<Child> {
    let cached = gateway_session_credentials()
        .lock()
        .map_err(|_| anyhow!("llm_gateway_session_credentials_unavailable"))?;
    match cached.as_ref() {
        Some(payload) => spawn_sidecar_with_handoff(sidecar, paths, port, payload.expose_bytes()),
        None => spawn_sidecar(sidecar, paths, port),
    }
}

/// Stop the managed sidecar. A healthy listener without a valid pid record is
/// never signalled: the CLI only stops processes it started itself.
pub fn service_stop(port: u16) -> Result<Value> {
    let paths = ServicePaths::resolve()?;
    let managed = read_pid_record(&paths.pid)?;
    let managed = match managed {
        Some(record) if pid_alive(record.pid) => Some(record),
        Some(_) => {
            clear_pid_record(&paths.pid)?;
            None
        }
        None => None,
    };
    let Some(record) = managed else {
        if probe_health(port, HEALTH_PROBE_TIMEOUT) {
            return Err(anyhow!("llm_gateway_unmanaged"));
        }
        return Ok(report(
            "stopped",
            false,
            None,
            port,
            &paths,
            Some("not_running"),
        ));
    };
    terminate(record.pid)?;
    loop {
        if !pid_alive(record.pid) && !probe_health(port, HEALTH_PROBE_TIMEOUT) {
            clear_pid_record(&paths.pid)?;
            let _ = std::fs::remove_file(&paths.credentials_control);
            return Ok(report("stopped", false, None, port, &paths, None));
        }
        thread::sleep(POLL_INTERVAL);
    }
}

/// Stop the Gateway only when this application owns a pid record, using the
/// port recorded by that owned process. Relocation must not guess a customized
/// port or signal an unmanaged listener.
pub fn service_stop_managed() -> Result<Value> {
    let paths = ServicePaths::resolve()?;
    service_stop_managed_at(&paths)
}

fn service_stop_managed_at(paths: &ServicePaths) -> Result<Value> {
    let Some(record) = read_pid_record(&paths.pid)? else {
        // This entry point controls only a sidecar owned by this data root.
        // Probing the default port here could make an unrelated local service
        // block a relocation even though LicoUp has no process to stop.
        return Ok(report(
            "stopped",
            false,
            None,
            DEFAULT_PORT,
            paths,
            Some("not_running"),
        ));
    };
    if pid_alive(record.pid) {
        terminate(record.pid)?;
        loop {
            if !pid_alive(record.pid) {
                clear_pid_record(&paths.pid)?;
                let _ = std::fs::remove_file(&paths.credentials_control);
                return Ok(report("stopped", false, None, record.port, paths, None));
            }
            thread::sleep(POLL_INTERVAL);
        }
    }
    clear_pid_record(&paths.pid)?;
    Ok(report(
        "stopped",
        false,
        None,
        record.port,
        paths,
        Some("not_running"),
    ))
}

fn status_report(port: u16, probe_timeout: Duration) -> Result<Value> {
    let paths = ServicePaths::resolve()?;
    let record = read_pid_record(&paths.pid)?;
    let stale = record.is_some();
    let live = record.filter(|record| pid_alive(record.pid));
    if stale && live.is_none() {
        clear_pid_record(&paths.pid)?;
    }
    let healthy = probe_health(port, probe_timeout);
    match (healthy, live) {
        (true, Some(record)) => Ok(report(
            "running",
            true,
            Some(record.pid),
            port,
            &paths,
            None,
        )),
        (true, None) => Ok(report("running", false, None, port, &paths, None)),
        (false, Some(record)) => Ok(report(
            "unhealthy",
            true,
            Some(record.pid),
            port,
            &paths,
            None,
        )),
        (false, None) => Ok(report("stopped", false, None, port, &paths, None)),
    }
}

fn report(
    state: &str,
    managed: bool,
    pid: Option<u32>,
    port: u16,
    paths: &ServicePaths,
    message: Option<&str>,
) -> Value {
    let credentials_loaded = session_credentials_loaded();
    let credentials_applied = pid.is_some_and(|expected_pid| {
        read_pid_record(&paths.pid)
            .ok()
            .flatten()
            .is_some_and(|record| record.pid == expected_pid && record.credentials_loaded)
    });
    let mut value = json!({
        "ok": true,
        "schemaVersion": REPORT_SCHEMA,
        "state": state,
        "managed": managed,
        "pid": pid,
        "processName": if pid.is_some() { Some(SIDECAR_BINARY) } else { None },
        "port": port,
        "credentialsLoaded": credentials_loaded,
        "credentialsApplied": credentials_applied,
        "modelReady": state == "running" && credentials_applied,
        "configPath": paths.config,
        "logPath": paths.log,
    });
    if let Some(message) = message {
        value["message"] = json!(message);
    }
    value
}

#[cfg(unix)]
fn session_credentials_loaded() -> bool {
    gateway_session_credentials()
        .lock()
        .map(|credentials| credentials.is_some())
        .unwrap_or(false)
}

#[cfg(not(unix))]
fn session_credentials_loaded() -> bool {
    false
}

/// Hot-apply the process-local session handoff to a managed running sidecar.
/// Returns `None` when no managed sidecar is available for this port; `Some`
/// reflects whether the sidecar lease is connected after the apply.
#[cfg(unix)]
fn hot_apply_session_credentials(port: u16) -> Result<Option<bool>> {
    let paths = ServicePaths::resolve()?;
    let Some(record) =
        read_pid_record(&paths.pid)?.filter(|record| pid_alive(record.pid) && record.port == port)
    else {
        return Ok(None);
    };
    if !paths.credentials_control.exists() {
        return Ok(None);
    }
    let handoff = {
        let cached = gateway_session_credentials()
            .lock()
            .map_err(|_| anyhow!("llm_gateway_session_credentials_unavailable"))?;
        match cached.as_ref() {
            Some(payload) => Some(GatewayCredentialHandoff::from_json(payload.expose_bytes())?),
            None => None,
        }
    };
    let loaded = licoup_private_gateway::control::credentials::apply_credentials_hot(
        &paths.credentials_control,
        handoff.as_ref(),
    )?;
    write_pid_record(
        &paths.pid,
        &PidRecord {
            pid: record.pid,
            port: record.port,
            started_at: record.started_at,
            credentials_loaded: loaded,
        },
    )?;
    Ok(Some(loaded))
}

/// Apply session credentials to the managed sidecar. Prefers hot apply; when the
/// live process has no control socket (pre-hot-apply builds), falls back to one
/// soft restart so the new lease is installed.
#[cfg(unix)]
fn apply_session_credentials_to_managed_sidecar() -> Result<Option<bool>> {
    let paths = ServicePaths::resolve()?;
    let Some(record) = read_pid_record(&paths.pid)?.filter(|record| pid_alive(record.pid)) else {
        return Ok(None);
    };
    match hot_apply_session_credentials(record.port) {
        Ok(Some(applied)) => Ok(Some(applied)),
        Ok(None) | Err(_) => {
            if !session_credentials_loaded()
                && !read_pid_record(&paths.pid)?.is_some_and(|current| current.credentials_loaded)
            {
                // Cleared session against a sidecar that never had keys.
                return Ok(Some(false));
            }
            // Legacy sidecar or control unavailable: one soft restart.
            let port = record.port;
            let _ = service_stop(port);
            let report = service_start(port)?;
            Ok(report
                .get("credentialsApplied")
                .and_then(Value::as_bool)
                .or(Some(session_credentials_loaded())))
        }
    }
}

#[cfg(not(unix))]
fn hot_apply_session_credentials(_port: u16) -> Result<Option<bool>> {
    Ok(None)
}

#[cfg(not(unix))]
fn apply_session_credentials_to_managed_sidecar() -> Result<Option<bool>> {
    Ok(None)
}

/// Partial hot-reload of conversation readiness (verified inventory).
///
/// Updates only the live readiness projection so newly verified agents appear
/// in Telegram `/agent`. Never restarts the Gateway process and never clears
/// Telegram bindings, bound `session_id`s, or in-flight conversation turns —
/// already-in-use chats keep their agent/session until the user rebinds.
///
/// Writes the overlay first, then pushes over `inventory.sock`. When the
/// control socket is unavailable, the overlay stays for the next start
/// (`overlay_pending`); the running process is left untouched.
pub fn reload_conversation_inventory(readiness_json: &str) -> Result<Value> {
    let paths = ServicePaths::resolve()?;
    let state_root = paths
        .credentials_control
        .parent()
        .ok_or_else(|| anyhow!("gateway_inventory_control_unavailable"))?
        .to_path_buf();
    let overlay = licoup_private_gateway::control::inventory::overlay_path(&state_root);
    licoup_private_gateway::control::inventory::write_inventory_overlay(&overlay, readiness_json)?;

    let Some(record) = read_pid_record(&paths.pid)?.filter(|record| pid_alive(record.pid)) else {
        return Ok(json!({
            "ok": true,
            "schemaVersion": "licoup.gateway-inventory-reload.v1",
            "reloaded": true,
            "mode": "overlay_only",
            "liveApplied": false,
            "sessionsPreserved": true,
            "overlayPath": overlay,
        }));
    };

    #[cfg(unix)]
    {
        let socket = licoup_private_gateway::control::inventory::control_socket_path(&state_root);
        // Brief retry covers socket bind races without ever restarting the process.
        let mut last_error = None;
        for _ in 0..8 {
            match licoup_private_gateway::control::inventory::apply_inventory_hot(
                &socket,
                readiness_json,
            ) {
                Ok(()) => {
                    return Ok(json!({
                        "ok": true,
                        "schemaVersion": "licoup.gateway-inventory-reload.v1",
                        "reloaded": true,
                        "mode": "hot",
                        "liveApplied": true,
                        "sessionsPreserved": true,
                        "pid": record.pid,
                        "port": record.port,
                        "overlayPath": overlay,
                    }));
                }
                Err(error) => {
                    last_error = Some(error.to_string());
                    std::thread::sleep(std::time::Duration::from_millis(50));
                }
            }
        }
        Ok(json!({
            "ok": true,
            "schemaVersion": "licoup.gateway-inventory-reload.v1",
            "reloaded": true,
            "mode": "overlay_pending",
            "liveApplied": false,
            "sessionsPreserved": true,
            "pid": record.pid,
            "port": record.port,
            "overlayPath": overlay,
            "message": last_error.unwrap_or_else(|| {
                "gateway_inventory_control_unavailable".to_owned()
            }),
        }))
    }

    #[cfg(not(unix))]
    {
        let _ = record;
        Err(anyhow!("gateway_inventory_control_unsupported"))
    }
}

fn default_config() -> GatewayConfig {
    let providers = vec![
        GatewayProvider {
            id: "kimi".to_owned(),
            base_url: "https://api.moonshot.cn/v1".to_owned(),
            protocol: UpstreamProtocol::OpenAiChatCompletions,
            credential_provider: LlmApiKeyProvider::Kimi,
            credential_style: CredentialStyle::Bearer,
        },
        GatewayProvider {
            id: "deepseek".to_owned(),
            base_url: "https://api.deepseek.com/v1".to_owned(),
            protocol: UpstreamProtocol::OpenAiChatCompletions,
            credential_provider: LlmApiKeyProvider::DeepSeek,
            credential_style: CredentialStyle::Bearer,
        },
        GatewayProvider {
            id: "kilo".to_owned(),
            base_url: "https://api.kilo.ai/api/gateway".to_owned(),
            protocol: UpstreamProtocol::OpenAiChatCompletions,
            credential_provider: LlmApiKeyProvider::Kilo,
            credential_style: CredentialStyle::Bearer,
        },
    ];
    GatewayConfig {
        schema_version: 1,
        providers,
        routes: Vec::new(),
    }
}

/// Materialize only the fixed provider boundaries. Model inventory is fetched
/// from each provider with the live authorized credential and is never baked
/// into this private configuration.
fn ensure_default_config(paths: &ServicePaths) -> Result<()> {
    let config = default_config();
    let body = serde_json::to_string_pretty(&config)?;
    atomic_write_private_text(&paths.config, &body)?;
    validate_config(&paths.config)
}

fn validate_config(path: &Path) -> Result<()> {
    let Some(text) = read_private_text_bounded(path, MAX_CONFIG_BYTES)? else {
        return Err(anyhow!("llm_gateway_config_invalid"));
    };
    let config: GatewayConfig =
        serde_json::from_str(&text).map_err(|_| anyhow!("llm_gateway_config_invalid"))?;
    CompiledGateway::compile(config).map_err(|_| anyhow!("llm_gateway_config_invalid"))?;
    Ok(())
}

fn sidecar_path() -> Result<PathBuf> {
    let current = std::env::current_exe().map_err(|_| anyhow!("llm_gateway_sidecar_missing"))?;
    let parent = licoup_foundation::platform::paths::packaged_binary_directory(&current)
        .ok_or_else(|| anyhow!("llm_gateway_sidecar_missing"))?;
    for name in [SIDECAR_BINARY, LEGACY_SIDECAR_BINARY] {
        let sibling = parent.join(format!("{}{}", name, std::env::consts::EXE_SUFFIX));
        if let Ok(metadata) = std::fs::symlink_metadata(&sibling) {
            if metadata.file_type().is_file() {
                return Ok(sibling);
            }
        }
    }
    Err(anyhow!("llm_gateway_sidecar_missing"))
}

fn sidecar_command(sidecar: &Path, paths: &ServicePaths, port: u16) -> Result<Command> {
    let mut options = OpenOptions::new();
    options.create(true).append(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let log = options
        .open(&paths.log)
        .map_err(|_| anyhow!("llm_gateway_start_failed"))?;
    let log_errors = log
        .try_clone()
        .map_err(|_| anyhow!("llm_gateway_start_failed"))?;
    let _ = std::fs::remove_file(&paths.credentials_control);
    let mut command = Command::new(sidecar);
    command
        .arg("--config")
        .arg(&paths.config)
        .arg("--port")
        .arg(port.to_string())
        .arg("--usage")
        .arg(&paths.usage)
        .arg("--credentials-control")
        .arg(&paths.credentials_control)
        .arg("--client-token-file")
        .arg(&paths.client_token)
        .stdin(Stdio::null())
        .stdout(Stdio::from(log))
        .stderr(Stdio::from(log_errors));
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        unsafe {
            command.pre_exec(|| {
                if libc::setsid() == -1 {
                    return Err(std::io::Error::last_os_error());
                }
                Ok(())
            });
        }
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt as WindowsCommandExt;
        command.creation_flags(0x0000_0008 | 0x0000_0200 | 0x0800_0000);
    }
    Ok(command)
}

fn spawn_sidecar(sidecar: &Path, paths: &ServicePaths, port: u16) -> Result<Child> {
    sidecar_command(sidecar, paths, port)?
        .spawn()
        .map_err(|_| anyhow!("llm_gateway_start_failed"))
}

/// Spawn the sidecar with the unlocked credentials handed over through an
/// inherited pipe whose read end is fixed at fd 3 in the child. The payload
/// is written only after the spawn succeeds; a failed write means the sidecar
/// can never become healthy and is treated as a start failure.
#[cfg(unix)]
fn spawn_sidecar_with_handoff(
    sidecar: &Path,
    paths: &ServicePaths,
    port: u16,
    payload: &[u8],
) -> Result<Child> {
    use std::os::fd::{FromRawFd, RawFd};
    use std::os::unix::process::CommandExt;

    let mut ends = [0 as RawFd; 2];
    if unsafe { libc::pipe(ends.as_mut_ptr()) } == -1 {
        return Err(anyhow!("llm_gateway_start_failed"));
    }
    let read_fd = ends[0];
    let write_fd = ends[1];
    // Close-on-exec on both ends immediately so nothing inherited past the
    // sidecar can leak them; the child's dup2 target clears CLOEXEC and
    // survives the exec.
    for fd in [read_fd, write_fd] {
        if unsafe { libc::fcntl(fd, libc::F_SETFD, libc::FD_CLOEXEC) } == -1 {
            unsafe {
                libc::close(read_fd);
                libc::close(write_fd);
            }
            return Err(anyhow!("llm_gateway_start_failed"));
        }
    }
    let mut command = match sidecar_command(sidecar, paths, port) {
        Ok(command) => command,
        Err(error) => {
            unsafe {
                libc::close(read_fd);
                libc::close(write_fd);
            }
            return Err(error);
        }
    };
    command.arg("--credentials-fd").arg("3");
    unsafe {
        command.pre_exec(move || {
            // pre_exec forces the fork+exec path, so this fd fix-up runs in the
            // child after fork and before exec with no ordering races.
            if read_fd != 3 {
                if libc::dup2(read_fd, 3) == -1 {
                    return Err(std::io::Error::last_os_error());
                }
            } else if libc::fcntl(3, libc::F_SETFD, 0) == -1 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
    }
    let mut child = match command.spawn() {
        Ok(child) => child,
        Err(_) => {
            unsafe {
                libc::close(read_fd);
                libc::close(write_fd);
            }
            return Err(anyhow!("llm_gateway_start_failed"));
        }
    };
    // Spawn succeeded: the parent keeps no read end. Write the whole payload
    // (write_all retries interrupted writes), then close the write end so the
    // sidecar reads to EOF. The parent's other secret copies zeroize through
    // SecretBytes drop; this payload buffer is dropped with the start flow.
    unsafe { libc::close(read_fd) };
    let write_result = {
        let mut write_end = unsafe { std::fs::File::from_raw_fd(write_fd) };
        let result = write_end.write_all(payload);
        drop(write_end);
        result
    };
    if let Err(error) = write_result {
        // A broken pipe means the sidecar died before reading the handoff;
        // either way the doomed child is reaped instead of left running.
        if error.kind() == std::io::ErrorKind::BrokenPipe {
            let _ = terminate(child.id());
            let _ = child.wait();
            let _ = clear_pid_record(&paths.pid);
        }
        return Err(anyhow!("llm_gateway_start_failed"));
    }
    Ok(child)
}

fn probe_health(port: u16, timeout: Duration) -> bool {
    probe_health_response(port, timeout).is_some()
}

fn probe_health_response(port: u16, timeout: Duration) -> Option<String> {
    let address = SocketAddr::from(([127, 0, 0, 1], port));
    let Ok(mut stream) = TcpStream::connect_timeout(&address, timeout) else {
        return None;
    };
    let _ = stream.set_read_timeout(Some(timeout));
    let _ = stream.set_write_timeout(Some(timeout));
    if stream
        .write_all(b"GET /health HTTP/1.1\r\nhost: 127.0.0.1\r\nconnection: close\r\n\r\n")
        .is_err()
    {
        return None;
    }
    let mut received = Vec::with_capacity(HEALTH_PROBE_BYTES);
    let mut chunk = [0u8; 1024];
    while received.len() < HEALTH_PROBE_BYTES {
        match stream.read(&mut chunk) {
            Ok(0) => break,
            Ok(count) => received.extend_from_slice(&chunk[..count]),
            Err(_) => break,
        }
    }
    let text = String::from_utf8_lossy(&received);
    (text
        .lines()
        .next()
        .is_some_and(|status| status.contains(" 200"))
        && text.contains("licoup-llm-gateway"))
    .then(|| text.into_owned())
}

fn pid_alive(pid: u32) -> bool {
    if pid == 0 {
        return false;
    }
    #[cfg(unix)]
    {
        let signaled = unsafe { libc::kill(pid as i32, 0) } == 0;
        if !signaled && std::io::Error::last_os_error().raw_os_error() != Some(libc::EPERM) {
            return false;
        }
        // A zombie still answers kill(pid, 0); for lifecycle purposes it is
        // already dead and its gateway socket is gone. The long-lived CLI
        // host never reaps detached sidecars, so zombies are a normal state.
        !pid_is_zombie(pid)
    }
    #[cfg(windows)]
    {
        Command::new("tasklist")
            .args(["/FI", &format!("PID eq {}", pid)])
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .output()
            .map(|output| {
                output.status.success()
                    && output.stdout.len() <= 64 * 1024
                    && String::from_utf8_lossy(&output.stdout).contains(&pid.to_string())
            })
            .unwrap_or(false)
    }
}

#[cfg(target_os = "macos")]
fn pid_is_zombie(pid: u32) -> bool {
    // XNU drops zombies from proc_pidinfo's view while they still answer
    // kill(pid, 0), so after a successful kill an ESRCH here means zombie.
    // Other errors (e.g. EPERM for a foreign process) stay conservative.
    let mut info: libc::proc_bsdinfo = unsafe { std::mem::zeroed() };
    let size = std::mem::size_of::<libc::proc_bsdinfo>() as i32;
    let written = unsafe {
        libc::proc_pidinfo(
            pid as i32,
            libc::PROC_PIDTBSDINFO,
            0,
            (&mut info as *mut libc::proc_bsdinfo).cast(),
            size,
        )
    };
    written <= 0 && std::io::Error::last_os_error().raw_os_error() == Some(libc::ESRCH)
}

#[cfg(target_os = "linux")]
fn pid_is_zombie(pid: u32) -> bool {
    let Ok(stat) = std::fs::read_to_string(format!("/proc/{pid}/stat")) else {
        return false;
    };
    // comm may contain spaces and parentheses; state follows the last ')'.
    stat.rsplit_once(')')
        .and_then(|(_, rest)| rest.split_whitespace().next())
        == Some("Z")
}

#[cfg(all(unix, not(any(target_os = "macos", target_os = "linux"))))]
fn pid_is_zombie(_pid: u32) -> bool {
    false
}

fn terminate(pid: u32) -> Result<()> {
    #[cfg(unix)]
    {
        let result = unsafe { libc::kill(pid as i32, libc::SIGTERM) };
        if result == 0 {
            return Ok(());
        }
        let error = std::io::Error::last_os_error();
        if error.raw_os_error() == Some(libc::ESRCH) {
            return Ok(());
        }
        return Err(anyhow!("llm_gateway_stop_failed"));
    }
    #[cfg(windows)]
    {
        let status = Command::new("taskkill")
            .args(["/PID", &pid.to_string(), "/T"])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .map_err(|_| anyhow!("llm_gateway_stop_failed"))?;
        if status.success() || !pid_alive(pid) {
            Ok(())
        } else {
            Err(anyhow!("llm_gateway_stop_failed"))
        }
    }
}

fn read_pid_record(path: &Path) -> Result<Option<PidRecord>> {
    let Some(text) = read_private_text_bounded(path, MAX_PID_BYTES)? else {
        return Ok(None);
    };
    Ok(serde_json::from_str::<PidRecord>(&text)
        .ok()
        .filter(|record| record.pid != 0))
}

fn write_pid_record(path: &Path, record: &PidRecord) -> Result<()> {
    atomic_write_private_text(path, &serde_json::to_string(record)?)
}

fn clear_pid_record(path: &Path) -> Result<()> {
    remove_private_state_marker(path)?;
    Ok(())
}

fn unix_seconds() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::TcpListener;

    #[test]
    fn authorization_failure_projects_only_allowlisted_keychain_action() {
        let report = authorization_failure_report(&anyhow!(
            "secure_mesh_keychain_classic_access_requires_user_action"
        ))
        .unwrap();
        assert_eq!(report["authorized"], false);
        assert_eq!(
            report["reasonCode"],
            "secure_mesh_keychain_classic_access_requires_user_action"
        );
        assert_eq!(report["providers"], json!([]));
        assert_eq!(report["authorizedCredentialIds"], json!([]));
        assert!(report.get("credentialsApplied").is_none());
        assert!(
            authorization_failure_report(&anyhow!("synthetic private platform detail")).is_none()
        );
        assert!(
            authorization_failure_report(&anyhow!("secure_mesh_authorization_required")).is_none()
        );
        let inconsistent =
            authorization_failure_report(&anyhow!("llm_api_key_inventory_inconsistent")).unwrap();
        assert_eq!(inconsistent["authorized"], false);
        assert_eq!(
            inconsistent["reasonCode"],
            "llm_api_key_inventory_inconsistent"
        );
    }

    struct PortableDataDirOverrideGuard {
        previous: Option<PathBuf>,
    }

    impl PortableDataDirOverrideGuard {
        fn set(path: PathBuf) -> Self {
            let previous = paths::set_portable_data_dir_override(Some(path));
            Self { previous }
        }
    }

    impl Drop for PortableDataDirOverrideGuard {
        fn drop(&mut self) {
            paths::set_portable_data_dir_override(self.previous.take());
        }
    }

    fn temp_state_root() -> PathBuf {
        std::env::temp_dir().join(format!(
            "licoup-llm-gateway-service-{}",
            uuid::Uuid::new_v4()
        ))
    }

    fn closed_port() -> u16 {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        drop(listener);
        port
    }

    #[test]
    fn default_config_compiles_without_a_product_owned_model_catalog() {
        let config = default_config();
        let text = serde_json::to_string_pretty(&config).unwrap();
        let parsed: GatewayConfig = serde_json::from_str(&text).unwrap();
        CompiledGateway::compile(parsed).unwrap();
        assert_eq!(config.providers.len(), LlmApiKeyProvider::ALL.len());
        assert!(config.routes.is_empty());
        assert!(text.contains("\"credentialProvider\": \"kimi\""));
        assert!(text.contains("\"credentialProvider\": \"kilo\""));
        assert!(!text.contains("requestedModel"));
        assert!(!text.contains("upstreamModel"));
        assert!(text.contains("\"baseUrl\": \"https://api.kilo.ai/api/gateway\""));
        assert!(text.contains("\"protocol\": \"open_ai_chat_completions\""));
    }

    #[test]
    fn default_config_keeps_only_the_three_provider_boundaries() {
        let config = default_config();
        CompiledGateway::compile(config.clone()).unwrap();
        assert_eq!(config.providers.len(), LlmApiKeyProvider::ALL.len());
        assert_eq!(config.providers[0].id, "kimi");
        assert_eq!(config.providers[1].id, "deepseek");
        assert_eq!(config.providers[2].id, "kilo");
        assert!(config.routes.is_empty());
    }

    #[test]
    fn unleased_provider_model_reaches_the_credential_boundary() {
        let gateway = CompiledGateway::compile(default_config()).unwrap();
        let credentials = licoup_vault::api_key::GatewayCredentialSlot::disconnected();
        let request = serde_json::json!({
            "model": "deepseek:deepseek-v4-flash",
            "messages": [{"role": "user", "content": "synthetic"}]
        });
        let error = licoup_private_gateway::http::transport::exchange(
            &gateway,
            "/v1/chat/completions",
            &serde_json::to_vec(&request).unwrap(),
            None,
            None,
            &credentials,
        )
        .unwrap_err();
        assert_eq!(
            error,
            licoup_private_gateway::http::transport::GatewayTransportError::CredentialUnavailable
        );
    }

    #[test]
    fn pid_record_round_trip_and_garbage_reads_as_none() {
        let root = temp_state_root();
        let _guard = PortableDataDirOverrideGuard::set(root.clone());
        let paths = ServicePaths::resolve().unwrap();
        assert_eq!(read_pid_record(&paths.pid).unwrap(), None);

        let record = PidRecord {
            pid: 4242,
            port: DEFAULT_PORT,
            started_at: 1_700_000_000,
            credentials_loaded: false,
        };
        write_pid_record(&paths.pid, &record).unwrap();
        assert_eq!(read_pid_record(&paths.pid).unwrap(), Some(record));
        clear_pid_record(&paths.pid).unwrap();
        assert_eq!(read_pid_record(&paths.pid).unwrap(), None);

        atomic_write_private_text(&paths.pid, "not-a-pid-record").unwrap();
        assert_eq!(read_pid_record(&paths.pid).unwrap(), None);
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn managed_stop_without_an_owned_pid_record_does_not_probe_default_port() {
        let root = temp_state_root();
        let _guard = PortableDataDirOverrideGuard::set(root.clone());
        let paths = ServicePaths::resolve().unwrap();

        let result = service_stop_managed_at(&paths).unwrap();

        assert_eq!(result["state"], "stopped");
        assert_eq!(result["managed"], false);
        assert_eq!(result["port"], DEFAULT_PORT);
        assert_eq!(result["message"], "not_running");
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn health_probe_accepts_canned_200_and_rejects_closed_port() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let server = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut request = [0u8; 1024];
            let _ = stream.read(&mut request);
            stream
                .write_all(
                    b"HTTP/1.1 200 OK\r\ncontent-type: application/json\r\nconnection: close\r\n\r\n{\"ok\":true,\"service\":\"licoup-llm-gateway\",\"protocols\":[\"openai-responses\",\"openai-chat-completions\",\"anthropic-messages\"]}",
                )
                .unwrap();
        });
        assert!(probe_health(port, Duration::from_millis(500)));
        server.join().unwrap();

        assert!(!probe_health(closed_port(), Duration::from_millis(50)));
    }

    #[test]
    fn old_gateway_health_requires_replacement_for_live_provider_models() {
        let old = r#"{"protocols":["openai-responses","openai-chat-completions","anthropic-messages"],"clientAuth":"bearer-or-x-api-key"}"#;
        assert!(!health_supports_current_protocols(old));
        let current = r#"{"protocols":["openai-responses","openai-chat-completions","anthropic-messages","live-provider-models"],"clientAuth":"bearer-or-x-api-key"}"#;
        assert!(health_supports_current_protocols(current));
    }

    #[test]
    fn agent_config_catalog_reads_the_authenticated_live_gateway_snapshot() {
        let root = temp_state_root();
        let _guard = PortableDataDirOverrideGuard::set(root.clone());
        let _token = licoup_private_gateway::control::client_auth::ensure_default_token().unwrap();
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let server = thread::spawn(move || {
            for request_index in 0..2 {
                let (mut stream, _) = listener.accept().unwrap();
                let mut request = [0u8; 4096];
                let count = stream.read(&mut request).unwrap();
                let request = String::from_utf8_lossy(&request[..count]);
                if request_index == 0 {
                    assert!(request.starts_with("GET /health HTTP/1.1"));
                    let body = r#"{"ok":true,"service":"licoup-llm-gateway"}"#;
                    write!(
                        stream,
                        "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
                        body.len()
                    )
                    .unwrap();
                } else {
                    assert!(request.starts_with("GET /v1/models HTTP/1.1"));
                    assert!(request.contains("authorization: Bearer "));
                    let body =
                        r#"{"object":"list","data":[{"id":"kimi:kimi-k3","name":"Kimi K3"}]}"#;
                    write!(
                        stream,
                        "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
                        body.len()
                    )
                    .unwrap();
                }
            }
        });

        let models = service_model_catalog(port).unwrap();

        assert_eq!(models.len(), 1);
        assert_eq!(models[0].id, "kimi:kimi-k3");
        assert_eq!(models[0].name, "Kimi K3");
        server.join().unwrap();
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn status_on_empty_state_dir_reports_stopped_and_unmanaged() {
        let root = temp_state_root();
        let _guard = PortableDataDirOverrideGuard::set(root.clone());
        let port = closed_port();
        let report = status_report(port, Duration::from_millis(50)).unwrap();
        assert_eq!(report["ok"], json!(true));
        assert_eq!(report["schemaVersion"], json!(REPORT_SCHEMA));
        assert_eq!(report["state"], json!("stopped"));
        assert_eq!(report["managed"], json!(false));
        assert!(report["pid"].is_null());
        assert_eq!(report["port"], json!(port));
        let _ = std::fs::remove_dir_all(root);
    }

    #[cfg(unix)]
    #[test]
    fn status_clears_stale_pid_record_for_dead_process() {
        let root = temp_state_root();
        let _guard = PortableDataDirOverrideGuard::set(root.clone());
        let paths = ServicePaths::resolve().unwrap();
        let mut child = Command::new("/usr/bin/true").spawn().unwrap();
        let dead_pid = child.id();
        child.wait().unwrap();
        assert!(!pid_alive(dead_pid));
        write_pid_record(
            &paths.pid,
            &PidRecord {
                pid: dead_pid,
                port: DEFAULT_PORT,
                started_at: unix_seconds(),
                credentials_loaded: false,
            },
        )
        .unwrap();
        let report = status_report(closed_port(), Duration::from_millis(50)).unwrap();
        assert_eq!(report["state"], json!("stopped"));
        assert_eq!(report["managed"], json!(false));
        assert_eq!(read_pid_record(&paths.pid).unwrap(), None);
        let _ = std::fs::remove_dir_all(root);
    }

    #[cfg(unix)]
    #[test]
    fn unreaped_zombie_child_is_not_alive() {
        let mut child = Command::new("/usr/bin/true").spawn().unwrap();
        let zombie_pid = child.id();
        // Never wait before the assertion: the exited child stays a zombie
        // and still answers kill(pid, 0), so only the zombie check rejects it.
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while !pid_is_zombie(zombie_pid) && std::time::Instant::now() < deadline {
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        assert!(pid_is_zombie(zombie_pid));
        assert!(!pid_alive(zombie_pid));
        child.wait().unwrap();
        assert!(!pid_alive(zombie_pid));
    }

    #[cfg(unix)]
    #[test]
    fn gateway_session_credentials_are_replaceable_without_disk_state() {
        let mut credentials = std::collections::BTreeMap::new();
        credentials.insert(
            LlmApiKeyProvider::Kimi,
            vec![
                licoup_vault::api_key::GatewayCredential::new(
                    uuid::Uuid::new_v4().to_string(),
                    SecretBytes::try_from_string("synthetic-secret".to_owned()).unwrap(),
                    None,
                )
                .unwrap(),
            ],
        );
        let handoff = GatewayCredentialHandoff::new(
            credentials,
            licoup_vault::api_key::GatewayCredentialLeaseDays::Seven,
            "11111111-1111-4111-8111-111111111111".to_owned(),
        )
        .unwrap();

        replace_gateway_session_credentials(Some(handoff)).unwrap();

        let cached = gateway_session_credentials().lock().unwrap();
        let parsed = GatewayCredentialHandoff::from_json(
            cached
                .as_ref()
                .expect("session credential payload")
                .expose_bytes(),
        );
        assert!(parsed.is_ok());
        drop(cached);
        replace_gateway_session_credentials(None).unwrap();
    }

    #[test]
    fn vault_mutation_reconciliation_never_auto_authorizes_new_credentials() {
        use licoup_vault::api_key::{
            GatewayCredentialLeaseDays, LlmApiKeyInventory, LlmApiKeyMetadata,
        };

        let previously_authorized = uuid::Uuid::new_v4().to_string();
        let newly_saved = uuid::Uuid::new_v4().to_string();
        let expired = uuid::Uuid::new_v4().to_string();
        let inventory = LlmApiKeyInventory::new(
            GatewayCredentialLeaseDays::Seven,
            vec![
                LlmApiKeyMetadata {
                    credential_id: previously_authorized.clone(),
                    provider: LlmApiKeyProvider::Kimi,
                    label: "Previously authorized".to_owned(),
                    created_at_epoch_seconds: 1,
                    expires_at_epoch_seconds: Some(200),
                },
                LlmApiKeyMetadata {
                    credential_id: newly_saved.clone(),
                    provider: LlmApiKeyProvider::Kimi,
                    label: "Newly saved".to_owned(),
                    created_at_epoch_seconds: 1,
                    expires_at_epoch_seconds: Some(200),
                },
                LlmApiKeyMetadata {
                    credential_id: expired.clone(),
                    provider: LlmApiKeyProvider::Kimi,
                    label: "Expired".to_owned(),
                    created_at_epoch_seconds: 1,
                    expires_at_epoch_seconds: Some(50),
                },
            ],
        )
        .unwrap();
        let enabled = BTreeSet::from([previously_authorized.clone(), expired]);

        let reconciled = reconcile_enabled_credential_ids(&enabled, &inventory, 100);

        assert_eq!(reconciled, BTreeSet::from([previously_authorized]));
        assert!(!reconciled.contains(&newly_saved));
    }
}
