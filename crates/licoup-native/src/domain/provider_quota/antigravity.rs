//! Antigravity quota source.
//!
//! Approach (reimplemented from CodexBar's documented behavior, no code
//! copied): the Antigravity desktop runtime hosts a local language server that
//! answers quota summaries on a loopback HTTPS lane; the standalone `agy` CLI
//! exposes the same lane without a CSRF token. The lane binding — the
//! already-running server's listen port and the optional CSRF token from its
//! process arguments — is discovered from bounded local process snapshots and
//! probed over the loopback lane; the quota summary POST then reuses the
//! loopback discipline of LicoUp's local-service lanes (loopback-only URL
//! policy, bounded timeouts, no proxy inheritance). Every `groups[].buckets[]`
//! entry with a remaining fraction normalizes into one `QuotaWindow`; account
//! identity is enriched best-effort from a separate user-status call.

use super::contract::{
    DEFAULT_STALE_AFTER_SECONDS, ProviderQuotaSnapshot, QuotaFetchError, QuotaIdentity,
    QuotaProvider, QuotaStatus, QuotaWindow,
};
use super::http;
use serde_json::Value;
use std::process::Command;
use std::time::Duration;
use time::OffsetDateTime;

const QUOTA_SUMMARY_PATH: &str =
    "/exa.language_server_pb.LanguageServerService/RetrieveUserQuotaSummary";
const UNLEASH_PROBE_PATH: &str = "/exa.language_server_pb.LanguageServerService/GetUnleashData";
const USER_STATUS_PATH: &str = "/exa.language_server_pb.LanguageServerService/GetUserStatus";
const CONNECT_VERSION_HEADER: &str = "Connect-Protocol-Version";
const CSRF_HEADER: &str = "x-codeium-csrf-token";
const DISCOVERY_TIMEOUT: Duration = Duration::from_secs(3);
const MAX_DISCOVERY_BYTES: usize = 4 * 1024 * 1024;

/// Loopback lane binding for the local language server. The CSRF token is
/// credential material: it lives only in memory for the request build and is
/// never serialized, logged, or retained. The agy CLI lane carries no token.
#[derive(Clone)]
pub(super) struct LoopbackBinding {
    port: u16,
    csrf_token: String,
    use_https: bool,
}

impl std::fmt::Debug for LoopbackBinding {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("LoopbackBinding")
            .field("port", &self.port)
            .field("csrf_token", &"<redacted>")
            .field("use_https", &self.use_https)
            .finish()
    }
}

impl LoopbackBinding {
    #[cfg(test)]
    pub(super) fn for_testing(port: u16, csrf_token: &str, use_https: bool) -> Self {
        Self {
            port,
            csrf_token: csrf_token.to_owned(),
            use_https,
        }
    }

    fn url_for(&self, path: &str) -> String {
        format!(
            "{}://127.0.0.1:{}{}",
            if self.use_https { "https" } else { "http" },
            self.port,
            path
        )
    }

    fn quota_summary_url(&self) -> String {
        self.url_for(QUOTA_SUMMARY_PATH)
    }

    fn probe_url(&self) -> String {
        self.url_for(UNLEASH_PROBE_PATH)
    }

    fn user_status_url(&self) -> String {
        self.url_for(USER_STATUS_PATH)
    }
}

type LoopbackPost = dyn Fn(&LoopbackBinding) -> Result<Value, QuotaFetchError> + Send + Sync;
type Discovery = dyn Fn() -> Option<LoopbackBinding> + Send + Sync;

pub(super) struct AntigravitySource {
    binding: Option<LoopbackBinding>,
    discover: Box<Discovery>,
    post_summary: Box<LoopbackPost>,
    post_identity: Box<LoopbackPost>,
}

impl AntigravitySource {
    pub(super) fn production() -> Self {
        Self {
            binding: None,
            discover: Box::new(discover_loopback_binding),
            post_summary: Box::new(post_quota_summary),
            post_identity: Box::new(post_user_status),
        }
    }

    #[cfg(test)]
    pub(super) fn for_testing(
        binding: Option<LoopbackBinding>,
        post_summary: Box<LoopbackPost>,
    ) -> Self {
        Self {
            binding,
            discover: Box::new(|| None),
            post_summary,
            // The two-argument test constructor leaves identity enrichment
            // inert so the injected summary path stays authoritative.
            post_identity: Box::new(|_| Ok(Value::Null)),
        }
    }

    #[cfg(test)]
    pub(super) fn for_testing_with_identity(
        binding: Option<LoopbackBinding>,
        post_summary: Box<LoopbackPost>,
        post_identity: Box<LoopbackPost>,
    ) -> Self {
        Self {
            binding,
            discover: Box::new(|| None),
            post_summary,
            post_identity,
        }
    }

    pub(super) fn fetch_snapshot(
        &self,
        now: OffsetDateTime,
    ) -> Result<ProviderQuotaSnapshot, QuotaFetchError> {
        let captured_at = super::scheduler::format_rfc3339(now);
        let binding = self
            .binding
            .clone()
            .or_else(|| (self.discover)())
            .ok_or_else(|| QuotaFetchError::new("antigravity_loopback_lane_unavailable"))?;
        let payload = (self.post_summary)(&binding)?;
        let mut snapshot = normalize_quota_summary(&payload, &captured_at)?;
        // Identity enrichment is best-effort: a failing or empty user-status
        // response must not downgrade an otherwise live quota snapshot.
        if let Ok(status_payload) = (self.post_identity)(&binding) {
            let identity = identity_from_user_status(&status_payload);
            if identity != QuotaIdentity::default() {
                snapshot.identity = identity;
            }
        }
        Ok(snapshot)
    }
}

/// Lane headers for one loopback call: the Connect protocol version always,
/// plus the CSRF token only when the lane carries one (the desktop app/IDE
/// lane; the agy CLI lane answers without it).
fn lane_headers(binding: &LoopbackBinding) -> Vec<(&str, &str)> {
    let mut headers = vec![(CONNECT_VERSION_HEADER, "1")];
    if !binding.csrf_token.is_empty() {
        headers.push((CSRF_HEADER, binding.csrf_token.as_str()));
    }
    headers
}

fn post_quota_summary(binding: &LoopbackBinding) -> Result<Value, QuotaFetchError> {
    http::post_json_loopback(
        &binding.quota_summary_url(),
        &lane_headers(binding),
        br#"{"forceRefresh":true}"#,
        http::LOOPBACK_FETCH_TIMEOUT,
    )
}

fn post_user_status(binding: &LoopbackBinding) -> Result<Value, QuotaFetchError> {
    http::post_json_loopback(
        &binding.user_status_url(),
        &lane_headers(binding),
        br#"{"metadata":{"ideName":"antigravity","extensionName":"antigravity","ideVersion":"unknown","locale":"en"}}"#,
        http::LOOPBACK_FETCH_TIMEOUT,
    )
}

/// Quota summary payload: the root is `response` (with `summary` as a fallback
/// and the bare payload as a last resort), carrying `groups[].buckets[]` where
/// each bucket has `remainingFraction` and `resetTime`. Every valid bucket
/// becomes one window; buckets that are `disabled` or lack a remaining
/// fraction are skipped. The quota payload carries no account identity.
fn normalize_quota_summary(
    payload: &Value,
    captured_at: &str,
) -> Result<ProviderQuotaSnapshot, QuotaFetchError> {
    let root = payload
        .get("response")
        .or_else(|| payload.get("summary"))
        .unwrap_or(payload);
    let mut windows = Vec::new();
    if let Some(groups) = root.get("groups").and_then(Value::as_array) {
        for group in groups {
            let model_label = group_model_label(group);
            let Some(buckets) = group.get("buckets").and_then(Value::as_array) else {
                continue;
            };
            for bucket in buckets {
                if let Some(window) = normalize_bucket(bucket, &model_label) {
                    windows.push(window);
                }
            }
        }
    }
    if windows.is_empty() {
        return Err(QuotaFetchError::new("antigravity_quota_windows_missing"));
    }
    Ok(ProviderQuotaSnapshot {
        agent_id: QuotaProvider::Antigravity.agent_id().to_owned(),
        provider: QuotaProvider::Antigravity,
        status: QuotaStatus::Live,
        windows,
        identity: QuotaIdentity::default(),
        captured_at: captured_at.to_owned(),
        stale_after_seconds: DEFAULT_STALE_AFTER_SECONDS,
    })
}

/// Collapse a group display name into a concise model label, e.g.
/// "Gemini Models" -> "Gemini" and "Claude and GPT models" -> "Claude · GPT".
fn group_model_label(group: &Value) -> Option<String> {
    let name = group.get("displayName").and_then(Value::as_str)?;
    let mut name = name.trim();
    for suffix in [" Models", " models", " MODEL", " model"] {
        if let Some(stripped) = name.strip_suffix(suffix) {
            name = stripped;
            break;
        }
    }
    if name.is_empty() {
        return None;
    }
    Some(name.replace(" and ", " · "))
}

fn normalize_bucket(bucket: &Value, model_label: &Option<String>) -> Option<QuotaWindow> {
    if bucket
        .get("disabled")
        .and_then(Value::as_bool)
        .unwrap_or(false)
    {
        return None;
    }
    let remaining = bucket
        .get("remainingFraction")
        .and_then(Value::as_f64)
        .or_else(|| {
            bucket
                .get("remaining")
                .and_then(|nested| nested.get("remainingFraction"))
                .and_then(Value::as_f64)
        })?;
    let window = bucket.get("window").and_then(Value::as_str);
    Some(QuotaWindow {
        label: concise_window_label(model_label, bucket, window),
        used_percent: (1.0 - remaining) * 100.0,
        window_minutes: window.and_then(window_minutes),
        resets_at: bucket.get("resetTime").and_then(parse_reset_time),
        reset_description: bucket
            .get("description")
            .and_then(Value::as_str)
            .map(str::to_owned)
            .unwrap_or_else(|| "bucket reset".to_owned()),
        ..QuotaWindow::default()
    })
}

fn window_minutes(window: &str) -> Option<u64> {
    match window {
        "weekly" => Some(10080),
        "5h" => Some(300),
        _ => None,
    }
}

fn window_label(window: &str) -> Option<String> {
    match window {
        "weekly" => Some("Weekly".to_owned()),
        "5h" => Some("5-hour".to_owned()),
        _ => None,
    }
}

/// Concise display label like "Gemini · Weekly" or "Claude · GPT · 5-hour",
/// built from the group model label and the bucket window. Without a group
/// label the bucket's own display name or id is used.
fn concise_window_label(
    model_label: &Option<String>,
    bucket: &Value,
    window: Option<&str>,
) -> String {
    if let Some(model) = model_label.as_deref() {
        let window_part = window
            .and_then(window_label)
            .unwrap_or_else(|| "quota".to_owned());
        return format!("{model} · {window_part}");
    }
    window.and_then(window_label).unwrap_or_else(|| {
        bucket
            .get("displayName")
            .or_else(|| bucket.get("bucketId"))
            .and_then(Value::as_str)
            .unwrap_or("quota")
            .to_owned()
    })
}

/// `resetTime` arrives as an RFC 3339 string; integer epoch seconds are the
/// fallback shape.
fn parse_reset_time(value: &Value) -> Option<String> {
    if let Some(text) = value.as_str() {
        return Some(text.to_owned());
    }
    if let Some(epoch_seconds) = value.as_i64() {
        return OffsetDateTime::from_unix_timestamp(epoch_seconds)
            .ok()
            .map(super::scheduler::format_rfc3339);
    }
    None
}

/// Best-effort identity enrichment from the user-status response; the quota
/// summary itself carries no account fields.
fn identity_from_user_status(payload: &Value) -> QuotaIdentity {
    let status = payload.get("userStatus");
    let account_label = status
        .and_then(|entry| entry.get("email"))
        .and_then(Value::as_str)
        .map(str::to_owned);
    let plan = status
        .and_then(|entry| entry.get("planStatus"))
        .and_then(|plan| plan.get("planInfo"))
        .and_then(|info| info.get("planName"))
        .and_then(Value::as_str)
        .map(str::to_owned);
    QuotaIdentity {
        account_label,
        plan,
    }
}

/// Discover the local language server's loopback binding from bounded process
/// and listen-port snapshots. Windows `tasklist` carries no command line, so
/// discovery reports no binding there instead of scanning further. Every
/// candidate is probed in pid order; the first port that answers the probe
/// endpoint becomes the binding.
fn discover_loopback_binding() -> Option<LoopbackBinding> {
    if cfg!(windows) {
        return None;
    }
    let processes = capture(&["ps", "-axo", "pid=,args="])?;
    let mut candidates = processes
        .lines()
        .filter_map(parse_candidate)
        .collect::<Vec<(u32, String)>>();
    // Deterministic across runs even when several Antigravity runtimes (IDE
    // and CLI) are alive at once.
    candidates.sort_by_key(|(pid, _)| *pid);
    for (pid, csrf_token) in candidates {
        let Some(ports) = capture(&[
            "lsof",
            "-nP",
            "-iTCP",
            "-sTCP:LISTEN",
            "-a",
            "-p",
            &pid.to_string(),
        ]) else {
            continue;
        };
        let mut ports = ports
            .lines()
            .filter_map(|line| listen_port_for_pid(line, pid))
            .collect::<Vec<u16>>();
        ports.sort_unstable();
        if let Some(binding) = select_working_binding(&ports, &csrf_token, probe_loopback_endpoint)
        {
            return Some(binding);
        }
    }
    None
}

/// Probe every listen port in order and keep the binding of the first one
/// that answers with a success status.
fn select_working_binding(
    ports: &[u16],
    csrf_token: &str,
    probe: impl Fn(u16, &str) -> bool,
) -> Option<LoopbackBinding> {
    ports.iter().copied().find_map(|port| {
        if probe(port, csrf_token) {
            Some(LoopbackBinding {
                port,
                csrf_token: csrf_token.to_owned(),
                use_https: true,
            })
        } else {
            None
        }
    })
}

/// Probe the unleash-data endpoint of one port; a 2xx answer identifies the
/// working lane.
fn probe_loopback_endpoint(port: u16, csrf_token: &str) -> bool {
    let binding = LoopbackBinding {
        port,
        csrf_token: csrf_token.to_owned(),
        use_https: true,
    };
    http::post_loopback_status(
        &binding.probe_url(),
        &lane_headers(&binding),
        b"{}",
        http::LOOPBACK_FETCH_TIMEOUT,
    )
    .is_ok()
}

fn capture(args: &[&str]) -> Option<String> {
    let (program, rest) = args.split_first()?;
    let mut command = Command::new(program);
    crate::platform::user_shell_environment::apply_to_command(&mut command);
    command.args(rest);
    let result = crate::platform::run_bounded_command_output(
        &mut command,
        DISCOVERY_TIMEOUT,
        MAX_DISCOVERY_BYTES,
    )
    .ok()?;
    if result.timed_out || result.truncated || !result.status.is_some_and(|status| status.success())
    {
        return None;
    }
    Some(String::from_utf8_lossy(&result.stdout).into_owned())
}

/// Match a candidate process: the Antigravity desktop language server (needs
/// `language_server`, the CSRF token, and an Antigravity marker) or the
/// standalone `agy` CLI (the token is optional). Only the pid and the token
/// leave the parser; the full command line does not.
fn parse_candidate(line: &str) -> Option<(u32, String)> {
    let trimmed = line.trim_start();
    let split_at = trimmed.find(char::is_whitespace)?;
    let pid = trimmed[..split_at].parse::<u32>().ok()?;
    let args = trimmed[split_at..].trim();
    let token = csrf_token_from_args(args);
    let app_lane = args.contains("language_server")
        && token.is_some()
        && (args.contains("--app_data_dir antigravity") || args.contains("/antigravity/"));
    let cli_lane = binary_basename(args)
        .is_some_and(|name| matches!(name, "agy" | "antigravity-cli" | "antigravity_cli"));
    if app_lane {
        token.map(|token| (pid, token))
    } else if cli_lane {
        Some((pid, token.unwrap_or_default()))
    } else {
        None
    }
}

fn binary_basename(args: &str) -> Option<&str> {
    let binary = args.split_whitespace().next()?;
    Some(binary.rsplit('/').next().unwrap_or(binary))
}

fn csrf_token_from_args(args: &str) -> Option<String> {
    for (index, part) in args.split_whitespace().enumerate() {
        if let Some(value) = part.strip_prefix("--csrf_token=") {
            return non_empty(value);
        }
        if part == "--csrf_token" {
            return args.split_whitespace().nth(index + 1).and_then(non_empty);
        }
    }
    None
}

fn non_empty(value: &str) -> Option<String> {
    let trimmed = value.trim();
    (!trimmed.is_empty()).then(|| trimmed.to_owned())
}

fn listen_port_for_pid(line: &str, pid: u32) -> Option<u16> {
    let fields = line.split_whitespace().collect::<Vec<_>>();
    if fields.len() < 2 || fields[1].parse::<u32>().ok()? != pid {
        return None;
    }
    fields.iter().rev().find_map(|field| {
        let (host, port) = field.rsplit_once(':')?;
        if host.is_empty() {
            return None;
        }
        port.trim_end_matches(|ch: char| ch.is_ascii_alphabetic() || ch == '(' || ch == ')')
            .parse::<u16>()
            .ok()
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn synthetic_quota_summary() -> Value {
        json!({
            "response": {
                "groups": [
                    {
                        "displayName": "Gemini Models",
                        "buckets": [
                            {
                                "bucketId": "gemini-weekly",
                                "window": "weekly",
                                "remainingFraction": 0.92015475,
                                "resetTime": "2026-08-30T10:08:23Z",
                                "description": "weekly limit reset"
                            },
                            {
                                "bucketId": "gemini-session",
                                "window": "5h",
                                "remainingFraction": 0.75
                            }
                        ]
                    },
                    {
                        "displayName": "Claude and GPT models",
                        "buckets": [
                            {
                                "bucketId": "claude-weekly",
                                "window": "weekly",
                                "remainingFraction": 0.8
                            },
                            {
                                "bucketId": "claude-session",
                                "window": "5h",
                                "remainingFraction": 1.0
                            }
                        ]
                    }
                ]
            }
        })
    }

    fn synthetic_user_status() -> Value {
        json!({
            "userStatus": {
                "email": "fixture@example.invalid",
                "planStatus": {"planInfo": {"planName": "Pro"}}
            }
        })
    }

    fn captured_at() -> String {
        super::super::scheduler::format_rfc3339(OffsetDateTime::now_utc())
    }

    #[test]
    fn synthetic_real_shape_normalizes_to_all_windows() {
        let snapshot = normalize_quota_summary(&synthetic_quota_summary(), &captured_at())
            .expect("synthetic real-shape payload must normalize");
        assert_eq!(snapshot.status, QuotaStatus::Live);
        assert_eq!(snapshot.windows.len(), 4);
        let weekly = snapshot
            .windows
            .iter()
            .find(|window| window.label == "Gemini · Weekly")
            .expect("gemini weekly window");
        assert_eq!(weekly.window_minutes, Some(10080));
        assert!((weekly.used_percent - (1.0 - 0.92015475) * 100.0).abs() < 1e-9);
        assert_eq!(weekly.resets_at.as_deref(), Some("2026-08-30T10:08:23Z"));
        assert!(weekly.reset_description.contains("weekly limit"));
        let claude_5h = snapshot
            .windows
            .iter()
            .find(|window| window.label == "Claude · GPT · 5-hour")
            .expect("claude 5h window");
        assert_eq!(claude_5h.window_minutes, Some(300));
        assert_eq!(claude_5h.used_percent, 0.0);
        let gemini_5h = snapshot
            .windows
            .iter()
            .find(|window| window.label == "Gemini · 5-hour")
            .expect("gemini 5h window");
        assert_eq!(gemini_5h.window_minutes, Some(300));
        // The quota payload itself carries no account identity.
        assert_eq!(snapshot.identity, QuotaIdentity::default());
        // The wire shape must not leak any credential material.
        assert!(!snapshot.wire_value().to_string().contains("csrf"));
    }

    #[test]
    fn agy_cli_lane_discovered_without_token() {
        let (pid, token) = parse_candidate("  32928 agy --port 50529 --serve --no-csrf")
            .expect("agy CLI without a token is a candidate");
        assert_eq!(pid, 32928);
        assert_eq!(token, "");
    }

    #[test]
    fn app_lane_requires_antigravity_marker_and_token() {
        // Marker via a path segment under the Antigravity app-data directory.
        let line = "  1234 /Applications/Antigravity.app/Contents/MacOS/Antigravity \
            language_server --csrf_token=fixture-token \
            --app_data_dir /Applications/Antigravity.app/Data/antigravity/";
        let (pid, token) = parse_candidate(line).expect("app lane candidate");
        assert_eq!(pid, 1234);
        assert_eq!(token, "fixture-token");
        // Marker via the literal `--app_data_dir antigravity` argument form.
        let (pid, token) = parse_candidate(
            "  2345 /Applications/Antigravity.app/Contents/MacOS/Antigravity \
            language_server --csrf_token=tok --app_data_dir antigravity",
        )
        .expect("app lane candidate with literal app data dir");
        assert_eq!(pid, 2345);
        assert_eq!(token, "tok");
        // A language server without the Antigravity marker is not a candidate.
        assert!(parse_candidate(
            "  4321 /Applications/Other.app/Contents/MacOS/Other language_server --csrf_token=x",
        )
        .is_none());
        // A language server without a token is not an app-lane candidate.
        assert!(parse_candidate(
            "  4322 /Applications/Antigravity.app/Contents/MacOS/Antigravity language_server --app_data_dir /x/antigravity/",
        )
        .is_none());
        // Unrelated binaries are not candidates.
        assert!(parse_candidate("  9999 /usr/bin/something else").is_none());
    }

    #[test]
    fn working_port_probe_selects_first_success() {
        // 50530 refuses the probe, 50529 answers: the binding must land on
        // 50529 with an empty token and the HTTPS lane.
        let binding = select_working_binding(&[50530, 50529], "", |port, _| port == 50529)
            .expect("a working port must be selected");
        assert_eq!(binding.port, 50529);
        assert_eq!(binding.csrf_token, "");
        assert!(binding.use_https);
        // Port order is honored: the first successful probe wins.
        let binding = select_working_binding(&[50529, 50530], "tok", |port, _| port == 50529)
            .expect("a working port must be selected");
        assert_eq!(binding.port, 50529);
        assert_eq!(binding.csrf_token, "tok");
        // No answering port yields no binding.
        assert!(select_working_binding(&[50530], "", |_, _| false).is_none());
    }

    #[test]
    fn disabled_and_fractionless_buckets_are_skipped() {
        let payload = json!({
            "response": {
                "groups": [
                    {
                        "displayName": "Gemini Models",
                        "buckets": [
                            { "bucketId": "a", "window": "weekly", "remainingFraction": 0.5 },
                            { "bucketId": "b", "window": "5h", "disabled": true, "remainingFraction": 1.0 },
                            { "bucketId": "c", "window": "5h" }
                        ]
                    }
                ]
            }
        });
        let snapshot = normalize_quota_summary(&payload, &captured_at()).unwrap();
        assert_eq!(snapshot.windows.len(), 1);
        assert_eq!(snapshot.windows[0].label, "Gemini · Weekly");
    }

    #[test]
    fn missing_windows_reports_windows_missing() {
        let error =
            normalize_quota_summary(&json!({ "response": { "groups": [] } }), &captured_at())
                .expect_err("no windows must fail");
        assert_eq!(error.code, "antigravity_quota_windows_missing");
        let error = normalize_quota_summary(&json!({ "unrelated": true }), &captured_at())
            .expect_err("no groups must fail");
        assert_eq!(error.code, "antigravity_quota_windows_missing");
    }

    #[test]
    fn identity_enrichment_is_best_effort() {
        let binding = LoopbackBinding::for_testing(49152, "fixture-token", false);
        let summary = |_binding: &LoopbackBinding| Ok(synthetic_quota_summary());

        let successful = AntigravitySource::for_testing_with_identity(
            Some(binding.clone()),
            Box::new(summary),
            Box::new(|_binding| Ok(synthetic_user_status())),
        );
        let snapshot = successful
            .fetch_snapshot(OffsetDateTime::now_utc())
            .expect("snapshot stays live with identity");
        assert_eq!(
            snapshot.identity.account_label.as_deref(),
            Some("fixture@example.invalid")
        );
        assert_eq!(snapshot.identity.plan.as_deref(), Some("Pro"));

        let failing = AntigravitySource::for_testing_with_identity(
            Some(binding),
            Box::new(summary),
            Box::new(|_binding| Err(QuotaFetchError::new("quota_loopback_request_failed"))),
        );
        let snapshot = failing
            .fetch_snapshot(OffsetDateTime::now_utc())
            .expect("identity failure must not downgrade the snapshot");
        assert_eq!(snapshot.status, QuotaStatus::Live);
        assert_eq!(snapshot.identity, QuotaIdentity::default());
        assert_eq!(snapshot.windows.len(), 4);
    }

    #[test]
    fn reset_time_falls_back_to_epoch_seconds() {
        assert_eq!(
            parse_reset_time(&json!(1700000000)),
            Some("2023-11-14T22:13:20Z".to_owned())
        );
        assert_eq!(
            parse_reset_time(&json!("2026-08-30T10:08:23Z")),
            Some("2026-08-30T10:08:23Z".to_owned())
        );
        assert_eq!(parse_reset_time(&json!(true)), None);
    }
}
