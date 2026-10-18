//! Adaptive refresh discipline for provider quota snapshots.
//!
//! One scheduler tick selects due providers: a 2-5 minute cadence while the
//! client is active and about 30 minutes when idle. A single-flight
//! coalescing guard ensures only one refresh runs at a time, consecutive
//! failures back off per provider with a cap, the last good snapshot is
//! retained and marked stale past `staleAfterSeconds`, and missing reset
//! timestamps are backfilled from the cached snapshot.

use super::contract::{QuotaStatus, QuotaWindow};
use std::sync::atomic::{AtomicBool, Ordering};
use time::{Duration, OffsetDateTime, format_description::well_known::Rfc3339};

/// Active-client cadence; 180s sits inside the documented 2-5 minute band.
pub(super) const ACTIVE_INTERVAL_SECONDS: u64 = 180;
/// Idle-client cadence: about thirty minutes.
pub(super) const IDLE_INTERVAL_SECONDS: u64 = 1800;
pub(super) const FAILURE_BACKOFF_BASE_SECONDS: u64 = 60;
pub(super) const FAILURE_BACKOFF_CAP_SECONDS: u64 = 1800;

/// Single-flight coalescing guard. While one refresh holds the permit, other
/// ticks serve the retained snapshot instead of starting a parallel fetch.
pub(super) struct RefreshGate {
    occupied: AtomicBool,
}

pub(super) struct RefreshPermit<'a> {
    occupied: &'a AtomicBool,
}

impl RefreshGate {
    pub(super) const fn new() -> Self {
        Self {
            occupied: AtomicBool::new(false),
        }
    }

    pub(super) fn try_acquire(&self) -> Option<RefreshPermit<'_>> {
        self.occupied
            .compare_exchange(false, true, Ordering::Acquire, Ordering::Relaxed)
            .ok()
            .map(|_| RefreshPermit {
                occupied: &self.occupied,
            })
    }
}

impl Drop for RefreshPermit<'_> {
    fn drop(&mut self) {
        self.occupied.store(false, Ordering::Release);
    }
}

pub(super) fn cadence_seconds(client_active: bool) -> u64 {
    if client_active {
        ACTIVE_INTERVAL_SECONDS
    } else {
        IDLE_INTERVAL_SECONDS
    }
}

/// A provider is due when no refresh is scheduled yet or its scheduled time
/// has arrived. Unparseable retained timestamps fail open into a refresh.
pub(super) fn is_due(next_due_at: Option<&str>, now: OffsetDateTime) -> bool {
    let Some(next_due_at) = next_due_at else {
        return true;
    };
    match OffsetDateTime::parse(next_due_at, &Rfc3339) {
        Ok(due) => now >= due,
        Err(_) => true,
    }
}

pub(super) fn next_due_after_success(now: OffsetDateTime, client_active: bool) -> String {
    format_rfc3339(now + Duration::seconds(cadence_seconds(client_active) as i64))
}

/// Consecutive-failure backoff: base interval doubling per failure, capped.
pub(super) fn next_due_after_failure(now: OffsetDateTime, consecutive_failures: u32) -> String {
    let shift = consecutive_failures.min(10);
    let backoff = FAILURE_BACKOFF_BASE_SECONDS
        .saturating_mul(1_u64 << shift)
        .min(FAILURE_BACKOFF_CAP_SECONDS);
    format_rfc3339(now + Duration::seconds(backoff as i64))
}

/// Snapshots captured longer than `staleAfterSeconds` ago are stale; the
/// retained capture age stays visible through `capturedAt`.
pub(super) fn status_for(
    snapshot_captured_at: &str,
    stale_after_seconds: u64,
    now: OffsetDateTime,
) -> QuotaStatus {
    match OffsetDateTime::parse(snapshot_captured_at, &Rfc3339) {
        Ok(captured) => {
            if now - captured > Duration::seconds(stale_after_seconds as i64) {
                QuotaStatus::Stale
            } else {
                QuotaStatus::Live
            }
        }
        Err(_) => QuotaStatus::Stale,
    }
}

/// Backfill missing reset timestamps from the cached snapshot's windows,
/// matching by label so provider window ordering never corrupts the mapping.
pub(super) fn backfill_reset_timestamps(windows: &mut [QuotaWindow], cached: &[QuotaWindow]) {
    for window in windows.iter_mut() {
        if window.resets_at.is_some() {
            continue;
        }
        window.resets_at = cached
            .iter()
            .find(|candidate| candidate.label == window.label)
            .and_then(|candidate| candidate.resets_at.clone());
    }
}

pub(super) fn format_rfc3339(value: OffsetDateTime) -> String {
    value
        .format(&Rfc3339)
        .unwrap_or_else(|_| "1970-01-01T00:00:00Z".to_owned())
}
