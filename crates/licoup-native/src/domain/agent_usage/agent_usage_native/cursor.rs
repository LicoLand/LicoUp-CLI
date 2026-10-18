//! Hosted Cursor usage ledger.
//!
//! Cursor's local stores are not a billing ledger: the IDE still writes
//! `tokenCount` on bubbles, but those counters are unused, Composer
//! `usageData` is empty, and `promptTokenBreakdown` / `contextTokensUsed` /
//! `contextUsagePercent` measure context-window occupancy rather than billed
//! consumption. Monitoring therefore reads the account's own dashboard ledger
//! (`POST cursor.com/api/dashboard/get-filtered-usage-events`, the endpoint
//! Cursor's usage page itself queries) through the same in-memory session the
//! quota ring uses.
//!
//! Aggregation keeps only safe per-day, per-model aggregates: token counts,
//! request units, and how many of those requests carried no token fields.
//! Conversation identifiers, owning users, subscription identifiers, cookies,
//! and JWTs are read in memory where required and never leave this module —
//! and never reach the retained report.
//!
//! Pagination is authoritative-count reconciled: a page that repeats rows at
//! its boundary is trimmed only by the exact number Cursor's own
//! `totalUsageEventsCount` proves, and any window that cannot be proven
//! complete fails closed instead of publishing a partial total.

use crate::domain::conversation::parameters::param_bool;
use crate::domain::provider_quota::{resolve_session, state_db_path};
use licoup_statistics::contract::{
    DailyUsageSummary, HistoryUsageSummary, MessageUsage, UsageAccuracy, number_field,
};
use licoup_statistics::persistence::read_retained_reports;
use licoup_statistics::window::UsageWindow;
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::io::Read;
use std::time::{Duration, Instant};
use time::Date;

pub(super) const SOURCE: &str = "cursor-hosted-usage-events";

const EVENTS_URL: &str = "https://cursor.com/api/dashboard/get-filtered-usage-events";
/// Cursor's dashboard POST endpoints enforce CSRF, so the request must carry
/// the exact Origin of the site it belongs to.
const ORIGIN: &str = "https://cursor.com";
const PAGE_SIZE: usize = 1000;
/// Hard page cap (40k events per scan). Reaching it without a proven final
/// page fails the scan closed rather than publishing a truncated window.
const MAX_PAGES: usize = 40;
const MAX_EVENTS: usize = MAX_PAGES * PAGE_SIZE;
const MAX_PAGE_BYTES: u64 = 4 * 1024 * 1024;
const FETCH_TIMEOUT: Duration = Duration::from_secs(20);
/// Whole-window budget so a stalled endpoint cannot hold the scan open. A full
/// window normally resolves in a handful of fast pages.
const FETCH_BUDGET: Duration = Duration::from_secs(90);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum HostedUsageFailure {
    Session(&'static str),
    RequestFailed,
    Unauthorized,
    ResponseInvalid,
    ResponseTooLarge,
    PaginationIncomplete,
    TimedOut,
}

impl HostedUsageFailure {
    const fn code(self) -> &'static str {
        match self {
            Self::Session(code) => code,
            Self::RequestFailed => "cursor_usage_request_failed",
            Self::Unauthorized => "cursor_usage_unauthorized",
            Self::ResponseInvalid => "cursor_usage_response_invalid",
            Self::ResponseTooLarge => "cursor_usage_response_too_large",
            Self::PaginationIncomplete => "cursor_usage_pagination_incomplete",
            Self::TimedOut => "cursor_usage_timeout",
        }
    }
}

pub(super) fn summarize(
    scan_params: &Value,
    window: &UsageWindow,
    warnings: &mut Vec<Value>,
) -> HistoryUsageSummary {
    let force_refresh = param_bool(scan_params, "forceRefresh").unwrap_or(false);
    let baseline = if force_refresh {
        HostedCoverage::default()
    } else {
        load_coverage(scan_params, window)
    };
    let fetch_start = baseline.next_fetch_day(window);
    match fetch_and_aggregate(scan_params, window, &fetch_start) {
        Ok(summary) => merge_baseline(summary, &baseline, window, &fetch_start),
        Err(failure) => {
            warnings.push(json!({
                "code": failure.code(),
                "agentId": "cursor"
            }));
            // A failed fetch publishes nothing for the window. Previously
            // published days could belong to a different Cursor account, so
            // they are never carried forward without a verified session.
            empty_summary()
        }
    }
}

fn fetch_and_aggregate(
    scan_params: &Value,
    window: &UsageWindow,
    fetch_start: &str,
) -> Result<HistoryUsageSummary, HostedUsageFailure> {
    let start_ms = window
        .local_day_start_millis(fetch_start)
        .ok_or(HostedUsageFailure::ResponseInvalid)?;
    let end_ms = window
        .now
        .unix_timestamp()
        .saturating_mul(1000)
        .max(start_ms);
    let session = resolve_session(state_db_path(scan_params), window.now)
        .map_err(|error| HostedUsageFailure::Session(error.code()))?;
    let events = fetch_all_events(session.cookie(), start_ms, end_ms)?;
    Ok(aggregate_events(&events, window))
}

fn empty_summary() -> HistoryUsageSummary {
    HistoryUsageSummary {
        source: Some(SOURCE),
        ..HistoryUsageSummary::default()
    }
}

fn aggregate_events(events: &[Value], window: &UsageWindow) -> HistoryUsageSummary {
    let mut summary = empty_summary();
    let mut requests = 0_u64;
    for event in events {
        let Some(timestamp) = number_field(event, &["timestamp"]).filter(|value| *value > 0) else {
            continue;
        };
        let Some(day) = window.date_key(&timestamp.to_string()) else {
            continue;
        };
        if !window.contains(&day) {
            continue;
        }
        requests = requests.saturating_add(1);
        let model = licoup_statistics::variant::model_label(event);
        match event_token_usage(event) {
            Some(usage) => {
                let message = MessageUsage {
                    prompt_tokens: usage.prompt_tokens,
                    cached_input_tokens: usage.cached_input_tokens,
                    completion_tokens: usage.completion_tokens,
                    total_tokens: usage.total_tokens,
                    variant: licoup_statistics::variant::UsageVariant::from_metadata(event),
                    model,
                    accuracy: UsageAccuracy::Exact,
                };
                summary.add(message, Some(day));
            }
            // A present request whose payload omits token fields stays a
            // request: no character or context estimate is ever substituted.
            None => summary.add_token_unavailable_request_with_variant(
                Some(day),
                model,
                licoup_statistics::variant::UsageVariant::from_metadata(event),
            ),
        }
    }
    summary.message_count = requests;
    summary
}

struct EventTokens {
    prompt_tokens: u64,
    cached_input_tokens: u64,
    completion_tokens: u64,
    total_tokens: u64,
}

/// Cursor's token counters are disjoint: `inputTokens` excludes cached reads
/// and cache writes, so the prompt total folds all three together while the
/// cached counter stays a subset of it. An event that is not token based, or
/// whose counters are all zero, reports no token usage at all.
fn event_token_usage(event: &Value) -> Option<EventTokens> {
    if event.get("isTokenBasedCall").and_then(Value::as_bool) == Some(false) {
        return None;
    }
    let usage = event.get("tokenUsage")?;
    let input = number_field(usage, &["inputTokens"]).unwrap_or(0);
    let output = number_field(usage, &["outputTokens"]).unwrap_or(0);
    let cache_read = number_field(usage, &["cacheReadTokens"]).unwrap_or(0);
    let cache_write = number_field(usage, &["cacheWriteTokens"]).unwrap_or(0);
    let prompt = input.saturating_add(cache_read).saturating_add(cache_write);
    let completion = output;
    let total = prompt.saturating_add(completion);
    if total == 0 {
        return None;
    }
    Some(EventTokens {
        prompt_tokens: prompt,
        cached_input_tokens: cache_read.min(prompt),
        completion_tokens: completion,
        total_tokens: total,
    })
}

fn fetch_all_events(
    cookie: &str,
    start_ms: i64,
    end_ms: i64,
) -> Result<Vec<Value>, HostedUsageFailure> {
    let mut pages = Vec::new();
    let mut expected_total = None;
    let mut completed = false;
    let deadline = Instant::now() + FETCH_BUDGET;
    for page in 1..=MAX_PAGES {
        if Instant::now() >= deadline {
            return Err(HostedUsageFailure::TimedOut);
        }
        let payload = post_events_page(cookie, page, start_ms, end_ms)?;
        let parsed = parse_events_page(&payload)?;
        if let Some(total) = parsed.total {
            if expected_total.is_some_and(|expected| expected != total) {
                return Err(HostedUsageFailure::PaginationIncomplete);
            }
            expected_total = Some(total);
        }
        if parsed.events.len() < PAGE_SIZE {
            // A short or empty page proves the window is fully read.
            completed = true;
            if !parsed.events.is_empty() {
                pages.push(parsed.events);
            }
            break;
        }
        pages.push(parsed.events);
    }
    if !completed {
        return Err(HostedUsageFailure::PaginationIncomplete);
    }
    reconcile_pages(pages, expected_total)
}

fn post_events_page(
    cookie: &str,
    page: usize,
    start_ms: i64,
    end_ms: i64,
) -> Result<Value, HostedUsageFailure> {
    if !licoup_foundation::platform::url_security::is_https_or_loopback_http_url(EVENTS_URL) {
        return Err(HostedUsageFailure::ResponseInvalid);
    }
    let body = json!({
        "page": page,
        "pageSize": PAGE_SIZE,
        "startDate": start_ms.to_string(),
        "endDate": end_ms.to_string(),
    })
    .to_string();
    let response = ureq::AgentBuilder::new()
        .timeout_connect(Duration::from_secs(2))
        .build()
        .post(EVENTS_URL)
        .timeout(FETCH_TIMEOUT)
        .set("Content-Type", "application/json")
        .set("Accept", "application/json")
        .set("Cookie", cookie)
        .set("Origin", ORIGIN)
        .send_string(&body);
    match response {
        Ok(response) => decode_bounded_json(response),
        Err(ureq::Error::Status(401 | 403, _)) => Err(HostedUsageFailure::Unauthorized),
        Err(_) => Err(HostedUsageFailure::RequestFailed),
    }
}

fn decode_bounded_json(response: ureq::Response) -> Result<Value, HostedUsageFailure> {
    let mut bytes = Vec::new();
    response
        .into_reader()
        .take(MAX_PAGE_BYTES.saturating_add(1))
        .read_to_end(&mut bytes)
        .map_err(|_| HostedUsageFailure::ResponseInvalid)?;
    if bytes.len() as u64 > MAX_PAGE_BYTES {
        return Err(HostedUsageFailure::ResponseTooLarge);
    }
    serde_json::from_slice(&bytes).map_err(|_| HostedUsageFailure::ResponseInvalid)
}

struct EventsPage {
    total: Option<usize>,
    events: Vec<Value>,
}

/// One page of the dashboard response. An empty query window answers `{}`;
/// an empty terminal page may keep only the query's total count. Any other
/// shape — including an error envelope — is rejected so it can never read as
/// confirmed-empty usage.
fn parse_events_page(payload: &Value) -> Result<EventsPage, HostedUsageFailure> {
    let object = payload
        .as_object()
        .ok_or(HostedUsageFailure::ResponseInvalid)?;
    let total = match object.get("totalUsageEventsCount") {
        None => None,
        Some(value) => {
            let total = value
                .as_u64()
                .and_then(|value| usize::try_from(value).ok())
                .ok_or(HostedUsageFailure::ResponseInvalid)?;
            Some(total)
        }
    };
    if object.is_empty() {
        return Ok(EventsPage {
            total: Some(0),
            events: Vec::new(),
        });
    }
    match object.get("usageEventsDisplay") {
        Some(value) => {
            let events = value
                .as_array()
                .ok_or(HostedUsageFailure::ResponseInvalid)?
                .clone();
            Ok(EventsPage { total, events })
        }
        None if total.is_some() && object.len() == 1 => Ok(EventsPage {
            total,
            events: Vec::new(),
        }),
        None => Err(HostedUsageFailure::ResponseInvalid),
    }
}

/// Cursor exposes no stable event id, so page-boundary repeats are removed
/// only at adjacent boundaries and only as many as the authoritative total
/// proves. Anything that cannot be reconciled exactly fails closed.
fn reconcile_pages(
    pages: Vec<Vec<Value>>,
    expected_total: Option<usize>,
) -> Result<Vec<Value>, HostedUsageFailure> {
    let raw_count = pages.iter().map(Vec::len).sum::<usize>();
    if raw_count > MAX_EVENTS {
        return Err(HostedUsageFailure::PaginationIncomplete);
    }
    let Some(expected_total) = expected_total else {
        // A single page without an authoritative count is unambiguous; several
        // pages cannot be reconciled without it, so they fail closed.
        if pages.len() > 1 {
            return Err(HostedUsageFailure::PaginationIncomplete);
        }
        return Ok(pages.into_iter().flatten().collect());
    };
    if raw_count < expected_total {
        return Err(HostedUsageFailure::PaginationIncomplete);
    }
    if raw_count == expected_total {
        return Ok(pages.into_iter().flatten().collect());
    }
    let mut removals = raw_count - expected_total;
    let mut reconciled = pages.first().cloned().unwrap_or_default();
    for index in 1..pages.len() {
        let overlap = boundary_overlap(&pages[index - 1], &pages[index]);
        let remove = overlap.min(removals);
        reconciled.extend_from_slice(&pages[index][remove..]);
        removals -= remove;
    }
    if removals != 0 || reconciled.len() != expected_total {
        return Err(HostedUsageFailure::PaginationIncomplete);
    }
    Ok(reconciled)
}

fn boundary_overlap(previous: &[Value], current: &[Value]) -> usize {
    let limit = previous.len().min(current.len());
    for count in (1..=limit).rev() {
        if previous[previous.len() - count..] == current[..count] {
            return count;
        }
    }
    0
}

/// Local days already published by an earlier hosted scan. A day is reusable
/// only while the previous report's coverage interval contains it and it
/// ended before the report was generated, so a partial day is never frozen.
#[derive(Clone, Debug, Default)]
struct HostedCoverage {
    coverage_start: Option<String>,
    sealed_through: Option<String>,
    days: BTreeMap<String, DailyUsageSummary>,
}

impl HostedCoverage {
    fn covers(&self, day: &str) -> bool {
        match (
            self.coverage_start.as_deref(),
            self.sealed_through.as_deref(),
        ) {
            (Some(start), Some(end)) => day >= start && day <= end,
            _ => false,
        }
    }

    /// First window day the previous report does not already cover. Days the
    /// previous scan already queried are not queried again; the current local
    /// day is always refetched because it is still accumulating.
    fn next_fetch_day(&self, window: &UsageWindow) -> String {
        for day in window_days(window) {
            if !self.covers(&day) {
                return day;
            }
        }
        window.end.clone()
    }
}

fn window_days(window: &UsageWindow) -> Vec<String> {
    let Some(start) = parse_day(&window.start) else {
        return vec![window.start.clone()];
    };
    let Some(end) = parse_day(&window.end) else {
        return vec![window.start.clone()];
    };
    let mut days = Vec::new();
    let mut current = start;
    while current <= end && days.len() < 128 {
        days.push(format_day(current));
        let Some(next) = current.next_day() else {
            break;
        };
        current = next;
    }
    days
}

fn load_coverage(scan_params: &Value, window: &UsageWindow) -> HostedCoverage {
    let Ok(reports) = read_retained_reports(
        scan_params,
        Some("cursor"),
        1,
        &crate::domain::model_registry::refresh_cached_snapshot_for_state_root(
            licoup_statistics::contract::text_field(scan_params, &["stateRoot"])
                .as_deref()
                .map(std::path::Path::new),
        ),
    ) else {
        return HostedCoverage::default();
    };
    reports
        .first()
        .and_then(|report| parse_coverage(report, window))
        .unwrap_or_default()
}

fn parse_coverage(report: &Value, window: &UsageWindow) -> Option<HostedCoverage> {
    if report.get("usageParserRevision").and_then(Value::as_str)
        != Some(licoup_statistics::contract::USAGE_PARSER_REVISION)
    {
        return None;
    }
    let report_day = report
        .get("generatedAt")
        .and_then(Value::as_str)
        .and_then(|value| window.date_key(value))?;
    let history = report
        .get("agents")?
        .as_array()?
        .iter()
        .find(|agent| agent.get("agentId").and_then(Value::as_str) == Some("cursor"))?
        .get("history")?;
    let coverage_start = history
        .pointer("/scanCache/hostedCoverageStart")
        .and_then(Value::as_str)
        .and_then(parse_day)
        .map(format_day)?;
    let sealed_through = parse_day(&report_day)?.previous_day().map(format_day)?;
    let mut days = BTreeMap::new();
    for entry in history.get("dailyUsage")?.as_array()? {
        if let Some((day, usage)) = parse_retained_day(entry) {
            days.insert(day, usage);
        }
    }
    Some(HostedCoverage {
        coverage_start: Some(coverage_start),
        sealed_through: Some(sealed_through),
        days,
    })
}

/// Rebuild one published day from the retained report. Only the numeric
/// aggregate fields exist here, so nothing private can be carried forward.
fn parse_retained_day(entry: &Value) -> Option<(String, DailyUsageSummary)> {
    let day = entry
        .get("date")
        .and_then(Value::as_str)
        .and_then(parse_day)
        .map(format_day)?;
    let mut usage = DailyUsageSummary {
        prompt_tokens: number_field(entry, &["promptTokens"]).unwrap_or(0),
        cached_input_tokens: number_field(entry, &["cachedInputTokens"]).unwrap_or(0),
        completion_tokens: number_field(entry, &["completionTokens"]).unwrap_or(0),
        total_tokens: number_field(entry, &["totalTokens"]).unwrap_or(0),
        message_count: number_field(entry, &["messageCount"]).unwrap_or(0),
        explicit_records: number_field(entry, &["explicitRecords"]).unwrap_or(0),
        estimated_records: 0,
        estimated_prompt_tokens: 0,
        estimated_completion_tokens: 0,
        request_count: number_field(entry, &["requestCount"]).unwrap_or(0),
        token_unavailable_requests: number_field(entry, &["tokenUnavailableRequests"]).unwrap_or(0),
        model_usage: BTreeMap::new(),
        model_variants: BTreeMap::new(),
    };
    if usage.total_tokens == 0 && usage.request_count == 0 {
        return None;
    }
    for ((model, variant), totals) in licoup_statistics::model_identity::raw_model_usage(entry) {
        usage.add_model_variant_totals(model, variant, totals);
    }
    Some((day, usage))
}

/// Adopt the previously published days and record the coverage interval this
/// scan now owns. Fetched days always win: coverage never overlaps them.
fn merge_baseline(
    mut summary: HistoryUsageSummary,
    baseline: &HostedCoverage,
    window: &UsageWindow,
    fetch_start: &str,
) -> HistoryUsageSummary {
    let mut coverage_start = fetch_start.to_owned();
    let mut reused_days = 0_usize;
    for (day, usage) in &baseline.days {
        if day.as_str() < window.start.as_str() || day.as_str() > window.end.as_str() {
            continue;
        }
        if !baseline.covers(day) || day.as_str() >= fetch_start {
            continue;
        }
        coverage_start = coverage_start.min(day.clone());
        reused_days = reused_days.saturating_add(1);
        summary.adopt_daily_summary(day.clone(), usage.clone());
    }
    summary.session_count = 0;
    summary.scan_cache = Some(json!({
        "parserRevision": super::PARSER_REVISION,
        "fresh": false,
        "hosted": true,
        "hostedCoverageStart": coverage_start,
        "reusedDays": reused_days,
    }));
    summary
}

fn parse_day(value: &str) -> Option<Date> {
    let value = value.trim();
    if value.len() != 10 {
        return None;
    }
    let year = value.get(0..4)?.parse().ok()?;
    let month = time::Month::try_from(value.get(5..7)?.parse::<u8>().ok()?).ok()?;
    let day = value.get(8..10)?.parse().ok()?;
    Date::from_calendar_date(year, month, day).ok()
}

fn format_day(date: Date) -> String {
    format!(
        "{:04}-{:02}-{:02}",
        date.year(),
        u8::from(date.month()),
        date.day()
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn window() -> UsageWindow {
        UsageWindow::from_params(&json!({
            "now": "2026-07-15T12:00:00Z",
            "historyDays": 3,
            "timezoneOffsetMinutes": 480
        }))
    }

    fn event(timestamp_ms: i64, model: &str, tokens: Option<(u64, u64, u64)>) -> Value {
        match tokens {
            Some((input, output, cache_read)) => json!({
                "timestamp": timestamp_ms.to_string(),
                "model": model,
                "kind": "USAGE_EVENT_KIND_INCLUDED_IN_ULTRA",
                "isTokenBasedCall": true,
                "isChargeable": true,
                "tokenUsage": {
                    "inputTokens": input,
                    "outputTokens": output,
                    "cacheReadTokens": cache_read,
                    "totalCents": 13.4
                },
                "owningUser": "private-user",
                "conversationId": "00000000-0000-0000-0000-000000000000"
            }),
            None => json!({
                "timestamp": timestamp_ms.to_string(),
                "model": model,
                "kind": "USAGE_EVENT_KIND_INCLUDED_IN_ULTRA",
                "isTokenBasedCall": false,
                "isChargeable": false,
                "chargedCents": 0.0,
                "owningUser": "private-user",
                "conversationId": "00000000-0000-0000-0000-000000000000"
            }),
        }
    }

    #[test]
    fn hosted_events_map_to_exact_tokens_and_tokenless_requests() {
        // 2026-07-15T10:00:00Z is 2026-07-15 18:00 in UTC+08:00.
        let summary = aggregate_events(
            &[
                event(
                    1_784_080_800_000,
                    "cursor-grok-4.6-xhigh-fast",
                    Some((864, 319, 126_848)),
                ),
                event(1_784_081_160_000, "grok-bot-automation", None),
            ],
            &window(),
        );

        assert_eq!(summary.source, Some(SOURCE));
        assert_eq!(summary.message_count, 2);
        assert_eq!(summary.explicit_records, 1);
        assert_eq!(summary.estimated_records, 0);
        assert_eq!(summary.token_unavailable_records, 1);
        // Disjoint Cursor counters fold into the prompt total, and the cached
        // counter stays a subset of it.
        assert_eq!(summary.explicit_prompt_tokens, 127_712);
        assert_eq!(summary.explicit_cached_input_tokens, 126_848);
        assert_eq!(summary.explicit_completion_tokens, 319);
        assert_eq!(summary.explicit_total_tokens, 128_031);
        // A tokenless request never turns into tokens or estimates, and the
        // source never claims high confidence for it.
        assert_eq!(summary.confidence(), "medium");
        let contract = summary.to_json_for_agent(None, &crate::domain::model_registry::snapshot());
        assert_eq!(contract["source"], SOURCE);
        assert_eq!(
            contract["dailyUsage"][0]["modelTokenUsage"]["grok-bot-automation"]["totalTokens"],
            0
        );
        assert_eq!(
            contract["dailyUsage"][0]["modelTokenUsage"]["grok-bot-automation"]["tokenUnavailableRequests"],
            1
        );
        let serialized = contract.to_string();
        assert!(!serialized.contains("private-user"));
        assert!(!serialized.contains("conversationId"));
        assert!(!serialized.contains("00000000-0000"));
    }

    #[test]
    fn hosted_placeholder_uses_only_actual_model_on_the_same_usage_event() {
        let mut linked = event(1_784_080_800_000, "default", Some((10, 2, 0)));
        linked["tokenUsage"]["model"] = json!("actual-response-model");
        let unknown = event(1_784_080_800_001, "auto", Some((5, 1, 0)));
        let summary = aggregate_events(&[linked, unknown], &window());
        let report = summary.to_json_for_agent(None, &crate::domain::model_registry::snapshot());
        assert_eq!(summary.total_tokens(), 18);
        assert_eq!(
            report["dailyUsage"][0]["modelUsage"]["actual-response-model"],
            12
        );
        assert_eq!(report["dailyUsage"][0]["modelUsage"]["Others"], 6);
    }

    #[test]
    fn events_outside_the_window_or_without_timestamps_are_ignored() {
        let summary = aggregate_events(
            &[
                json!({"model": "composer-2.5", "tokenUsage": {"inputTokens": 10}}),
                event(1_690_000_000_000, "composer-2.5", Some((10, 5, 0))),
            ],
            &window(),
        );
        assert_eq!(summary.explicit_records, 0);
        assert_eq!(summary.token_unavailable_records, 0);
        assert_eq!(summary.total_tokens(), 0);
    }

    #[test]
    fn page_shapes_require_a_usable_envelope() {
        // An empty query window answers with an empty object.
        let empty = parse_events_page(&json!({})).unwrap();
        assert_eq!(empty.total, Some(0));
        assert!(empty.events.is_empty());
        // An empty terminal page keeps the query's authoritative count.
        let terminal = parse_events_page(&json!({"totalUsageEventsCount": 4})).unwrap();
        assert_eq!(terminal.total, Some(4));
        assert!(terminal.events.is_empty());
        // Anything else is rejected instead of reading as confirmed-empty usage.
        assert!(parse_events_page(&json!({"error": "Unauthorized"})).is_err());
        assert!(parse_events_page(&json!({"usageEventsDisplay": "nope"})).is_err());
        assert!(parse_events_page(&json!({"totalUsageEventsCount": "many"})).is_err());
        assert!(parse_events_page(&json!([1, 2, 3])).is_err());
    }

    #[test]
    fn pagination_removes_only_proven_boundary_repeats() {
        let first = vec![json!({"timestamp": "1"}), json!({"timestamp": "2"})];
        let repeated = vec![json!({"timestamp": "2"}), json!({"timestamp": "3"})];
        let reconciled = reconcile_pages(vec![first.clone(), repeated], Some(3)).unwrap();
        assert_eq!(reconciled.len(), 3);

        // A total the pages cannot explain fails closed.
        assert_eq!(
            reconcile_pages(
                vec![
                    vec![json!({"timestamp": "1"})],
                    vec![json!({"timestamp": "2"})]
                ],
                Some(5)
            )
            .unwrap_err(),
            HostedUsageFailure::PaginationIncomplete
        );
        // A total smaller than the proven overlap also fails closed.
        assert!(reconcile_pages(vec![first], Some(1)).is_err());
        // Several pages without an authoritative count cannot be reconciled.
        assert!(
            reconcile_pages(
                vec![
                    vec![json!({"timestamp": "1"})],
                    vec![json!({"timestamp": "2"})]
                ],
                None
            )
            .is_err()
        );
        // A single page without a count is unambiguous.
        assert_eq!(
            reconcile_pages(vec![vec![json!({"timestamp": "1"})]], None)
                .unwrap()
                .len(),
            1
        );
    }

    #[test]
    fn coverage_reuses_only_sealed_days_and_refetches_the_rest() {
        let window = window();
        let coverage = HostedCoverage {
            coverage_start: Some("2026-07-13".to_owned()),
            sealed_through: Some("2026-07-14".to_owned()),
            days: BTreeMap::new(),
        };
        // Today (2026-07-15) is still accumulating, so it is always fetched.
        assert_eq!(coverage.next_fetch_day(&window), "2026-07-15");

        let partial = HostedCoverage {
            coverage_start: Some("2026-07-14".to_owned()),
            sealed_through: Some("2026-07-14".to_owned()),
            days: BTreeMap::new(),
        };
        assert_eq!(partial.next_fetch_day(&window), "2026-07-13");

        assert_eq!(
            HostedCoverage::default().next_fetch_day(&window),
            "2026-07-13"
        );
    }

    #[test]
    fn retained_coverage_round_trips_published_aggregates_only() {
        let window = window();
        let report = json!({
            "generatedAt": "2026-07-15T12:00:00Z",
            "usageParserRevision": licoup_statistics::contract::USAGE_PARSER_REVISION,
            "agents": [{
                "agentId": "cursor",
                "history": {
                    "scanCache": {"hosted": true, "hostedCoverageStart": "2026-07-13"},
                    "dailyUsage": [{
                        "date": "2026-07-13",
                        "promptTokens": 10,
                        "cachedInputTokens": 4,
                        "completionTokens": 5,
                        "totalTokens": 15,
                        "messageCount": 2,
                        "explicitRecords": 1,
                        "requestCount": 2,
                        "tokenUnavailableRequests": 1,
                        "modelTokenUsage": {
                            "composer-2.5": {
                                "promptTokens": 10,
                                "cachedInputTokens": 4,
                                "completionTokens": 5,
                                "totalTokens": 15,
                                "requestCount": 1
                            },
                            "cursor-auto": {
                                "promptTokens": 0,
                                "cachedInputTokens": 0,
                                "completionTokens": 0,
                                "totalTokens": 0,
                                "requestCount": 1,
                                "tokenUnavailableRequests": 1
                            }
                        }
                    }]
                }
            }]
        });
        let coverage = parse_coverage(&report, &window).unwrap();
        assert_eq!(coverage.coverage_start.as_deref(), Some("2026-07-13"));
        assert_eq!(coverage.sealed_through.as_deref(), Some("2026-07-14"));
        assert!(coverage.covers("2026-07-13"));
        assert!(!coverage.covers("2026-07-15"));
        let day = coverage.days.get("2026-07-13").unwrap();
        assert_eq!(day.total_tokens, 15);
        assert_eq!(day.token_unavailable_requests, 1);
        assert_eq!(day.model_usage["cursor-auto"].token_unavailable_requests, 1);
    }

    #[test]
    fn adopted_days_keep_published_counters_and_never_overlap_fetched_days() {
        let window = window();
        let mut baseline = HostedCoverage {
            coverage_start: Some("2026-07-13".to_owned()),
            sealed_through: Some("2026-07-14".to_owned()),
            days: BTreeMap::new(),
        };
        baseline.days.insert(
            "2026-07-13".to_owned(),
            DailyUsageSummary {
                prompt_tokens: 10,
                completion_tokens: 5,
                total_tokens: 15,
                message_count: 2,
                explicit_records: 1,
                request_count: 2,
                token_unavailable_requests: 1,
                ..DailyUsageSummary::default()
            },
        );
        baseline.days.insert(
            "2026-07-15".to_owned(),
            DailyUsageSummary {
                total_tokens: 99,
                ..DailyUsageSummary::default()
            },
        );
        let mut summary = aggregate_events(
            &[event(1_784_080_800_000, "composer-2.5", Some((10, 5, 0)))],
            &window,
        );
        summary = merge_baseline(summary, &baseline, &window, "2026-07-14");

        assert_eq!(summary.daily_usage["2026-07-13"].total_tokens, 15);
        assert_eq!(summary.daily_usage["2026-07-13"].request_count, 2);
        assert_eq!(summary.total_tokens(), 30);
        assert_eq!(summary.token_unavailable_records, 1);
        // The fetched day is authoritative: the stale duplicate is dropped.
        assert_eq!(summary.daily_usage["2026-07-15"].total_tokens, 15);
        let scan_cache = summary.scan_cache.as_ref().unwrap();
        assert_eq!(scan_cache["hostedCoverageStart"], "2026-07-13");
        assert_eq!(scan_cache["reusedDays"], 1);
    }

    #[test]
    fn local_day_query_bounds_follow_the_scan_offset() {
        let window = window();
        // 2026-07-15 00:00 in UTC+08:00 is 2026-07-14T16:00:00Z.
        assert_eq!(
            window.local_day_start_millis("2026-07-15"),
            Some(1_784_044_800_000)
        );
        assert_eq!(window.local_day_start_millis("not-a-day"), None);
    }

    #[test]
    fn failure_codes_are_stable_and_sanitized() {
        assert_eq!(
            HostedUsageFailure::Session("cursor_auth_token_absent").code(),
            "cursor_auth_token_absent"
        );
        assert_eq!(
            HostedUsageFailure::Unauthorized.code(),
            "cursor_usage_unauthorized"
        );
        assert_eq!(
            HostedUsageFailure::PaginationIncomplete.code(),
            "cursor_usage_pagination_incomplete"
        );
        assert_eq!(HostedUsageFailure::TimedOut.code(), "cursor_usage_timeout");
    }

    #[test]
    fn empty_window_days_are_bounded_and_ordered() {
        let days = window_days(&window());
        assert_eq!(days, vec!["2026-07-13", "2026-07-14", "2026-07-15"]);
    }

    #[test]
    fn unparsable_report_coverage_is_ignored() {
        let report = json!({"generatedAt": "2026-07-15T12:00:00Z", "agents": []});
        assert!(parse_coverage(&report, &window()).is_none());
    }
}
