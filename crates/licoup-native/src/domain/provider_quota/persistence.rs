//! Retained provider-quota snapshots and per-provider refresh state.
//!
//! Retention is local-only through the client state store: one record per
//! provider, newest-first, bounded count. Records pass through the redaction
//! guard before every write so no credential material can enter retained
//! state.

use super::contract::{
    MAX_RETAINED_PROVIDERS, ProviderQuotaSnapshot, QuotaProvider, SNAPSHOT_COLLECTION,
    SNAPSHOT_SCHEMA_VERSION,
};
use super::redaction;
use crate::domain::conversation::parameters::text_param;
use crate::platform::client_state::ClientStateStore;
use anyhow::Result;
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::path::PathBuf;

/// Per-provider refresh state retained between command invocations so the
/// adaptive cadence, backoff, and stale retention survive client restarts.
#[derive(Clone, Debug, Default, PartialEq)]
pub(super) struct RetainedProviderState {
    pub(super) snapshot: Option<ProviderQuotaSnapshot>,
    pub(super) consecutive_failures: u32,
    pub(super) last_attempt_at: Option<String>,
    pub(super) next_due_at: Option<String>,
}

pub(super) fn client_state_store(params: &Value) -> Result<ClientStateStore> {
    if let Some(path) = text_param(params, &["stateRoot"])
        && !path.trim().is_empty()
    {
        return ClientStateStore::new(PathBuf::from(path));
    }
    ClientStateStore::portable()
}

pub(super) fn load_retained(
    store: &ClientStateStore,
) -> Result<BTreeMap<QuotaProvider, RetainedProviderState>> {
    let collection = store.read_collection(SNAPSHOT_COLLECTION)?;
    let mut retained = BTreeMap::new();
    for item in collection
        .get("items")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
    {
        let Some(state) = parse_retained_record(&item) else {
            continue;
        };
        retained.insert(state.0, state.1);
    }
    Ok(retained)
}

/// Upsert one provider record: replace the provider's retained snapshot with
/// the new good snapshot when present, always refresh the schedule state, and
/// keep the collection bounded and newest-first.
pub(super) fn persist_provider(
    store: &ClientStateStore,
    provider: QuotaProvider,
    snapshot: Option<&ProviderQuotaSnapshot>,
    consecutive_failures: u32,
    last_attempt_at: Option<&str>,
    next_due_at: Option<&str>,
    updated_at: &str,
) -> Result<()> {
    let mut collection = store.read_collection(SNAPSHOT_COLLECTION)?;
    let mut items = collection
        .get("items")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
        .into_iter()
        .filter(|item| item.get("provider").and_then(Value::as_str) != Some(provider.wire_name()))
        .collect::<Vec<_>>();

    let mut record = json!({
        "schemaVersion": SNAPSHOT_SCHEMA_VERSION,
        "provider": provider.wire_name(),
        "updatedAt": updated_at,
        "snapshot": snapshot.map(ProviderQuotaSnapshot::wire_value).unwrap_or(Value::Null),
        "schedule": {
            "consecutiveFailures": consecutive_failures,
            "lastAttemptAt": last_attempt_at,
            "nextDueAt": next_due_at,
        }
    });
    redaction::redact_outgoing(&mut record);
    items.insert(0, record);
    sort_items_newest_first(&mut items);
    items.truncate(MAX_RETAINED_PROVIDERS);
    if let Some(object) = collection.as_object_mut() {
        object.insert("items".to_owned(), Value::Array(items));
    }
    store
        .write_collection(SNAPSHOT_COLLECTION, collection)
        .map(|_| ())
}

fn parse_retained_record(item: &Value) -> Option<(QuotaProvider, RetainedProviderState)> {
    if item.get("schemaVersion").and_then(Value::as_str) != Some(SNAPSHOT_SCHEMA_VERSION) {
        return None;
    }
    let provider = QuotaProvider::parse(item.get("provider")?.as_str()?)?;
    let snapshot = item
        .get("snapshot")
        .filter(|snapshot| !snapshot.is_null())
        .and_then(|snapshot| {
            serde_json::from_value::<ProviderQuotaSnapshot>(snapshot.clone()).ok()
        });
    let schedule = item.get("schedule").cloned().unwrap_or(Value::Null);
    Some((
        provider,
        RetainedProviderState {
            snapshot,
            consecutive_failures: schedule
                .get("consecutiveFailures")
                .and_then(Value::as_u64)
                .unwrap_or(0) as u32,
            last_attempt_at: schedule
                .get("lastAttemptAt")
                .and_then(Value::as_str)
                .map(str::to_owned),
            next_due_at: schedule
                .get("nextDueAt")
                .and_then(Value::as_str)
                .map(str::to_owned),
        },
    ))
}

fn sort_items_newest_first(items: &mut [Value]) {
    items.sort_by(|left, right| {
        let key = |item: &Value| {
            item.get("updatedAt")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_owned()
        };
        key(right).cmp(&key(left))
    });
}
