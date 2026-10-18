//! Public provider-quota snapshot command orchestration.

use super::contract::{
    MAX_RETAINED_PROVIDERS, ProviderQuotaSnapshot, QUOTA_PROVIDERS, QuotaFetchError, QuotaProvider,
    QuotaStatus, SNAPSHOT_COLLECTION, SNAPSHOT_SCHEMA_VERSION,
};
use super::persistence::{
    RetainedProviderState, client_state_store, load_retained, persist_provider,
};
use super::scheduler::{self, RefreshGate};
use super::{antigravity, codex, cursor, kimi_code, redaction};
use crate::domain::conversation::parameters::text_param;
use anyhow::Result;
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::time::{SystemTime, UNIX_EPOCH};
use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;

static REFRESH_GATE: RefreshGate = RefreshGate::new();

/// One provider quota source behind the shared snapshot contract.
pub(super) trait QuotaSource: Send + Sync {
    fn fetch(&self, now: OffsetDateTime) -> Result<ProviderQuotaSnapshot, QuotaFetchError>;
}

impl QuotaSource for codex::CodexSource {
    fn fetch(&self, now: OffsetDateTime) -> Result<ProviderQuotaSnapshot, QuotaFetchError> {
        self.fetch_snapshot(now)
    }
}

impl QuotaSource for cursor::CursorSource {
    fn fetch(&self, now: OffsetDateTime) -> Result<ProviderQuotaSnapshot, QuotaFetchError> {
        self.fetch_snapshot(now)
    }
}

impl QuotaSource for antigravity::AntigravitySource {
    fn fetch(&self, now: OffsetDateTime) -> Result<ProviderQuotaSnapshot, QuotaFetchError> {
        self.fetch_snapshot(now)
    }
}

impl QuotaSource for kimi_code::KimiCodeSource {
    fn fetch(&self, now: OffsetDateTime) -> Result<ProviderQuotaSnapshot, QuotaFetchError> {
        self.fetch_snapshot(now)
    }
}

pub(super) type SourceRegistry = BTreeMap<QuotaProvider, Box<dyn QuotaSource>>;

fn production_sources(params: &Value) -> SourceRegistry {
    let mut sources = SourceRegistry::new();
    sources.insert(
        QuotaProvider::Codex,
        Box::new(codex::CodexSource::production(params)) as Box<dyn QuotaSource>,
    );
    sources.insert(
        QuotaProvider::Cursor,
        Box::new(cursor::CursorSource::production(params)) as Box<dyn QuotaSource>,
    );
    sources.insert(
        QuotaProvider::Antigravity,
        Box::new(antigravity::AntigravitySource::production()) as Box<dyn QuotaSource>,
    );
    sources.insert(
        QuotaProvider::KimiCode,
        Box::new(kimi_code::KimiCodeSource::production(params)) as Box<dyn QuotaSource>,
    );
    sources
}

pub fn snapshot(params: &Value) -> Result<Value> {
    snapshot_with_sources(params, &production_sources(params), &REFRESH_GATE)
}

pub(super) fn snapshot_with_sources(
    params: &Value,
    sources: &SourceRegistry,
    gate: &RefreshGate,
) -> Result<Value> {
    let now = resolve_now(params);
    let now_text = scheduler::format_rfc3339(now);
    let agent_filter = text_param(params, &["agent", "target"]);
    let force_refresh = bool_param(params, "forceRefresh").unwrap_or(false);
    let client_active = bool_param(params, "clientActive").unwrap_or(true);

    let store = client_state_store(params)?;
    let mut retained = load_retained(&store)?;
    let mut snapshots = Vec::<Value>::new();

    for provider in QUOTA_PROVIDERS {
        if agent_filter
            .as_ref()
            .is_some_and(|filter| QuotaProvider::parse(filter) != Some(*provider))
        {
            continue;
        }
        let Some(source) = sources.get(provider) else {
            continue;
        };
        let state = retained.remove(provider).unwrap_or_default();
        let due = scheduler::is_due(state.next_due_at.as_deref(), now);
        if !force_refresh && !due {
            // Not due: serve retained state (or the explicit unavailable
            // projection when nothing is retained) so per-provider backoff
            // bounds fetch attempts even before a first success lands.
            snapshots.push(project_retained(*provider, state.snapshot, now));
            continue;
        }

        let Some(permit) = gate.try_acquire() else {
            // Coalesced: another refresh is in flight; serve retained state.
            snapshots.push(project_retained(*provider, state.snapshot, now));
            continue;
        };
        let outcome = refresh_provider(
            &store,
            *provider,
            source.as_ref(),
            state,
            now,
            &now_text,
            client_active,
        );
        drop(permit);
        snapshots.push(outcome);
    }

    let mut response = json!({
        "ok": true,
        "schemaVersion": SNAPSHOT_SCHEMA_VERSION,
        "generatedAt": now_text,
        "snapshots": snapshots,
        "capabilities": super::contract::quota_capabilities(),
        "retention": {
            "collection": SNAPSHOT_COLLECTION,
            "maxProviders": MAX_RETAINED_PROVIDERS
        },
        "refresh": {
            "clientActive": client_active,
            "activeIntervalSeconds": scheduler::ACTIVE_INTERVAL_SECONDS,
            "idleIntervalSeconds": scheduler::IDLE_INTERVAL_SECONDS
        }
    });
    redaction::redact_outgoing(&mut response);
    Ok(response)
}

/// Run one bounded provider refresh, persist the outcome, and project the
/// snapshot the wire shape carries.
fn refresh_provider(
    store: &crate::platform::client_state::ClientStateStore,
    provider: QuotaProvider,
    source: &dyn QuotaSource,
    state: RetainedProviderState,
    now: OffsetDateTime,
    now_text: &str,
    client_active: bool,
) -> Value {
    match source.fetch(now) {
        Ok(mut fresh) => {
            if let Some(cached) = state.snapshot.as_ref() {
                scheduler::backfill_reset_timestamps(&mut fresh.windows, &cached.windows);
            }
            fresh.status = QuotaStatus::Live;
            let next_due = scheduler::next_due_after_success(now, client_active);
            let _ = persist_provider(
                store,
                provider,
                Some(&fresh),
                0,
                Some(now_text),
                Some(&next_due),
                now_text,
            );
            fresh.wire_value()
        }
        Err(_) => {
            let failures = state.consecutive_failures.saturating_add(1);
            let next_due = scheduler::next_due_after_failure(now, failures);
            let mut retained_snapshot = state.snapshot;
            if let Some(snapshot) = retained_snapshot.as_mut() {
                snapshot.status =
                    scheduler::status_for(&snapshot.captured_at, snapshot.stale_after_seconds, now);
            }
            let _ = persist_provider(
                store,
                provider,
                retained_snapshot.as_ref(),
                failures,
                Some(now_text),
                Some(&next_due),
                now_text,
            );
            project_retained(provider, retained_snapshot, now)
        }
    }
}

/// Project the retained snapshot with its staleness recomputed against now,
/// or an explicit unavailable entry when the provider has no usable data.
/// Unavailable entries carry no fabricated quota: empty windows, no identity.
fn project_retained(
    provider: QuotaProvider,
    snapshot: Option<ProviderQuotaSnapshot>,
    now: OffsetDateTime,
) -> Value {
    match snapshot {
        Some(mut snapshot) => {
            snapshot.status =
                scheduler::status_for(&snapshot.captured_at, snapshot.stale_after_seconds, now);
            snapshot.wire_value()
        }
        None => ProviderQuotaSnapshot {
            agent_id: provider.agent_id().to_owned(),
            provider,
            status: QuotaStatus::Unavailable,
            windows: Vec::new(),
            identity: Default::default(),
            captured_at: scheduler::format_rfc3339(now),
            stale_after_seconds: super::contract::DEFAULT_STALE_AFTER_SECONDS,
        }
        .wire_value(),
    }
}

fn resolve_now(params: &Value) -> OffsetDateTime {
    if let Some(text) = text_param(params, &["now"])
        && let Ok(parsed) = OffsetDateTime::parse(&text, &Rfc3339)
    {
        return parsed;
    }
    let duration = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default();
    OffsetDateTime::from_unix_timestamp(duration.as_secs() as i64)
        .unwrap_or(OffsetDateTime::UNIX_EPOCH)
}

fn bool_param(params: &Value, key: &str) -> Option<bool> {
    params.get(key).and_then(|value| {
        value.as_bool().or_else(|| {
            value
                .as_str()
                .and_then(|text| match text.trim().to_ascii_lowercase().as_str() {
                    "1" | "true" | "yes" | "on" => Some(true),
                    "0" | "false" | "no" | "off" => Some(false),
                    _ => None,
                })
        })
    })
}
