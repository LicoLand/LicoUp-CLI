//! Provider and Agent model billing facts owned by one current catalog.
//!
//! The embedded JSON is the sole maintained pricing authority. Rust reads its
//! rich route and tier records directly and projects only the default tier at
//! planning boundaries.

use regex::Regex;
use serde::Deserialize;
use std::collections::{BTreeSet, HashSet};
use std::io::Read;
use std::sync::{OnceLock, RwLock};
use std::time::Duration;

const PRICING_CATALOG_JSON: &str = include_str!("pricing/pricing_catalog.json");
const MAX_PRICE_PAGE_BYTES: u64 = 2 * 1024 * 1024;
const REFRESH_TIMEOUT: Duration = Duration::from_secs(8);

#[derive(Clone, Debug, Deserialize)]
struct PricingCatalog {
    last_updated: String,
    providers: Vec<PricingTable>,
    agents: Vec<PricingTable>,
}

#[derive(Clone, Debug, Deserialize)]
struct PricingTable {
    id: String,
    unit: String,
    routes: Vec<PricingRoute>,
}

#[derive(Clone, Debug, Deserialize)]
struct PricingRoute {
    model_id: String,
    lifecycle: Lifecycle,
    verified_on: String,
    source_urls: Vec<String>,
    billing_mode: String,
    included_by_harness: bool,
    tiers: Vec<PricingTier>,
}

#[derive(Clone, Debug, Deserialize)]
struct Lifecycle {
    status: String,
    service_end: Option<String>,
}

#[derive(Clone, Debug, Deserialize)]
struct PricingTier {
    default: bool,
    input: f64,
    cache_read: Option<f64>,
    cache_write: Option<CacheWrite>,
    output: f64,
    context_min: Option<u64>,
    context_max: Option<u64>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(untagged)]
enum CacheWrite {
    Price(f64),
    Retention(Vec<CacheWriteRate>),
}

#[derive(Clone, Debug, Deserialize)]
struct CacheWriteRate {
    ttl_seconds: u64,
    price: f64,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ModelTokenPrice {
    pub input: f64,
    pub output: f64,
}

#[derive(Clone, Debug, PartialEq)]
pub struct PlanningModelPrice {
    pub input: f64,
    pub cached_input: f64,
    pub output: f64,
    pub unit: String,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct RefreshSummary {
    pub providers_attempted: usize,
    pub providers_refreshed: usize,
}

static PRICING: OnceLock<Option<RwLock<PricingCatalog>>> = OnceLock::new();

fn pricing() -> Option<&'static RwLock<PricingCatalog>> {
    PRICING
        .get_or_init(|| {
            let catalog = serde_json::from_str::<PricingCatalog>(PRICING_CATALOG_JSON).ok()?;
            validate_catalog(&catalog).then_some(RwLock::new(catalog))
        })
        .as_ref()
}

fn valid_date(value: &str) -> bool {
    let bytes = value.as_bytes();
    if bytes.len() != 10 || bytes[4] != b'-' || bytes[7] != b'-' {
        return false;
    }
    if !bytes
        .iter()
        .enumerate()
        .all(|(index, byte)| index == 4 || index == 7 || byte.is_ascii_digit())
    {
        return false;
    }
    let year = value[0..4].parse::<u32>().ok();
    let month = value[5..7].parse::<u32>().ok();
    let day = value[8..10].parse::<u32>().ok();
    let (Some(year), Some(month), Some(day)) = (year, month, day) else {
        return false;
    };
    if year == 0 || !(1..=12).contains(&month) {
        return false;
    }
    let leap = year % 4 == 0 && (year % 100 != 0 || year % 400 == 0);
    let days = match month {
        2 if leap => 29,
        2 => 28,
        4 | 6 | 9 | 11 => 30,
        _ => 31,
    };
    (1..=days).contains(&day)
}

fn date_key(value: &str) -> u32 {
    value
        .split('-')
        .filter_map(|part| part.parse::<u32>().ok())
        .fold(0, |key, part| key * 100 + part)
}

fn valid_cache_write(value: Option<&CacheWrite>) -> bool {
    match value {
        None => true,
        Some(CacheWrite::Price(price)) => price.is_finite() && *price >= 0.0,
        Some(CacheWrite::Retention(rates)) => {
            if rates.is_empty() {
                return false;
            }
            let mut previous_ttl = 0;
            rates.iter().all(|rate| {
                let valid =
                    rate.ttl_seconds > previous_ttl && rate.price.is_finite() && rate.price >= 0.0;
                previous_ttl = rate.ttl_seconds;
                valid
            })
        }
    }
}

fn valid_tier(tier: &PricingTier) -> bool {
    tier.input.is_finite()
        && tier.input >= 0.0
        && tier
            .cache_read
            .is_none_or(|price| price.is_finite() && price >= 0.0)
        && valid_cache_write(tier.cache_write.as_ref())
        && tier.output.is_finite()
        && tier.output >= 0.0
        && tier
            .context_min
            .zip(tier.context_max)
            .is_none_or(|(min, max)| min <= max)
}

fn valid_route(route: &PricingRoute, last_updated: &str) -> bool {
    if route.model_id.is_empty()
        || route.lifecycle.status != "active"
        || !valid_date(&route.verified_on)
        || date_key(&route.verified_on) > date_key(last_updated)
        || route.source_urls.is_empty()
        || route.billing_mode.is_empty()
        || route.tiers.is_empty()
    {
        return false;
    }
    if route
        .lifecycle
        .service_end
        .as_deref()
        .is_some_and(|date| !valid_date(date) || date_key(date) <= date_key(last_updated))
    {
        return false;
    }
    let mut sources = HashSet::new();
    if route
        .source_urls
        .iter()
        .any(|source| !source.starts_with("https://") || !sources.insert(source))
    {
        return false;
    }
    let defaults = route.tiers.iter().filter(|tier| tier.default).count();
    if defaults != 1 || route.tiers.iter().any(|tier| !valid_tier(tier)) {
        return false;
    }
    for (index, left) in route.tiers.iter().enumerate() {
        for right in route.tiers.iter().skip(index + 1) {
            let left_min = left.context_min.unwrap_or(0);
            let left_max = left.context_max.unwrap_or(u64::MAX);
            let right_min = right.context_min.unwrap_or(0);
            let right_max = right.context_max.unwrap_or(u64::MAX);
            if left_min <= right_max && right_min <= left_max {
                return false;
            }
        }
    }
    true
}

fn valid_table(
    table: &PricingTable,
    last_updated: &str,
    table_ids: &mut HashSet<String>,
    raw_model_ids: Option<&mut HashSet<String>>,
) -> bool {
    if table.id.is_empty()
        || table.unit.is_empty()
        || table.routes.is_empty()
        || !table_ids.insert(table.id.clone())
    {
        return false;
    }
    let mut route_ids = HashSet::new();
    let mut raw_model_ids = raw_model_ids;
    for route in &table.routes {
        if !valid_route(route, last_updated) || !route_ids.insert(route.model_id.clone()) {
            return false;
        }
        if let Some(ids) = raw_model_ids.as_mut()
            && !ids.insert(route.model_id.clone())
        {
            return false;
        }
    }
    true
}

fn validate_catalog(catalog: &PricingCatalog) -> bool {
    if !valid_date(&catalog.last_updated)
        || catalog.providers.is_empty()
        || catalog.agents.is_empty()
        || catalog.providers.len() + catalog.agents.len() != 10
    {
        return false;
    }
    let mut table_ids = HashSet::new();
    let mut raw_model_ids = HashSet::new();
    catalog.providers.iter().all(|table| {
        valid_table(
            table,
            &catalog.last_updated,
            &mut table_ids,
            Some(&mut raw_model_ids),
        )
    }) && catalog
        .agents
        .iter()
        .all(|table| valid_table(table, &catalog.last_updated, &mut table_ids, None))
}

/// Return one model's raw input and output rates for prediction and planning.
pub fn model_price(model_id: &str) -> Option<ModelTokenPrice> {
    model_planning_price(model_id).map(|price| price.token_price())
}

pub fn model_planning_price(model_id: &str) -> Option<PlanningModelPrice> {
    let guard = pricing()?.read().ok()?;
    let mut selected = None;
    for table in &guard.providers {
        let Some(route) = find_route(table, model_id) else {
            continue;
        };
        if selected.is_some() {
            return None;
        }
        selected = Some((table, route));
    }
    selected.map(|(table, route)| planning_price(table, route, false))
}

/// Return the effective Agent route rates for one model and Thinking setting.
pub fn agent_model_price(
    agent_id: &str,
    model_id: &str,
    thinking: &str,
) -> Option<ModelTokenPrice> {
    agent_model_planning_price(agent_id, model_id, thinking).map(|price| price.token_price())
}

pub fn agent_model_planning_price(
    agent_id: &str,
    model_id: &str,
    thinking: &str,
) -> Option<PlanningModelPrice> {
    if thinking.trim().is_empty() {
        return None;
    }
    let table_id = provider_for_agent(agent_id)?;
    let guard = pricing()?.read().ok()?;
    let table = guard.agents.iter().find(|table| table.id == table_id)?;
    let route = find_route(table, agent_price_key(agent_id, model_id))?;
    Some(planning_price(table, route, route.included_by_harness))
}

fn agent_price_key<'a>(agent_id: &str, model_id: &'a str) -> &'a str {
    if agent_id == "kilo-code"
        && (model_id == "kilo-auto/free"
            || model_id.ends_with("/kilo-auto/free")
            || model_id.ends_with(":free"))
    {
        "free"
    } else {
        model_id
    }
}

/// Resolve a planning/AA identifier to the exact provider identifier used by
/// the official source. This is intentionally a bounded table, rather than a
/// punctuation normalizer, because dots and hyphens are meaningful in some
/// provider namespaces.
fn source_model_alias(model_id: &str) -> Option<&'static str> {
    match model_id {
        "gpt-5-6-sol" => Some("gpt-5.6-sol"),
        "gpt-5-6-terra" => Some("gpt-5.6-terra"),
        "gpt-5-6-luna" => Some("gpt-5.6-luna"),
        "gpt-5-5" => Some("gpt-5.5"),
        "gpt-5-4" => Some("gpt-5.4"),
        "gpt-5-3-codex" => Some("gpt-5.3-codex"),
        "kimi-k2-7-code" => Some("kimi-k2.7-code"),
        "kimi-k2-7-code-highspeed" => Some("kimi-k2.7-code-highspeed"),
        "opus-5" => Some("claude-opus-5"),
        "opus-4-8" => Some("claude-opus-4-8"),
        "opus-4-7" => Some("claude-opus-4-7"),
        "opus-4-6" => Some("claude-opus-4-6"),
        "fable-5" => Some("claude-fable-5"),
        "gemini-3-6-flash" => Some("gemini-3.6-flash"),
        "gemini-3-1-pro" => Some("gemini-3.1-pro"),
        "gemini-3-1-pro-preview" => Some("gemini-3.1-pro-preview"),
        "muse-spark-1-1" => Some("muse-spark-1.1"),
        "muse-spark-1-2" => Some("muse-spark-1.2"),
        "composer-2-5" => Some("composer-2.5"),
        "composer-2-5-fast" => Some("composer-2.5-fast"),
        "grok-4-5" => Some("grok-4.5"),
        _ => None,
    }
}

fn route_index(table: &PricingTable, model_id: &str) -> Option<usize> {
    table
        .routes
        .iter()
        .position(|route| route.model_id == model_id)
        .or_else(|| {
            source_model_alias(model_id).and_then(|alias| {
                table
                    .routes
                    .iter()
                    .position(|route| route.model_id == alias)
            })
        })
        .or_else(|| {
            table.routes.iter().position(|route| {
                source_model_alias(route.model_id.as_str()).is_some_and(|alias| alias == model_id)
            })
        })
}

fn find_route<'a>(table: &'a PricingTable, model_id: &str) -> Option<&'a PricingRoute> {
    route_index(table, model_id).and_then(|index| table.routes.get(index))
}

fn default_tier(route: &PricingRoute) -> Option<&PricingTier> {
    route.tiers.iter().find(|tier| tier.default)
}

impl PlanningModelPrice {
    fn token_price(&self) -> ModelTokenPrice {
        ModelTokenPrice {
            input: self.input,
            output: self.output,
        }
    }
}

fn planning_price(
    table: &PricingTable,
    route: &PricingRoute,
    included: bool,
) -> PlanningModelPrice {
    let Some(tier) = default_tier(route) else {
        return PlanningModelPrice {
            input: 0.0,
            cached_input: 0.0,
            output: 0.0,
            unit: table.unit.clone(),
        };
    };
    if included {
        return PlanningModelPrice {
            input: 0.0,
            cached_input: 0.0,
            output: 0.0,
            unit: table.unit.clone(),
        };
    }
    PlanningModelPrice {
        input: tier.input,
        cached_input: tier.cache_read.unwrap_or(tier.input),
        output: tier.output,
        unit: table.unit.clone(),
    }
}

/// Refresh every pricing table concurrently from its official public source.
/// Each table is merged independently; malformed or unavailable pages keep
/// the last embedded/in-memory values.
pub fn refresh_official_sources() -> RefreshSummary {
    let Some(lock) = pricing() else {
        return RefreshSummary::default();
    };
    let entries = lock
        .read()
        .ok()
        .map(|catalog| {
            catalog
                .providers
                .iter()
                .cloned()
                .map(|table| (false, table))
                .chain(catalog.agents.iter().cloned().map(|table| (true, table)))
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    let attempted = entries.len();
    let handles = entries.into_iter().map(|(is_agent, table)| {
        std::thread::spawn(move || refresh_pricing_table(table).map(|table| (is_agent, table)))
    });
    let refreshed = handles
        .filter_map(|handle| handle.join().ok().flatten())
        .collect::<Vec<_>>();
    let refreshed_count = refreshed.len();
    if let Ok(mut catalog) = lock.write() {
        for (is_agent, table) in refreshed {
            let tables = if is_agent {
                &mut catalog.agents
            } else {
                &mut catalog.providers
            };
            if let Some(current) = tables.iter_mut().find(|row| row.id == table.id) {
                *current = table;
            }
        }
    }
    RefreshSummary {
        providers_attempted: attempted,
        providers_refreshed: refreshed_count,
    }
}

fn provider_for_agent(agent_id: &str) -> Option<&'static str> {
    match agent_id {
        "cursor" | "cursor-cli" => Some("cursor"),
        "codex" => Some("openai-chatgpt"),
        "kilo-code" => Some("kilo"),
        "opencode" => Some("opencode-zen"),
        _ => None,
    }
}

fn refresh_pricing_table(mut table: PricingTable) -> Option<PricingTable> {
    let source_urls = table
        .routes
        .iter()
        .flat_map(|route| route.source_urls.iter().cloned())
        .collect::<BTreeSet<_>>();
    let pages = source_urls
        .iter()
        .filter_map(|url| fetch_official_page(url).map(|page| plain_text(&page)))
        .collect::<Vec<_>>();
    if pages.is_empty() {
        return None;
    }
    let text = pages.concat();
    let changed = match table.id.as_str() {
        "deepseek" => refresh_deepseek(&mut table, &text),
        "kimi" => refresh_kimi(&mut table, &text),
        "google" => refresh_google(&mut table, &text),
        "cursor" => refresh_cursor(&mut table, &text),
        "openai" | "openai-chatgpt" => refresh_codex(&mut table, &text),
        "kilo" => refresh_kilo(&mut table, &text),
        _ => false,
    };
    changed.then_some(table)
}

fn refresh_kilo(table: &mut PricingTable, text: &str) -> bool {
    // Kilo's official catalog is dynamic, but its billing contract keeps both
    // Auto Free and :free routes at zero cost.
    if !text.to_ascii_lowercase().contains("kilo-auto/free")
        && !text.to_ascii_lowercase().contains(":free")
    {
        return false;
    }
    set_prices(table, "free", None, 0.0, 0.0)
}

fn fetch_official_page(url: &str) -> Option<String> {
    let parsed = url::Url::parse(url).ok()?;
    if parsed.scheme() != "https" {
        return None;
    }
    let response = ureq::AgentBuilder::new()
        .timeout_connect(REFRESH_TIMEOUT)
        .timeout_read(REFRESH_TIMEOUT)
        .timeout_write(REFRESH_TIMEOUT)
        .redirects(3)
        .build()
        .get(url)
        .call()
        .ok()?;
    let mut reader = response.into_reader().take(MAX_PRICE_PAGE_BYTES + 1);
    let mut body = String::new();
    reader.read_to_string(&mut body).ok()?;
    (body.len() as u64 <= MAX_PRICE_PAGE_BYTES).then_some(body)
}

fn plain_text(html: &str) -> String {
    let without_tags = Regex::new(r"(?s)<[^>]*>")
        .expect("static html regex")
        .replace_all(html, " ");
    Regex::new(r"\s+")
        .expect("static whitespace regex")
        .replace_all(&without_tags, " ")
        .replace("&amp;", "&")
        .replace("&#36;", "$")
}

fn set_prices(
    table: &mut PricingTable,
    model_id: &str,
    cached: Option<f64>,
    input: f64,
    output: f64,
) -> bool {
    let Some(index) = route_index(table, model_id) else {
        return false;
    };
    let Some(tier) = table.routes[index]
        .tiers
        .iter_mut()
        .find(|tier| tier.default)
    else {
        return false;
    };
    tier.cache_read = cached;
    tier.input = input;
    tier.output = output;
    true
}

fn captures(text: &str, pattern: &str) -> Option<Vec<f64>> {
    let regex = Regex::new(pattern).ok()?;
    let capture = regex.captures(text)?;
    (1..capture.len())
        .map(|index| capture.get(index)?.as_str().parse::<f64>().ok())
        .collect()
}

fn refresh_deepseek(table: &mut PricingTable, text: &str) -> bool {
    let Some(values) = captures(
        text,
        r"(?i)CACHE HIT\D+\$([0-9.]+)\s+\$([0-9.]+).*?CACHE MISS\D+\$([0-9.]+)\s+\$([0-9.]+).*?OUTPUT TOKENS\D+\$([0-9.]+)\s+\$([0-9.]+)",
    ) else {
        return false;
    };
    set_prices(
        table,
        "deepseek-v4-flash",
        Some(values[0]),
        values[2],
        values[4],
    ) & set_prices(
        table,
        "deepseek-v4-pro",
        Some(values[1]),
        values[3],
        values[5],
    )
}

fn refresh_kimi(table: &mut PricingTable, text: &str) -> bool {
    let k3 = captures(
        text,
        r"(?i)kimi-k3\s+1M tokens\s+\$([0-9.]+)\s+\$([0-9.]+)\s+\$([0-9.]+)",
    )
    .is_some_and(|values| set_prices(table, "kimi-k3", Some(values[0]), values[1], values[2]));
    let code = captures(
        text,
        r"(?i)kimi-k2\.7-code\s+1M tokens\s+\$([0-9.]+)\s+\$([0-9.]+)\s+\$([0-9.]+)",
    )
    .is_some_and(|values| {
        set_prices(
            table,
            "kimi-k2-7-code",
            Some(values[0]),
            values[1],
            values[2],
        )
    });
    k3 || code
}

fn refresh_google(table: &mut PricingTable, text: &str) -> bool {
    [
        ("gemini-3-6-flash", "gemini-3.6-flash"),
        ("gemini-3-1-pro-preview", "gemini-3.1-pro-preview"),
    ]
    .into_iter()
    .filter_map(|(id, marker)| {
        let start = text.find(marker)?;
        let window = text.get(start..start.saturating_add(1_200).min(text.len()))?;
        let values = captures(
            window,
            r"(?i)Input price.*?\$([0-9.]+).*?Output price.*?\$([0-9.]+).*?Context caching price.*?\$([0-9.]+)",
        )?;
        Some(set_prices(table, id, Some(values[2]), values[0], values[1]))
    })
    .any(|changed| changed)
}

fn refresh_cursor(table: &mut PricingTable, text: &str) -> bool {
    let Some(values) = captures(
        text,
        r"(?i)Standard:\s*\$([0-9.]+)/M input,\s*\$([0-9.]+)/M output.*?Fast \(default\):\s*\$([0-9.]+)/M input,\s*\$([0-9.]+)/M output",
    ) else {
        return false;
    };
    set_prices(table, "composer-2-5", None, values[0], values[1])
        & set_prices(table, "composer-2-5-fast", None, values[2], values[3])
}

fn refresh_codex(table: &mut PricingTable, text: &str) -> bool {
    [
        ("gpt-5-6-sol", "GPT-5.6 Sol"),
        ("gpt-5-6-terra", "GPT-5.6 Terra"),
        ("gpt-5-6-luna", "GPT-5.6 Luna"),
    ]
    .into_iter()
    .filter_map(|(id, marker)| {
        let start = text.find(marker)?;
        let window = text.get(start..start.saturating_add(240).min(text.len()))?;
        let values = captures(
            window,
            r"(?i)([0-9.]+) credits\s+([0-9.]+) credits\s+([0-9,.]+) credits",
        )?;
        Some(set_prices(table, id, Some(values[1]), values[0], values[2]))
    })
    .any(|changed| changed)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn catalog_last_updated_bounds_route_verification_dates() {
        let catalog = serde_json::from_str::<PricingCatalog>(PRICING_CATALOG_JSON).unwrap();
        assert!(validate_catalog(&catalog));

        let mut invalid_catalog_date = catalog.clone();
        invalid_catalog_date.last_updated = "invalid".to_owned();
        assert!(!validate_catalog(&invalid_catalog_date));

        let mut future_route = catalog;
        future_route.providers[0].routes[0].verified_on = "9999-12-31".to_owned();
        assert!(!validate_catalog(&future_route));
    }

    #[test]
    fn model_price_returns_planning_rates_without_usage() {
        let price = model_price("deepseek-v4-flash").unwrap();
        assert!(price.input.is_finite() && price.input >= 0.0);
        assert!(price.output.is_finite() && price.output >= 0.0);
        assert_eq!(model_price("unknown"), None);
    }

    #[test]
    fn agent_model_price_applies_agent_included_routes() {
        assert_eq!(
            agent_model_price("cursor", "composer-2-5", "none"),
            Some(ModelTokenPrice {
                input: 0.0,
                output: 0.0,
            })
        );
        assert_eq!(
            agent_model_price("cursor-cli", "composer-2.5", "none"),
            Some(ModelTokenPrice {
                input: 0.0,
                output: 0.0,
            })
        );
        assert_eq!(
            agent_model_price("kilo-code", "kilo-auto/free", "none"),
            Some(ModelTokenPrice {
                input: 0.0,
                output: 0.0,
            })
        );
        assert_eq!(agent_model_price("unknown", "unknown", "none"), None);
    }

    #[test]
    fn raw_supplier_and_agent_override_tables_have_distinct_lookup_roles() {
        assert_eq!(model_price("big-pickle"), None);
        assert_eq!(model_price("free"), None);
        assert_eq!(model_price("composer-2-5"), None);
        assert_eq!(
            agent_model_price("opencode", "big-pickle", "none"),
            Some(ModelTokenPrice {
                input: 0.0,
                output: 0.0,
            })
        );

        let raw = model_planning_price("gpt-5-6-sol").unwrap();
        let raw_official = model_planning_price("gpt-5.6-sol").unwrap();
        assert_eq!(raw, raw_official);
        assert_eq!(raw.unit, "usd_per_million_tokens");

        let codex = agent_model_planning_price("codex", "gpt-5-6-sol", "max").unwrap();
        assert_eq!(codex.unit, "credits_per_million_tokens");
        assert_ne!(raw.unit, codex.unit);

        let zen = agent_model_price("opencode", "gpt-5-6-sol", "max").unwrap();
        assert_eq!(
            zen,
            agent_model_price("opencode", "gpt-5.6-sol", "max").unwrap()
        );
    }

    #[test]
    fn provider_parsers_update_synthetic_official_tables() {
        let mut catalog = serde_json::from_str::<PricingCatalog>(PRICING_CATALOG_JSON).unwrap();
        let deepseek = catalog
            .providers
            .iter_mut()
            .find(|table| table.id == "deepseek")
            .unwrap();
        assert!(refresh_deepseek(
            deepseek,
            "CACHE HIT $0.1 $0.2 CACHE MISS $1 $2 OUTPUT TOKENS $3 $4"
        ));
        assert_eq!(
            find_route(deepseek, "deepseek-v4-flash").unwrap().tiers[0].output,
            3.0
        );

        let cursor = catalog
            .agents
            .iter_mut()
            .find(|table| table.id == "cursor")
            .unwrap();
        assert!(refresh_cursor(
            cursor,
            "Standard: $0.5/M input, $2.5/M output tokens Fast (default): $3/M input, $15/M output tokens"
        ));
        assert_eq!(
            find_route(cursor, "composer-2-5-fast").unwrap().tiers[0].output,
            15.0
        );

        let kilo = catalog
            .agents
            .iter_mut()
            .find(|table| table.id == "kilo")
            .unwrap();
        assert!(refresh_kilo(
            kilo,
            "Auto Free (kilo-auto/free) requires no credits; tagged models use :free."
        ));
        assert_eq!(find_route(kilo, "free").unwrap().tiers[0].input, 0.0);
    }
}
