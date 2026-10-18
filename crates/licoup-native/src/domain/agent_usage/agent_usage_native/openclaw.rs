use crate::domain::targets;
use crate::platform::run_bounded_command_output;
use licoup_statistics::contract::{HistoryUsageSummary, MessageUsage, UsageAccuracy, number_field};
use licoup_statistics::window::UsageWindow;
use serde_json::{Value, json};
use std::path::Path;
use std::process::Command;
use std::thread;
use std::time::{Duration, Instant};

const SOURCE: &str = "openclaw-gateway-usage-cost";
const RPC_TIMEOUT: Duration = Duration::from_secs(4);
const COMMAND_TIMEOUT: Duration = Duration::from_secs(6);
const SETTLE_TIMEOUT: Duration = Duration::from_secs(15);
const MAX_OUTPUT_BYTES: usize = 512 * 1024;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum GatewayUsageFailure {
    ExecutableUnavailable,
    QueryFailed,
    QueryTimedOut,
    OutputLimit,
    Incomplete,
    Invalid,
}

impl GatewayUsageFailure {
    const fn code(self) -> &'static str {
        match self {
            Self::ExecutableUnavailable => "openclaw_gateway_usage_executable_unavailable",
            Self::QueryFailed => "openclaw_gateway_usage_query_failed",
            Self::QueryTimedOut => "openclaw_gateway_usage_query_timeout",
            Self::OutputLimit => "openclaw_gateway_usage_output_limit",
            Self::Incomplete => "openclaw_gateway_usage_incomplete",
            Self::Invalid => "openclaw_gateway_usage_invalid",
        }
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
struct GatewayTotals {
    prompt: u64,
    cached: u64,
    completion: u64,
    total: u64,
}

pub(super) fn summarize(window: &UsageWindow, warnings: &mut Vec<Value>) -> HistoryUsageSummary {
    let result =
        targets::available_runtime_executable(&crate::target_port::agent_target_port(), "openclaw")
            .ok_or(GatewayUsageFailure::ExecutableUnavailable)
            .and_then(|executable| query_gateway_usage(&executable, window))
            .and_then(|payload| {
                parse_gateway_usage(&payload, window).ok_or(GatewayUsageFailure::Invalid)
            });
    match result {
        Ok(summary) => summary,
        Err(failure) => {
            warnings.push(json!({
                "code": failure.code(),
                "agentId": "openclaw"
            }));
            empty_summary()
        }
    }
}

fn query_gateway_usage(
    executable: &Path,
    window: &UsageWindow,
) -> Result<Value, GatewayUsageFailure> {
    let utc_offset =
        format_utc_offset(window.timezone_offset_minutes).ok_or(GatewayUsageFailure::Invalid)?;
    let params = json!({
        "startDate": window.start,
        "endDate": window.end,
        "mode": "specific",
        "utcOffset": utc_offset,
        "agentScope": "all"
    })
    .to_string();
    let deadline = Instant::now() + SETTLE_TIMEOUT;
    let mut poll = Duration::from_millis(100);
    loop {
        let payload = query_gateway_once(executable, &params)?;
        if cache_is_settled(&payload) {
            return Ok(payload);
        }
        let now = Instant::now();
        if now >= deadline {
            return Err(GatewayUsageFailure::Incomplete);
        }
        thread::sleep(poll.min(deadline - now));
        poll = (poll * 2).min(Duration::from_millis(800));
    }
}

fn query_gateway_once(executable: &Path, params: &str) -> Result<Value, GatewayUsageFailure> {
    let mut command = Command::new(executable);
    crate::platform::user_shell_environment::apply_to_command(&mut command);
    command
        .arg("gateway")
        .arg("call")
        .arg("usage.cost")
        .arg("--params")
        .arg(params)
        .arg("--timeout")
        .arg(RPC_TIMEOUT.as_millis().to_string())
        .arg("--json");
    let output = run_bounded_command_output(&mut command, COMMAND_TIMEOUT, MAX_OUTPUT_BYTES)
        .map_err(|_| GatewayUsageFailure::QueryFailed)?;
    if output.timed_out {
        return Err(GatewayUsageFailure::QueryTimedOut);
    }
    if output.truncated {
        return Err(GatewayUsageFailure::OutputLimit);
    }
    if !output.status.is_some_and(|status| status.success()) {
        return Err(GatewayUsageFailure::QueryFailed);
    }
    serde_json::from_slice(&output.stdout).map_err(|_| GatewayUsageFailure::Invalid)
}

fn cache_is_settled(payload: &Value) -> bool {
    payload
        .pointer("/cacheStatus/status")
        .and_then(Value::as_str)
        .is_none_or(|status| status == "fresh")
}

fn parse_gateway_usage(payload: &Value, window: &UsageWindow) -> Option<HistoryUsageSummary> {
    let expected = gateway_totals(payload.get("totals")?)?;
    let daily = payload.get("daily")?.as_array()?;
    let mut summary = empty_summary();
    for entry in daily {
        let day = entry.get("date")?.as_str()?.trim();
        if day.is_empty() || !window.contains(day) {
            return None;
        }
        let totals = gateway_totals(entry)?;
        if totals.total == 0 {
            continue;
        }
        summary.add(
            MessageUsage {
                prompt_tokens: totals.prompt,
                cached_input_tokens: totals.cached,
                completion_tokens: totals.completion,
                total_tokens: totals.total,
                model: None,
                variant: Default::default(),
                accuracy: UsageAccuracy::Exact,
            },
            Some(day.to_owned()),
        );
    }
    if summary.prompt_tokens() != expected.prompt
        || summary.explicit_cached_input_tokens != expected.cached
        || summary.completion_tokens() != expected.completion
        || summary.total_tokens() != expected.total
    {
        return None;
    }
    summary.message_count = summary.explicit_records;
    Some(summary)
}

fn gateway_totals(value: &Value) -> Option<GatewayTotals> {
    let input = number_field(value, &["input"])?;
    let output = number_field(value, &["output"])?;
    let cache_read = number_field(value, &["cacheRead"])?;
    let cache_write = number_field(value, &["cacheWrite"])?;
    let total = number_field(value, &["totalTokens"])?;
    Some(GatewayTotals {
        prompt: input.saturating_add(cache_read).saturating_add(cache_write),
        cached: cache_read,
        completion: output,
        total,
    })
}

fn format_utc_offset(minutes: i64) -> Option<String> {
    if !(-12 * 60..=14 * 60).contains(&minutes) {
        return None;
    }
    let sign = if minutes < 0 { '-' } else { '+' };
    let absolute = minutes.unsigned_abs();
    Some(format!(
        "UTC{sign}{:02}:{:02}",
        absolute / 60,
        absolute % 60
    ))
}

fn empty_summary() -> HistoryUsageSummary {
    HistoryUsageSummary {
        source: Some(SOURCE),
        ..HistoryUsageSummary::default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn window() -> UsageWindow {
        UsageWindow::from_params(&json!({
            "now": "2026-07-15T12:00:00Z",
            "historyDays": 2,
            "timezoneOffsetMinutes": 480
        }))
    }

    #[test]
    fn official_gateway_daily_totals_are_the_only_openclaw_consumption_source() {
        let summary = parse_gateway_usage(
            &json!({
                "updatedAt": 1,
                "days": 2,
                "daily": [
                    {
                        "date": "2026-07-14",
                        "input": 80,
                        "output": 15,
                        "cacheRead": 20,
                        "cacheWrite": 5,
                        "totalTokens": 120,
                        "totalCost": 0
                    },
                    {
                        "date": "2026-07-15",
                        "input": 30,
                        "output": 6,
                        "cacheRead": 10,
                        "cacheWrite": 2,
                        "totalTokens": 48,
                        "totalCost": 0
                    }
                ],
                "totals": {
                    "input": 110,
                    "output": 21,
                    "cacheRead": 30,
                    "cacheWrite": 7,
                    "totalTokens": 168,
                    "totalCost": 0
                },
                "cacheStatus": {"status": "fresh"}
            }),
            &window(),
        )
        .unwrap();

        assert_eq!(summary.prompt_tokens(), 147);
        assert_eq!(summary.explicit_cached_input_tokens, 30);
        assert_eq!(summary.completion_tokens(), 21);
        assert_eq!(summary.total_tokens(), 168);
        assert_eq!(summary.explicit_records, 2);
        assert_eq!(summary.estimated_records, 0);
        assert_eq!(summary.source, Some(SOURCE));
    }

    #[test]
    fn incomplete_or_inconsistent_gateway_results_are_not_presented_as_exact() {
        assert!(!cache_is_settled(
            &json!({"cacheStatus": {"status": "refreshing"}})
        ));
        assert!(
            parse_gateway_usage(
                &json!({
                    "daily": [{
                        "date": "2026-07-15",
                        "input": 10,
                        "output": 5,
                        "cacheRead": 0,
                        "cacheWrite": 0,
                        "totalTokens": 15
                    }],
                    "totals": {
                        "input": 11,
                        "output": 5,
                        "cacheRead": 0,
                        "cacheWrite": 0,
                        "totalTokens": 16
                    }
                }),
                &window()
            )
            .is_none()
        );
        assert_eq!(format_utc_offset(480).as_deref(), Some("UTC+08:00"));
        assert_eq!(format_utc_offset(-330).as_deref(), Some("UTC-05:30"));
        assert_eq!(format_utc_offset(14 * 60).as_deref(), Some("UTC+14:00"));
        assert_eq!(format_utc_offset(14 * 60 + 1), None);
    }
}
