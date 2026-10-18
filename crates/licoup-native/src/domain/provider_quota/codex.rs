//! Codex quota source.
//!
//! Approach (reimplemented from CodexBar's documented behavior, no code
//! copied): read the OAuth access token from the agent's local auth store
//! (`auth.json` under the Codex home resolved through home-dir APIs, never a
//! literal absolute path), fetch the hosted usage endpoint, and fall back to
//! the app-server stdio JSON-RPC `account/rateLimits/read` lane launched
//! through the same `codex app-server --stdio` invocation contract as the
//! existing Codex launch binding.

use super::contract::{
    DEFAULT_STALE_AFTER_SECONDS, ProviderQuotaSnapshot, QuotaFetchError, QuotaIdentity,
    QuotaProvider, QuotaStatus, QuotaWindow,
};
use super::credentials;
use super::http;
use serde_json::{Value, json};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};
use time::OffsetDateTime;

const WHAM_USAGE_URL: &str = "https://chatgpt.com/backend-api/wham/usage";
/// CodexBar-documented app-server lane discipline: 8s to establish the stdio
/// exchange, 3s to read the response.
const APP_SERVER_CONNECT_TIMEOUT: Duration = Duration::from_secs(8);
const APP_SERVER_READ_TIMEOUT: Duration = Duration::from_secs(3);
const MAX_APP_SERVER_OUTPUT_BYTES: usize = 256 * 1024;
const SHUTDOWN_GRACE: Duration = Duration::from_millis(800);
const POLL_INTERVAL: Duration = Duration::from_millis(20);
const LUNA_RESERVE_LABEL: &str = "Luna Reserve";
const LUNA_RESERVE_LIMIT_ID: &str = "base_model_inference";
const LUNA_RESERVE_MODEL: &str = "gpt-reserve";
const APP_SERVER_CAPABILITY_REJECTED: &str = "codex_app_server_capability_rejected";

type HostedFetch = dyn Fn(&str, &str) -> Result<Value, QuotaFetchError> + Send + Sync;
type AppServerFetch = dyn Fn(&Path) -> Result<Value, QuotaFetchError> + Send + Sync;

pub(super) struct CodexSource {
    auth_path: Option<PathBuf>,
    executable: Option<PathBuf>,
    fetch_usage: Box<HostedFetch>,
    read_rate_limits: Box<AppServerFetch>,
}

impl CodexSource {
    /// Production source: credential path resolved through home-dir helpers,
    /// hosted fetch over bounded HTTPS, fallback lane over the existing Codex
    /// app-server launch binding.
    pub(super) fn production(params: &Value) -> Self {
        Self {
            auth_path: resolve_codex_home(params).map(|home| home.join("auth.json")),
            executable: crate::domain::targets::agent_cli_executable("codex"),
            fetch_usage: Box::new(|url, bearer| {
                http::get_json_with_bearer(url, bearer, http::HOSTED_FETCH_TIMEOUT)
            }),
            read_rate_limits: Box::new(|executable| app_server_rate_limits(executable)),
        }
    }

    #[cfg(test)]
    pub(super) fn for_testing(
        auth_path: Option<PathBuf>,
        executable: Option<PathBuf>,
        fetch_usage: Box<HostedFetch>,
        read_rate_limits: Box<AppServerFetch>,
    ) -> Self {
        Self {
            auth_path,
            executable,
            fetch_usage,
            read_rate_limits,
        }
    }

    pub(super) fn fetch_snapshot(
        &self,
        now: OffsetDateTime,
    ) -> Result<ProviderQuotaSnapshot, QuotaFetchError> {
        let captured_at = super::scheduler::format_rfc3339(now);
        let auth = self
            .auth_path
            .as_deref()
            .and_then(|path| credentials::read_bounded_utf8(path).ok())
            .and_then(|text| serde_json::from_str::<Value>(&text).ok());
        let identity = auth.as_ref().map(identity_from_auth).unwrap_or_default();

        if let Some(access_token) = auth
            .as_ref()
            .and_then(|auth| auth.pointer("/tokens/access_token"))
            .and_then(Value::as_str)
            && let Ok(payload) = (self.fetch_usage)(WHAM_USAGE_URL, access_token)
        {
            return normalize_wham_usage(&payload, identity, &captured_at);
        }

        // Fallback lane: app-server rate limits through the existing Codex
        // launch binding.
        let executable = self
            .executable
            .clone()
            .ok_or_else(|| QuotaFetchError::new("codex_app_server_unavailable"))?;
        let payload = (self.read_rate_limits)(&executable)?;
        normalize_app_server_rate_limits(&payload, identity, &captured_at)
    }
}

fn resolve_codex_home(params: &Value) -> Option<PathBuf> {
    if let Some(path) = crate::domain::conversation::parameters::text_param(params, &["codexHome"])
    {
        return Some(PathBuf::from(path));
    }
    if let Ok(path) = std::env::var("CODEX_HOME") {
        let trimmed = path.trim();
        if !trimmed.is_empty() {
            return Some(PathBuf::from(trimmed));
        }
    }
    licoup_foundation::platform::paths::user_home_from_env().map(|home| {
        licoup_foundation::platform::paths::strip_macos_data_volume(&home).join(".codex")
    })
}

/// Identity labels come from the user's own local auth store claims
/// (untrusted JWT payload decoded without signature trust) or, failing that,
/// the hosted payload's identity fields.
fn identity_from_auth(auth: &Value) -> QuotaIdentity {
    let claims = auth
        .pointer("/tokens/id_token")
        .and_then(Value::as_str)
        .and_then(credentials::decode_jwt_payload);
    QuotaIdentity {
        account_label: claims
            .as_ref()
            .and_then(|claims| claims.get("email"))
            .and_then(Value::as_str)
            .map(str::to_owned),
        plan: claims
            .as_ref()
            .and_then(|claims| {
                claims.get("chatgpt_plan_type").or_else(|| {
                    claims
                        .get("https://api.openai.com/auth")
                        .and_then(|nested| nested.get("chatgpt_plan_type"))
                })
            })
            .and_then(Value::as_str)
            .map(str::to_owned),
    }
}

fn snapshot(
    windows: Vec<QuotaWindow>,
    identity: QuotaIdentity,
    captured_at: &str,
) -> Result<ProviderQuotaSnapshot, QuotaFetchError> {
    if windows.is_empty() {
        return Err(QuotaFetchError::new("codex_quota_windows_missing"));
    }
    Ok(ProviderQuotaSnapshot {
        agent_id: QuotaProvider::Codex.agent_id().to_owned(),
        provider: QuotaProvider::Codex,
        status: QuotaStatus::Live,
        windows,
        identity,
        captured_at: captured_at.to_owned(),
        stale_after_seconds: DEFAULT_STALE_AFTER_SECONDS,
    })
}

/// Hosted `wham/usage` payload: primary/secondary windows carry
/// `used_percent`, `reset_at` (epoch seconds), and `limit_window_seconds`.
fn normalize_wham_usage(
    payload: &Value,
    mut identity: QuotaIdentity,
    captured_at: &str,
) -> Result<ProviderQuotaSnapshot, QuotaFetchError> {
    let rate_limit = payload
        .get("rate_limit")
        .or_else(|| payload.get("rateLimit"))
        .ok_or_else(|| QuotaFetchError::new("codex_usage_contract_invalid"))?;
    let mut windows = Vec::new();
    for (key, label) in [
        ("primary_window", "session"),
        ("secondary_window", "weekly"),
    ] {
        if let Some(window) = rate_limit.get(key)
            && let Some(normalized) = normalize_hosted_window(window, label)
        {
            windows.push(normalized);
        }
    }
    if let Some(additional) = payload
        .get("additional_rate_limits")
        .or_else(|| payload.get("additionalRateLimits"))
        .or_else(|| rate_limit.get("additional_rate_limits"))
        .or_else(|| rate_limit.get("additionalRateLimits"))
    {
        append_hosted_additional_windows(&mut windows, additional);
    }
    if identity.account_label.is_none() {
        identity.account_label = payload
            .get("account_email")
            .or_else(|| payload.get("accountEmail"))
            .and_then(Value::as_str)
            .map(str::to_owned);
    }
    if identity.plan.is_none() {
        identity.plan = payload
            .get("plan_type")
            .or_else(|| payload.get("planType"))
            .and_then(Value::as_str)
            .map(str::to_owned);
    }
    snapshot(windows, identity, captured_at)
}

fn normalize_hosted_window(window: &Value, label: &str) -> Option<QuotaWindow> {
    let used_percent = window
        .get("used_percent")
        .or_else(|| window.get("usedPercent"))
        .and_then(Value::as_f64)?;
    let window_minutes = window
        .get("limit_window_seconds")
        .or_else(|| window.get("limitWindowSeconds"))
        .and_then(Value::as_u64)
        .map(|seconds| seconds / 60);
    let resets_at = window
        .get("reset_at")
        .or_else(|| window.get("resetAt"))
        .and_then(|value| {
            value
                .as_i64()
                .map(epoch_seconds_to_rfc3339)
                .or_else(|| value.as_str().map(str::to_owned))
        });
    Some(QuotaWindow {
        label: label.to_owned(),
        used_percent,
        window_minutes,
        resets_at,
        reset_description: reset_description(window_minutes),
        ..QuotaWindow::default()
    })
}

/// App-server `account/rateLimits/read` result: rate limit windows keyed by
/// name carrying `usedPercent`, `windowMinutes`, and `resetsAt`.
fn normalize_app_server_rate_limits(
    payload: &Value,
    identity: QuotaIdentity,
    captured_at: &str,
) -> Result<ProviderQuotaSnapshot, QuotaFetchError> {
    let rate_limits = payload
        .get("rateLimits")
        .or_else(|| payload.get("rate_limits"))
        .ok_or_else(|| QuotaFetchError::new("codex_rate_limits_contract_invalid"))?;
    let mut windows = Vec::new();
    for (key, label) in [("primary", "session"), ("secondary", "weekly")] {
        if let Some(window) = rate_limits.get(key)
            && let Some(normalized) = normalize_app_server_window(window, label)
        {
            windows.push(normalized);
        }
    }
    if let Some(additional) = payload
        .get("rateLimitsByLimitId")
        .or_else(|| payload.get("rate_limits_by_limit_id"))
    {
        append_app_server_additional_windows(&mut windows, additional);
    }
    snapshot(windows, identity, captured_at)
}

fn string_field<'a>(value: &'a Value, keys: &[&str]) -> Option<&'a str> {
    keys.iter()
        .find_map(|key| value.get(*key).and_then(Value::as_str))
        .map(str::trim)
        .filter(|value| !value.is_empty())
}

fn is_luna_reserve_bucket(limit_id: Option<&str>, limit_name: Option<&str>) -> bool {
    limit_id.is_some_and(|value| {
        value.eq_ignore_ascii_case(LUNA_RESERVE_LIMIT_ID)
            || value.eq_ignore_ascii_case("base-model-inference")
    }) || limit_name.is_some_and(|value| value.eq_ignore_ascii_case(LUNA_RESERVE_MODEL))
}

fn append_hosted_additional_windows(windows: &mut Vec<QuotaWindow>, additional: &Value) {
    if let Some(items) = additional.as_array() {
        for item in items {
            append_hosted_reserve_bucket(windows, None, item);
        }
    } else if let Some(items) = additional.as_object() {
        for (key, item) in items {
            append_hosted_reserve_bucket(windows, Some(key), item);
        }
    }
}

fn append_hosted_reserve_bucket(
    windows: &mut Vec<QuotaWindow>,
    map_key: Option<&str>,
    bucket: &Value,
) {
    let limit_id = string_field(bucket, &["metered_feature", "meteredFeature"])
        .or(map_key)
        .or_else(|| string_field(bucket, &["limit_id", "limitId"]));
    let limit_name = string_field(bucket, &["limit_name", "limitName"]);
    if !is_luna_reserve_bucket(limit_id, limit_name) {
        return;
    }
    let rate_limit = bucket
        .get("rate_limit")
        .or_else(|| bucket.get("rateLimit"))
        .unwrap_or(bucket);
    for (key, suffix) in [("primary_window", ""), ("secondary_window", " (secondary)")] {
        let Some(window) = rate_limit.get(key) else {
            continue;
        };
        let label = format!("{LUNA_RESERVE_LABEL}{suffix}");
        if let Some(normalized) = normalize_hosted_window(window, &label) {
            windows.push(normalized);
        }
    }
}

fn append_app_server_additional_windows(windows: &mut Vec<QuotaWindow>, additional: &Value) {
    let Some(items) = additional.as_object() else {
        return;
    };
    for (key, snapshot) in items {
        let limit_id = string_field(snapshot, &["limitId", "limit_id"]).or(Some(key.as_str()));
        let limit_name = string_field(snapshot, &["limitName", "limit_name"]);
        if !is_luna_reserve_bucket(limit_id, limit_name) {
            continue;
        }
        for (window_key, suffix) in [("primary", ""), ("secondary", " (secondary)")] {
            let Some(window) = snapshot.get(window_key) else {
                continue;
            };
            let label = format!("{LUNA_RESERVE_LABEL}{suffix}");
            if let Some(normalized) = normalize_app_server_window(window, &label) {
                windows.push(normalized);
            }
        }
    }
}

fn normalize_app_server_window(window: &Value, label: &str) -> Option<QuotaWindow> {
    let used_percent = window
        .get("usedPercent")
        .or_else(|| window.get("used_percent"))
        .and_then(Value::as_f64)?;
    let window_minutes = window
        .get("windowMinutes")
        .or_else(|| window.get("window_minutes"))
        .and_then(Value::as_u64);
    let resets_at = window
        .get("resetsAt")
        .or_else(|| window.get("resets_at"))
        .and_then(|value| {
            value
                .as_str()
                .map(str::to_owned)
                .or_else(|| value.as_i64().map(epoch_seconds_to_rfc3339))
        });
    Some(QuotaWindow {
        label: label.to_owned(),
        used_percent,
        window_minutes,
        resets_at,
        reset_description: reset_description(window_minutes),
        ..QuotaWindow::default()
    })
}

fn epoch_seconds_to_rfc3339(epoch: i64) -> String {
    OffsetDateTime::from_unix_timestamp(epoch)
        .map(|value| super::scheduler::format_rfc3339(value))
        .unwrap_or_default()
}

fn reset_description(window_minutes: Option<u64>) -> String {
    match window_minutes {
        Some(minutes) if minutes % (24 * 60) == 0 => format!("{}-day window", minutes / (24 * 60)),
        Some(minutes) if minutes % 60 == 0 => format!("{}-hour window", minutes / 60),
        Some(minutes) => format!("{minutes}-minute window"),
        None => String::new(),
    }
}

/// One JSON-RPC `account/rateLimits/read` exchange over the app-server stdio
/// lane. Stdin is closed after the request; shutdown escalates SIGTERM to
/// SIGKILL so no app-server child outlives the exchange.
fn app_server_rate_limits(executable: &Path) -> Result<Value, QuotaFetchError> {
    match app_server_rate_limits_once(executable, true) {
        Err(error) if error.code == APP_SERVER_CAPABILITY_REJECTED => {
            app_server_rate_limits_once(executable, false)
        }
        result => result,
    }
}

fn app_server_rate_limits_once(
    executable: &Path,
    supports_luna_reserve: bool,
) -> Result<Value, QuotaFetchError> {
    use command_group::CommandGroup;

    let initialize = json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "initialize",
        "params": {"clientInfo": {"name": "licoup", "version": env!("CARGO_PKG_VERSION")}}
    });
    let read = json!({
        "jsonrpc": "2.0",
        "id": 2,
        "method": "account/rateLimits/read",
        "params": if supports_luna_reserve {
            json!({"supportsLunaReserve": true})
        } else {
            Value::Null
        }
    });
    let mut request = serde_json::to_vec(&initialize)
        .map_err(|_| QuotaFetchError::new("codex_app_server_request_invalid"))?;
    request.push(b'\n');
    request.extend(
        serde_json::to_vec(&read)
            .map_err(|_| QuotaFetchError::new("codex_app_server_request_invalid"))?,
    );
    request.push(b'\n');

    let mut command = Command::new(executable);
    crate::platform::user_shell_environment::apply_to_command(&mut command);
    command
        .arg("app-server")
        .arg("--stdio")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    let mut child = match command.group_spawn() {
        Ok(child) => child,
        Err(_) => return Err(QuotaFetchError::new("codex_app_server_start_failed")),
    };
    let result = exchange_rate_limits(&mut child, &request);
    shutdown_child_tree(&mut child);
    result
}

fn exchange_rate_limits(
    child: &mut command_group::GroupChild,
    request: &[u8],
) -> Result<Value, QuotaFetchError> {
    use std::io::Read;

    // The connect budget bounds the request write; the read budget bounds the
    // response wait, per the documented 8s/3s discipline. If the write ever
    // consumed the connect budget, the read window closes with the envelope.
    let exchange_start = Instant::now();
    {
        let mut stdin = child
            .inner()
            .stdin
            .take()
            .ok_or_else(|| QuotaFetchError::new("codex_app_server_stdin_unavailable"))?;
        stdin
            .write_all(request)
            .map_err(|_| QuotaFetchError::new("codex_app_server_write_failed"))?;
        // Stdin closes here, right after the request document.
    }
    let stdout = child
        .inner()
        .stdout
        .take()
        .ok_or_else(|| QuotaFetchError::new("codex_app_server_stdout_unavailable"))?;
    let reader = std::thread::spawn(move || {
        let mut bytes = Vec::with_capacity(4096);
        let _ = stdout
            .take(MAX_APP_SERVER_OUTPUT_BYTES as u64)
            .read_to_end(&mut bytes);
        bytes
    });

    let read_deadline = (exchange_start + APP_SERVER_CONNECT_TIMEOUT + APP_SERVER_READ_TIMEOUT)
        .min(Instant::now() + APP_SERVER_READ_TIMEOUT);
    while !reader.is_finished() && Instant::now() < read_deadline {
        std::thread::sleep(POLL_INTERVAL);
    }
    if !reader.is_finished() {
        return Err(QuotaFetchError::new("codex_app_server_read_timeout"));
    }
    let bytes = reader
        .join()
        .map_err(|_| QuotaFetchError::new("codex_app_server_read_failed"))?;
    parse_rate_limits_response(&bytes)
}

/// Pick the JSON-RPC response for the rate-limits request id from the stdio
/// output; notification lines and the initialize response are ignored.
pub(super) fn parse_rate_limits_response(bytes: &[u8]) -> Result<Value, QuotaFetchError> {
    for line in bytes.split(|byte| *byte == b'\n') {
        let Ok(value) = serde_json::from_slice::<Value>(line) else {
            continue;
        };
        if value.get("id").and_then(Value::as_i64) == Some(2)
            && let Some(result) = value.get("result")
        {
            return Ok(result.clone());
        }
        if value.get("id").and_then(Value::as_i64) == Some(2)
            && value
                .get("error")
                .and_then(|error| error.get("code"))
                .and_then(Value::as_i64)
                .is_some_and(|code| matches!(code, -32600 | -32602))
        {
            return Err(QuotaFetchError::new(APP_SERVER_CAPABILITY_REJECTED));
        }
    }
    Err(QuotaFetchError::new("codex_app_server_response_missing"))
}

fn shutdown_child_tree(child: &mut command_group::GroupChild) {
    #[cfg(unix)]
    {
        use command_group::{Signal, UnixChildExt};
        let _ = child.signal(Signal::SIGTERM);
        let deadline = Instant::now() + SHUTDOWN_GRACE;
        while Instant::now() < deadline {
            match child.try_wait() {
                Ok(Some(_)) => return,
                Ok(None) => std::thread::sleep(POLL_INTERVAL),
                Err(_) => break,
            }
        }
    }
    // Escalation: SIGKILL (or the platform kill) after the grace period.
    let _ = child.kill();
    let _ = child.wait();
}
