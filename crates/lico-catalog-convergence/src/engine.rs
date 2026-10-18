use crate::model::{
    CATALOG_CONVERGENCE_SCHEMA, CatalogFetchedSnapshot, CatalogPullContext, CatalogSnapshot,
    CohortEntry, CohortOutcome, DiscoveryResult, InvalidationNotification, InvalidationResult,
    PendingInvalidation, RefreshOutcomeKind, RefreshResult, is_stale_candidate,
};
use std::collections::{HashMap, HashSet};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;

#[derive(Debug)]
struct CoalesceSlot {
    waiters: AtomicUsize,
    done: Mutex<bool>,
    result: Mutex<Option<RefreshResult>>,
    cv: Condvar,
}

impl CoalesceSlot {
    fn new() -> Self {
        Self {
            waiters: AtomicUsize::new(0),
            done: Mutex::new(false),
            result: Mutex::new(None),
            cv: Condvar::new(),
        }
    }

    fn wait_for_result(&self) -> RefreshResult {
        self.waiters.fetch_add(1, Ordering::SeqCst);
        let mut done = self.done.lock().expect("coalesce done lock");
        while !*done {
            done = self.cv.wait(done).expect("coalesce wait");
        }
        self.result
            .lock()
            .expect("coalesce result lock")
            .clone()
            .expect("coalesce result missing")
    }

    fn complete(&self, result: RefreshResult) {
        {
            let mut stored = self.result.lock().expect("coalesce result lock");
            *stored = Some(result);
            let mut done = self.done.lock().expect("coalesce done lock");
            *done = true;
        }
        self.cv.notify_all();
    }

    fn waiter_count(&self) -> usize {
        self.waiters.load(Ordering::SeqCst)
    }
}

#[derive(Debug)]
struct EngineState {
    partitions: HashMap<String, CatalogSnapshot>,
    pending: HashMap<String, PendingInvalidation>,
    cohort: HashMap<String, CohortEntry>,
    in_flight: HashMap<String, Arc<CoalesceSlot>>,
    reconnect_fences: HashSet<String>,
    last_known_audience_revision: i64,
    ui_observed_revisions: HashMap<String, i64>,
}

impl EngineState {
    fn new() -> Self {
        Self {
            partitions: HashMap::new(),
            pending: HashMap::new(),
            cohort: HashMap::new(),
            in_flight: HashMap::new(),
            reconnect_fences: HashSet::new(),
            last_known_audience_revision: -1,
            ui_observed_revisions: HashMap::new(),
        }
    }
}

pub struct CatalogConvergenceEngine {
    max_partitions: usize,
    state: Mutex<EngineState>,
}

const MAX_TOOLS_PER_PARTITION: usize = 4096;

fn tracked_partition_count(state: &EngineState) -> usize {
    state
        .partitions
        .keys()
        .chain(state.pending.keys())
        .chain(state.cohort.keys())
        .collect::<HashSet<_>>()
        .len()
}

impl Default for CatalogConvergenceEngine {
    fn default() -> Self {
        Self::new(64)
    }
}

impl CatalogConvergenceEngine {
    pub fn new(max_partitions: usize) -> Self {
        Self {
            max_partitions,
            state: Mutex::new(EngineState::new()),
        }
    }

    pub fn apply_invalidation(&self, notification: InvalidationNotification) -> InvalidationResult {
        let mut state = self.state.lock().expect("engine state lock");
        let keys = if !notification.affected_partitions.is_empty() {
            notification.affected_partitions.clone()
        } else {
            notification
                .partition_key
                .as_deref()
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(|value| vec![value.to_string()])
                .unwrap_or_default()
        };

        let audience_revision = notification.audience_revision;
        let catalog_revision = notification.catalog_revision.trim().to_string();
        let source_revision = notification.source_revision;
        let reason_code = if notification.reason_code.trim().is_empty() {
            "tool_list_changed".to_string()
        } else {
            notification.reason_code.trim().to_string()
        };
        let at = now_rfc3339();

        let mut accepted = Vec::new();
        let mut seen = HashSet::new();
        for raw in keys {
            let key = raw.trim().to_string();
            if key.is_empty() || !seen.insert(key.clone()) {
                continue;
            }
            let tracks_key = state.partitions.contains_key(&key)
                || state.pending.contains_key(&key)
                || state.cohort.contains_key(&key);
            if !tracks_key && tracked_partition_count(&state) >= self.max_partitions {
                continue;
            }
            if let Some(current) = state.partitions.get(&key) {
                if is_stale_candidate(
                    current,
                    source_revision,
                    audience_revision,
                    &catalog_revision,
                ) {
                    continue;
                }
            }
            if let Some(prior) = state.pending.get(&key) {
                let same_revision = prior.audience_revision == audience_revision
                    && prior.source_revision == source_revision;
                if prior.audience_revision > audience_revision
                    || prior.source_revision > source_revision
                    || same_revision
                {
                    continue;
                }
            }
            state.pending.insert(
                key.clone(),
                PendingInvalidation {
                    partition_key: key.clone(),
                    audience_revision,
                    catalog_revision: catalog_revision.clone(),
                    source_revision,
                    reason_code: reason_code.clone(),
                    at: at.clone(),
                },
            );
            state.cohort.insert(
                key.clone(),
                CohortEntry {
                    partition_key: key.clone(),
                    outcome: CohortOutcome::Pending,
                    audience_revision,
                    catalog_revision: catalog_revision.clone(),
                    source_revision,
                    at: at.clone(),
                },
            );
            accepted.push(key);
        }

        if audience_revision > state.last_known_audience_revision {
            state.last_known_audience_revision = audience_revision;
        }

        let pending_count = state.pending.len();
        InvalidationResult {
            accepted_partition_keys: accepted,
            pending_count,
        }
    }

    pub fn replace_partition(
        &self,
        partition_key: &str,
        fetched: CatalogFetchedSnapshot,
    ) -> RefreshResult {
        let key = partition_key.trim().to_string();
        let fetched_at = now_rfc3339();
        let snapshot = CatalogSnapshot::from_fetched(&key, &fetched, &fetched_at);
        let mut state = self.state.lock().expect("engine state lock");

        if key.is_empty()
            || snapshot.source_revision < 0
            || snapshot.audience_revision < 0
            || snapshot.catalog_revision.is_empty()
            || snapshot.tools.len() > MAX_TOOLS_PER_PARTITION
            || snapshot
                .tools
                .iter()
                .any(|tool| tool.name.trim().is_empty())
            || snapshot
                .tools
                .iter()
                .map(|tool| tool.name.as_str())
                .collect::<HashSet<_>>()
                .len()
                != snapshot.tools.len()
        {
            return RefreshResult {
                outcome: RefreshOutcomeKind::RejectedConflict,
                partition_key: key,
                snapshot: None,
                retained: None,
            };
        }

        if let Some(pending) = state.pending.get(&key) {
            let behind_pending = snapshot.source_revision < pending.source_revision
                || snapshot.audience_revision < pending.audience_revision;
            let conflicts_pending = snapshot.source_revision == pending.source_revision
                && snapshot.audience_revision == pending.audience_revision
                && snapshot.catalog_revision != pending.catalog_revision;
            if behind_pending || conflicts_pending {
                return RefreshResult {
                    outcome: if behind_pending {
                        RefreshOutcomeKind::RejectedStale
                    } else {
                        RefreshOutcomeKind::RejectedConflict
                    },
                    partition_key: key,
                    snapshot: None,
                    retained: state.partitions.get(partition_key.trim()).cloned(),
                };
            }
        }

        if let Some(current) = state.partitions.get(&key) {
            if is_stale_candidate(
                current,
                snapshot.source_revision,
                snapshot.audience_revision,
                &snapshot.catalog_revision,
            ) {
                return RefreshResult {
                    outcome: RefreshOutcomeKind::RejectedStale,
                    partition_key: key,
                    snapshot: None,
                    retained: Some(current.clone()),
                };
            }
            let same_revision = current.source_revision == snapshot.source_revision
                && current.audience_revision == snapshot.audience_revision;
            if same_revision && current.catalog_revision != snapshot.catalog_revision {
                return RefreshResult {
                    outcome: RefreshOutcomeKind::RejectedConflict,
                    partition_key: key,
                    snapshot: None,
                    retained: Some(current.clone()),
                };
            }
            if same_revision && current.catalog_revision == snapshot.catalog_revision {
                return RefreshResult {
                    outcome: if current.digest == snapshot.digest {
                        RefreshOutcomeKind::Unchanged
                    } else {
                        RefreshOutcomeKind::RejectedConflict
                    },
                    partition_key: key,
                    snapshot: None,
                    retained: Some(current.clone()),
                };
            }
        }

        if state.partitions.len() >= self.max_partitions && !state.partitions.contains_key(&key) {
            return RefreshResult {
                outcome: RefreshOutcomeKind::RejectedCapacity,
                partition_key: key,
                snapshot: None,
                retained: None,
            };
        }

        state.partitions.insert(key.clone(), snapshot.clone());
        state.pending.remove(&key);
        state.reconnect_fences.remove(&key);
        state.cohort.insert(
            key.clone(),
            CohortEntry {
                partition_key: key.clone(),
                outcome: CohortOutcome::Applied,
                audience_revision: snapshot.audience_revision,
                catalog_revision: snapshot.catalog_revision.clone(),
                source_revision: snapshot.source_revision,
                at: fetched_at,
            },
        );
        RefreshResult {
            outcome: RefreshOutcomeKind::Replaced,
            partition_key: key,
            snapshot: Some(snapshot),
            retained: None,
        }
    }

    pub fn refresh_partition<F>(&self, partition_key: &str, fetcher: F) -> RefreshResult
    where
        F: FnOnce(CatalogPullContext) -> CatalogFetchedSnapshot,
    {
        let key = partition_key.trim().to_string();
        if key.is_empty() {
            return RefreshResult {
                outcome: RefreshOutcomeKind::RejectedStale,
                partition_key: key,
                snapshot: None,
                retained: None,
            };
        }

        let slot = {
            let mut state = self.state.lock().expect("engine state lock");
            if let Some(existing) = state.in_flight.get(&key) {
                let existing = Arc::clone(existing);
                drop(state);
                return existing.wait_for_result();
            }
            let slot = Arc::new(CoalesceSlot::new());
            state.in_flight.insert(key.clone(), Arc::clone(&slot));
            slot
        };

        let pull_context = {
            let state = self.state.lock().expect("engine state lock");
            CatalogPullContext {
                partition_key: key.clone(),
                pending_invalidation: state.pending.get(&key).cloned(),
            }
        };

        let result = match catch_unwind(AssertUnwindSafe(|| fetcher(pull_context))) {
            Ok(fetched) => self.replace_partition(&key, fetched),
            Err(_) => RefreshResult {
                outcome: RefreshOutcomeKind::FetchFailed,
                partition_key: key.clone(),
                snapshot: None,
                retained: self
                    .state
                    .lock()
                    .expect("engine state lock")
                    .partitions
                    .get(&key)
                    .cloned(),
            },
        };
        slot.complete(result.clone());

        let mut state = self.state.lock().expect("engine state lock");
        state.in_flight.remove(&key);
        result
    }

    pub fn refresh_waiter_count(&self, partition_key: &str) -> usize {
        let state = self.state.lock().expect("engine state lock");
        state
            .in_flight
            .get(partition_key.trim())
            .map(|slot| slot.waiter_count())
            .unwrap_or(0)
    }

    pub fn list_tools(&self, partition_key: &str) -> DiscoveryResult {
        let state = self.state.lock().expect("engine state lock");
        let key = partition_key.trim();
        if state.reconnect_fences.contains(key) || state.pending.contains_key(key) {
            return DiscoveryResult {
                ok: false,
                reason_code: "catalog_reconciliation_required".to_string(),
                tools: Vec::new(),
                source_revision: None,
                catalog_revision: None,
                audience_revision: None,
            };
        }

        let Some(snapshot) = state.partitions.get(key) else {
            return DiscoveryResult {
                ok: false,
                reason_code: "catalog_partition_missing".to_string(),
                tools: Vec::new(),
                source_revision: None,
                catalog_revision: None,
                audience_revision: None,
            };
        };

        DiscoveryResult {
            ok: true,
            reason_code: "ok".to_string(),
            tools: snapshot.tools.clone(),
            source_revision: Some(snapshot.source_revision),
            catalog_revision: Some(snapshot.catalog_revision.clone()),
            audience_revision: Some(snapshot.audience_revision),
        }
    }

    pub fn begin_reconnect(&self) -> serde_json::Value {
        let mut state = self.state.lock().expect("engine state lock");
        state.reconnect_fences = state.partitions.keys().cloned().collect();
        serde_json::json!({
            "reconnectFence": !state.reconnect_fences.is_empty(),
            "lastKnownAudienceRevision": state.last_known_audience_revision,
            "pendingInvalidationCount": state.pending.len(),
        })
    }

    pub fn mark_fenced(
        &self,
        partition_key: &str,
        audience_revision: i64,
        catalog_revision: &str,
        source_revision: i64,
    ) -> Option<CohortEntry> {
        let mut state = self.state.lock().expect("engine state lock");
        let key = partition_key.trim().to_string();
        if key.is_empty() {
            return None;
        }
        if !state.partitions.contains_key(&key)
            && !state.pending.contains_key(&key)
            && !state.cohort.contains_key(&key)
            && tracked_partition_count(&state) >= self.max_partitions
        {
            return None;
        }
        let entry = CohortEntry {
            partition_key: key.clone(),
            outcome: CohortOutcome::Fenced,
            audience_revision,
            catalog_revision: catalog_revision.trim().to_string(),
            source_revision,
            at: now_rfc3339(),
        };
        state.reconnect_fences.insert(key.clone());
        state.cohort.insert(key, entry.clone());
        Some(entry)
    }

    pub fn mark_disconnected(
        &self,
        partition_key: &str,
        audience_revision: i64,
    ) -> Option<CohortEntry> {
        let mut state = self.state.lock().expect("engine state lock");
        let key = partition_key.trim().to_string();
        if key.is_empty() {
            return None;
        }
        if !state.partitions.contains_key(&key)
            && !state.pending.contains_key(&key)
            && !state.cohort.contains_key(&key)
            && tracked_partition_count(&state) >= self.max_partitions
        {
            return None;
        }
        let entry = CohortEntry {
            partition_key: key.clone(),
            outcome: CohortOutcome::Disconnected,
            audience_revision,
            catalog_revision: String::new(),
            source_revision: -1,
            at: now_rfc3339(),
        };
        state.cohort.insert(key, entry.clone());
        Some(entry)
    }

    pub fn remove_partition(&self, partition_key: &str) -> bool {
        let mut state = self.state.lock().expect("engine state lock");
        let key = partition_key.trim();
        let removed = state.partitions.remove(key).is_some();
        state.pending.remove(key);
        state.cohort.remove(key);
        state.reconnect_fences.remove(key);
        state.ui_observed_revisions.remove(key);
        removed
    }

    pub fn purge_all(&self) {
        let mut state = self.state.lock().expect("engine state lock");
        state.partitions.clear();
        state.pending.clear();
        state.cohort.clear();
        state.in_flight.clear();
        state.reconnect_fences.clear();
        state.last_known_audience_revision = -1;
        state.ui_observed_revisions.clear();
    }

    pub fn restore_partition(&self, snapshot: CatalogSnapshot) -> RefreshResult {
        let expected_digest = crate::model::digest_catalog_snapshot(
            &snapshot.partition_key,
            snapshot.source_revision,
            &snapshot.catalog_revision,
            snapshot.audience_revision,
            &snapshot.tools,
        );
        if snapshot.tool_count != snapshot.tools.len() || snapshot.digest != expected_digest {
            return RefreshResult {
                outcome: RefreshOutcomeKind::RejectedConflict,
                partition_key: snapshot.partition_key,
                snapshot: None,
                retained: None,
            };
        }
        self.replace_partition(
            &snapshot.partition_key,
            CatalogFetchedSnapshot {
                source_revision: snapshot.source_revision,
                catalog_revision: snapshot.catalog_revision.clone(),
                audience_revision: snapshot.audience_revision,
                tools: snapshot.tools.clone(),
            },
        )
    }

    pub fn ui_observed_revision(&self) -> i64 {
        self.state
            .lock()
            .expect("engine state lock")
            .ui_observed_revisions
            .values()
            .copied()
            .max()
            .unwrap_or(-1)
    }

    pub fn observe_ui_revision(&self, partition_key: &str) -> Option<i64> {
        let mut state = self.state.lock().expect("engine state lock");
        let key = partition_key.trim().to_string();
        if state.pending.contains_key(&key) || state.reconnect_fences.contains(&key) {
            return None;
        }
        let revision = state.partitions.get(&key)?.audience_revision;
        state.ui_observed_revisions.insert(key, revision);
        Some(revision)
    }

    pub fn discovery_blocked(&self) -> bool {
        let state = self.state.lock().expect("engine state lock");
        !state.reconnect_fences.is_empty() || !state.pending.is_empty()
    }

    pub fn cohort_snapshot(&self) -> Vec<(String, CohortEntry)> {
        let state = self.state.lock().expect("engine state lock");
        let mut entries: Vec<(String, CohortEntry)> = state
            .cohort
            .iter()
            .map(|(key, value)| (key.clone(), value.clone()))
            .collect();
        entries.sort_by(|left, right| left.0.cmp(&right.0));
        entries
    }

    pub fn state(&self) -> serde_json::Value {
        let state = self.state.lock().expect("engine state lock");
        let mut cohort_entries: Vec<(String, CohortEntry)> = state
            .cohort
            .iter()
            .map(|(key, value)| (key.clone(), value.clone()))
            .collect();
        cohort_entries.sort_by(|left, right| left.0.cmp(&right.0));
        let cohort: Vec<serde_json::Value> = cohort_entries
            .into_iter()
            .map(|(key, entry)| {
                serde_json::json!({
                    "partitionKey": key,
                    "outcome": entry.outcome.wire_name(),
                    "audienceRevision": entry.audience_revision,
                    "catalogRevision": entry.catalog_revision,
                    "sourceRevision": entry.source_revision,
                    "at": entry.at,
                })
            })
            .collect();
        serde_json::json!({
            "schemaVersion": CATALOG_CONVERGENCE_SCHEMA,
            "partitionCount": state.partitions.len(),
            "inFlightCount": state.in_flight.len(),
            "pendingInvalidationCount": state.pending.len(),
            "reconnectFence": !state.reconnect_fences.is_empty(),
            "reconnectFenceCount": state.reconnect_fences.len(),
            "lastKnownAudienceRevision": state.last_known_audience_revision,
            "uiObservedRevision": state.ui_observed_revisions.values().copied().max().unwrap_or(-1),
            "cohort": cohort,
        })
    }
}

fn now_rfc3339() -> String {
    OffsetDateTime::now_utc()
        .format(&Rfc3339)
        .unwrap_or_else(|_| "1970-01-01T00:00:00Z".to_string())
}
