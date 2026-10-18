//! Kimi Code quota source.
//!
//! Approach (reimplemented from CodexBar's documented behavior, no code
//! copied): read the Kimi Code CLI credential file (`credentials/
//! kimi-code.json` under the Kimi Code home directory, resolved through the
//! `kimiCodeHome` param, the `KIMI_CODE_HOME` environment variable, or
//! home-dir APIs, never a literal absolute path), take the short-lived
//! access token with a 60-second validity margin, then call the hosted
//! coding usages endpoint. The refresh token is never read or used; the CLI
//! itself renews the access token when the user runs it.

use super::contract::{
    DEFAULT_STALE_AFTER_SECONDS, ProviderQuotaSnapshot, QuotaFetchError, QuotaIdentity,
    QuotaProvider, QuotaStatus, QuotaWindow,
};
use super::credentials;
use super::http;
use serde_json::Value;
use std::path::PathBuf;
use time::OffsetDateTime;

const USAGES_URL: &str = "https://api.kimi.com/coding/v1/usages";
/// Device identity headers accompany the bearer token, matching the CLI's
/// own requests.
const PLATFORM_HEADER: (&str, &str) = ("X-Msh-Platform", "kimi_code_cli");
/// Tokens with at most this much remaining validity are treated as expired so
/// an in-flight fetch never carries a token that dies mid-request.
const MIN_TOKEN_VALIDITY_SECONDS: i64 = 60;
/// The weekly member quota window.
const WEEKLY_WINDOW_MINUTES: u64 = 7 * 24 * 60;

type HostedFetch = dyn Fn(&str, &[(&str, &str)]) -> Result<Value, QuotaFetchError> + Send + Sync;

pub(super) struct KimiCodeSource {
    credentials_path: Option<PathBuf>,
    device_id_path: Option<PathBuf>,
    fetch_usages: Box<HostedFetch>,
}

impl KimiCodeSource {
    pub(super) fn production(params: &Value) -> Self {
        let home = resolve_kimi_code_home(params);
        Self {
            credentials_path: home
                .as_ref()
                .map(|root| root.join("credentials").join("kimi-code.json")),
            device_id_path: home.as_ref().map(|root| root.join("device_id")),
            fetch_usages: Box::new(|url, headers| {
                http::get_json_with_headers(url, headers, http::HOSTED_FETCH_TIMEOUT)
            }),
        }
    }

    #[cfg(test)]
    pub(super) fn for_testing(
        credentials_path: Option<PathBuf>,
        device_id_path: Option<PathBuf>,
        fetch_usages: Box<HostedFetch>,
    ) -> Self {
        Self {
            credentials_path,
            device_id_path,
            fetch_usages,
        }
    }

    pub(super) fn fetch_snapshot(
        &self,
        now: OffsetDateTime,
    ) -> Result<ProviderQuotaSnapshot, QuotaFetchError> {
        let captured_at = super::scheduler::format_rfc3339(now);
        let credentials_path = self
            .credentials_path
            .clone()
            .ok_or_else(|| QuotaFetchError::new("kimi_code_home_unavailable"))?;
        let document = credentials::read_bounded_utf8(&credentials_path)
            .ok()
            .and_then(|text| serde_json::from_str::<Value>(&text).ok())
            .ok_or_else(|| QuotaFetchError::new("kimi_code_credentials_unreadable"))?;
        let token = document
            .get("access_token")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_owned)
            .ok_or_else(|| QuotaFetchError::new("kimi_code_access_token_absent"))?;

        // The token is dropped after the freshness check and the request
        // build; it never enters the snapshot, retained state, logs, or
        // diagnostics. The refresh token in the same file is never read.
        let expires_at = credentials::jwt_expiry_epoch_seconds(&token)
            .or_else(|| expires_at_epoch_seconds(&document));
        if expires_at
            .is_some_and(|expiry| expiry <= now.unix_timestamp() + MIN_TOKEN_VALIDITY_SECONDS)
        {
            return Err(QuotaFetchError::new("kimi_code_token_expired"));
        }

        let device_id = self
            .device_id_path
            .as_ref()
            .and_then(|path| credentials::read_bounded_utf8(path).ok())
            .map(|value| value.trim().to_owned())
            .filter(|value| !value.is_empty());
        let bearer = format!("Bearer {token}");
        let device_header = device_id.map(|id| ("X-Msh-Device-Id", id));
        let mut headers: Vec<(&str, &str)> =
            vec![("Authorization", bearer.as_str()), PLATFORM_HEADER];
        if let Some((name, value)) = device_header.as_ref() {
            headers.push((name, value.as_str()));
        }

        let payload = (self.fetch_usages)(USAGES_URL, &headers)?;
        let account_label = credentials::jwt_string_claim(&token, "sub");
        normalize_usages(&payload, &captured_at, account_label)
    }
}

/// `expires_at` from the credential file as epoch seconds; the CLI writes it
/// as a number, but a numeric string is accepted as well.
fn expires_at_epoch_seconds(document: &Value) -> Option<i64> {
    let value = document.get("expires_at")?;
    value
        .as_i64()
        .or_else(|| value.as_f64().map(|number| number as i64))
        .or_else(|| value.as_str()?.trim().parse::<i64>().ok())
}

fn resolve_kimi_code_home(params: &Value) -> Option<PathBuf> {
    if let Some(path) =
        crate::domain::conversation::parameters::text_param(params, &["kimiCodeHome"])
    {
        return Some(PathBuf::from(path));
    }
    if let Some(path) = std::env::var_os("KIMI_CODE_HOME").filter(|value| !value.is_empty()) {
        return Some(PathBuf::from(path));
    }
    licoup_foundation::platform::paths::user_home_from_env().map(|home| {
        licoup_foundation::platform::paths::strip_macos_data_volume(&home).join(".kimi-code")
    })
}

/// Numeric quota counters arrive as numbers from the live endpoint, but the
/// documented contract also allows numeric strings; accept both.
fn numeric_field(value: &Value, field: &str) -> Option<f64> {
    value
        .get(field)?
        .as_f64()
        .or_else(|| value.get(field)?.as_str()?.trim().parse::<f64>().ok())
}

/// One quota counter block (`usage` or `limits[].detail`) normalized into a
/// window. Returns None when the block cannot yield a usable percentage.
fn normalize_counter_window(
    block: &Value,
    label: &str,
    window_minutes: Option<u64>,
    reset_description: String,
) -> Option<QuotaWindow> {
    let limit = numeric_field(block, "limit").filter(|limit| *limit > 0.0)?;
    let used = numeric_field(block, "used").or_else(|| {
        numeric_field(block, "remaining").map(|remaining| (limit - remaining).max(0.0))
    })?;
    Some(QuotaWindow {
        label: label.to_owned(),
        // Raw provider value; may exceed 100. The UI clamps for display.
        used_percent: used / limit * 100.0,
        window_minutes,
        resets_at: block
            .get("resetTime")
            .and_then(Value::as_str)
            .map(str::to_owned),
        reset_description,
        ..QuotaWindow::default()
    })
}

/// Usages payload: the weekly member quota (`usage`) plus the sliding rate
/// limit (`limits[0]`). Blocks without a usable counter are skipped; a
/// payload with no usable window fails the fetch.
fn normalize_usages(
    payload: &Value,
    captured_at: &str,
    account_label: Option<String>,
) -> Result<ProviderQuotaSnapshot, QuotaFetchError> {
    let mut windows = Vec::new();
    if let Some(usage) = payload.get("usage") {
        if let Some(window) = normalize_counter_window(
            usage,
            "weekly",
            Some(WEEKLY_WINDOW_MINUTES),
            "weekly member quota".to_owned(),
        ) {
            windows.push(window);
        }
    }
    if let Some(first_limit) = payload
        .get("limits")
        .and_then(Value::as_array)
        .and_then(|limits| limits.first())
    {
        let window_minutes = first_limit.get("window").and_then(rate_window_minutes);
        if let Some(detail) = first_limit.get("detail") {
            if let Some(window) = normalize_counter_window(
                detail,
                "session",
                window_minutes,
                "rate limit window".to_owned(),
            ) {
                windows.push(window);
            }
        }
    }
    if windows.is_empty() {
        return Err(QuotaFetchError::new("kimi_code_quota_windows_missing"));
    }
    let identity = QuotaIdentity {
        account_label,
        plan: payload
            .pointer("/user/membership/level")
            .and_then(Value::as_str)
            .map(membership_level_label),
    };
    Ok(ProviderQuotaSnapshot {
        agent_id: QuotaProvider::KimiCode.agent_id().to_owned(),
        provider: QuotaProvider::KimiCode,
        status: QuotaStatus::Live,
        windows,
        identity,
        captured_at: captured_at.to_owned(),
        stale_after_seconds: DEFAULT_STALE_AFTER_SECONDS,
    })
}

/// Rate-limit window length in minutes from `{duration, timeUnit}`; the time
/// unit arrives as the enum name (`TIME_UNIT_MINUTE` and friends) or its
/// numeric value.
fn rate_window_minutes(window: &Value) -> Option<u64> {
    let duration = numeric_field(window, "duration").filter(|duration| *duration > 0.0)?;
    let multiplier = match window.get("timeUnit") {
        Some(Value::String(unit)) => match unit.as_str() {
            "TIME_UNIT_MINUTE" => 1.0,
            "TIME_UNIT_HOUR" => 60.0,
            "TIME_UNIT_DAY" => 1440.0,
            _ => return None,
        },
        Some(value) => match value.as_i64() {
            Some(1) => 1.0,
            Some(60) => 60.0,
            Some(1440) => 1440.0,
            _ => return None,
        },
        None => return None,
    };
    let minutes = duration * multiplier;
    (minutes.is_finite() && minutes > 0.0).then_some(minutes as u64)
}

/// `LEVEL_*` membership enum names read better on the card without the
/// prefix and in title case.
fn membership_level_label(level: &str) -> String {
    let trimmed = level.strip_prefix("LEVEL_").unwrap_or(level);
    let lowered = trimmed.to_lowercase();
    let mut chars = lowered.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
        None => lowered,
    }
}
