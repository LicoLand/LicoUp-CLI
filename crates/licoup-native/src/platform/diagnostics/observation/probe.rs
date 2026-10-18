//! The bounded, switchable observation probe (contract C07).
//!
//! # Business-path guarantee
//!
//! [`ObservationProbe::begin`] and [`ObservationProbe::submit`] are the only
//! methods business code calls. They read no clock while the probe is off, they
//! never invoke a backend, and they never wait on one: a full buffer refuses the
//! newest record and counts the refusal. Backends run from
//! [`ObservationProbe::drain`], which the telemetry owner calls.
//! Contended buffer access also drops immediately, with a separate atomic count.
//!
//! # Budgets
//!
//! The buffer is bounded by [`ObservationProbeConfig::buffer_capacity`], the
//! sample rate by [`SamplingBudget`], and what may be recorded by
//! [`PrivacyBudget`]. A refused record always increments exactly one counted
//! reason.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, TryLockError};

use super::backend::ObservationTelemetryBackend;
use super::correlation::CorrelationIds;
use super::privacy::{PrivacyBudget, PrivacyViolation};
use super::segment::{ActiveObservationSegment, ObservationPhase, ObservationSegmentRecord};

#[cfg(test)]
#[path = "probe_tests.rs"]
mod tests;

/// Reads the probe clock, in microseconds.
///
/// The clock is injected so probes stay deterministic under test and so the
/// probe never depends on a wall-clock source it does not own.
/// Implementations must be nonblocking and must not panic.
pub type ObservationClock = Arc<dyn Fn() -> u64 + Send + Sync>;

const MICROSECONDS_PER_SECOND: u64 = 1_000_000;

/// How many records may be sampled per window.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SamplingBudget {
    /// Records accepted per window. `0` samples nothing.
    pub max_records_per_window: u64,
    /// Window length in microseconds; must be positive.
    pub window_micros: u64,
}

impl SamplingBudget {
    /// No budget: every eligible record is sampled.
    pub const UNLIMITED: Self = Self {
        max_records_per_window: u64::MAX,
        window_micros: MICROSECONDS_PER_SECOND,
    };

    /// A budget of `max_records_per_window` records per `window_micros`.
    pub const fn per_window(max_records_per_window: u64, window_micros: u64) -> Self {
        Self {
            max_records_per_window,
            window_micros,
        }
    }
}

impl Default for SamplingBudget {
    fn default() -> Self {
        Self::UNLIMITED
    }
}

/// Why a record was not queued.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ObservationDropReason {
    /// The bounded buffer was full.
    BufferFull,
    /// Another producer or drain already held the buffer lock.
    BufferContended,
    /// The sampling window's record budget was exhausted.
    SamplingBudgetExhausted,
    /// The privacy budget refused the record.
    PrivacyBudgetExceeded,
}

impl ObservationDropReason {
    /// The stable wire name of this reason.
    pub const fn wire(self) -> &'static str {
        match self {
            Self::BufferFull => "buffer_full",
            Self::BufferContended => "buffer_contended",
            Self::SamplingBudgetExhausted => "sampling_budget_exhausted",
            Self::PrivacyBudgetExceeded => "privacy_budget_exceeded",
        }
    }
}

/// What happened to one submitted record.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ObservationSubmitOutcome {
    /// Accepted into the bounded buffer.
    Queued,
    /// Refused; the reason is counted and the caller proceeds.
    Dropped(ObservationDropReason),
    /// The probe is switched off, so there was nothing to record.
    Disabled,
}

/// Counted refusals. Every refused record lands in exactly one bucket.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct ObservationDropCounts {
    /// Records refused because the bounded buffer was full.
    pub buffer_full: u64,
    /// Records refused rather than waiting for another buffer user.
    pub buffer_contended: u64,
    /// Records refused because the sampling window was exhausted.
    pub sampling_budget_exhausted: u64,
    /// Records refused by the privacy budget.
    pub privacy_budget_exceeded: u64,
    /// The most recent privacy refusal, named but not valued.
    pub last_privacy_violation: Option<PrivacyViolation>,
}

impl ObservationDropCounts {
    /// Total counted refusals.
    pub const fn total(&self) -> u64 {
        self.buffer_full
            .saturating_add(self.buffer_contended)
            .saturating_add(self.sampling_budget_exhausted)
            .saturating_add(self.privacy_budget_exceeded)
    }
}

/// Bounds of one probe instance.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ObservationProbeConfig {
    /// Bounded buffer capacity; must be positive.
    pub buffer_capacity: usize,
    /// Sampling budget per window.
    pub sampling: SamplingBudget,
    /// What may be recorded.
    pub privacy: PrivacyBudget,
}

impl ObservationProbeConfig {
    /// A bounded probe with [`SamplingBudget::UNLIMITED`] and
    /// [`PrivacyBudget::STANDARD`].
    pub const fn bounded(buffer_capacity: usize) -> Self {
        Self {
            buffer_capacity,
            sampling: SamplingBudget::UNLIMITED,
            privacy: PrivacyBudget::STANDARD,
        }
    }
}

/// Bounded, switchable segment probe.
///
/// Clone to share one probe; clones share the buffer, budgets, and counts.
#[derive(Clone)]
pub struct ObservationProbe {
    inner: Option<Arc<ProbeInner>>,
}

struct ProbeInner {
    config: ObservationProbeConfig,
    clock: ObservationClock,
    backend: Arc<dyn ObservationTelemetryBackend>,
    state: Mutex<ProbeState>,
    contended: AtomicU64,
}

#[derive(Default)]
struct ProbeState {
    buffer: VecDeque<ObservationSegmentRecord>,
    drops: ObservationDropCounts,
    sampled_in_window: u64,
    window_opened_micros: u64,
    window_open: bool,
}

impl ObservationProbe {
    /// A switched-off probe. It reads no clock and holds no buffer, so an off
    /// probe costs business code one `Option` check.
    pub const fn disabled() -> Self {
        Self { inner: None }
    }

    /// A bounded probe with a replaceable backend.
    ///
    /// # Panics
    ///
    /// Panics when `config.buffer_capacity` or `config.sampling.window_micros`
    /// is zero. Both are configuration errors on the telemetry owner's path,
    /// never on the business path.
    pub fn bounded(
        config: ObservationProbeConfig,
        clock: ObservationClock,
        backend: Arc<dyn ObservationTelemetryBackend>,
    ) -> Self {
        assert!(
            config.buffer_capacity > 0,
            "observation buffer capacity must be positive"
        );
        assert!(
            config.sampling.window_micros > 0,
            "observation sampling window must be positive"
        );
        Self {
            inner: Some(Arc::new(ProbeInner {
                config,
                clock,
                backend,
                state: Mutex::new(ProbeState {
                    buffer: VecDeque::with_capacity(config.buffer_capacity),
                    ..ProbeState::default()
                }),
                contended: AtomicU64::new(0),
            })),
        }
    }

    /// Whether this probe records anything.
    pub fn is_enabled(&self) -> bool {
        self.inner.is_some()
    }

    /// Opens a waiting or work segment, or returns `None` when switched off.
    ///
    /// The clock is read only when the probe is enabled.
    pub fn begin(
        &self,
        phase: ObservationPhase,
        correlation: CorrelationIds,
    ) -> Option<ActiveObservationSegment> {
        let inner = self.inner.as_ref()?;
        let started_micros = (inner.clock)();
        Some(ActiveObservationSegment::open(
            phase,
            correlation,
            started_micros,
            Arc::clone(&inner.clock),
        ))
    }

    /// Closes a segment and submits it.
    pub fn complete(&self, segment: ActiveObservationSegment) -> ObservationSubmitOutcome {
        if !self.is_enabled() {
            return ObservationSubmitOutcome::Disabled;
        }
        self.submit(segment.finish())
    }

    /// Submits one record. Never waits for the buffer lock or calls the backend.
    pub fn submit(&self, record: ObservationSegmentRecord) -> ObservationSubmitOutcome {
        let Some(inner) = self.inner.as_ref() else {
            return ObservationSubmitOutcome::Disabled;
        };
        let violation = inner.config.privacy.record_violation(&record);
        // Read the caller's clock outside the lock, just as the backend runs
        // outside it. No injected callback may hold up another producer.
        let now = if violation.is_none() {
            (inner.clock)()
        } else {
            0
        };
        let mut state = match inner.state.try_lock() {
            Ok(state) => state,
            Err(TryLockError::Poisoned(poisoned)) => poisoned.into_inner(),
            Err(TryLockError::WouldBlock) => {
                inner.contended.fetch_add(1, Ordering::Relaxed);
                return ObservationSubmitOutcome::Dropped(ObservationDropReason::BufferContended);
            }
        };
        if let Some(violation) = violation {
            state.drops.privacy_budget_exceeded =
                state.drops.privacy_budget_exceeded.saturating_add(1);
            state.drops.last_privacy_violation = Some(violation);
            return ObservationSubmitOutcome::Dropped(ObservationDropReason::PrivacyBudgetExceeded);
        }
        state.advance_window(now, inner.config.sampling);
        if state.sampled_in_window >= inner.config.sampling.max_records_per_window {
            state.drops.sampling_budget_exhausted =
                state.drops.sampling_budget_exhausted.saturating_add(1);
            return ObservationSubmitOutcome::Dropped(
                ObservationDropReason::SamplingBudgetExhausted,
            );
        }
        if state.buffer.len() >= inner.config.buffer_capacity {
            state.drops.buffer_full = state.drops.buffer_full.saturating_add(1);
            return ObservationSubmitOutcome::Dropped(ObservationDropReason::BufferFull);
        }
        state.sampled_in_window += 1;
        state.buffer.push_back(record);
        ObservationSubmitOutcome::Queued
    }

    /// Hands at most `max_records` queued records to the backend and returns how
    /// many were emitted.
    ///
    /// The backend runs outside the probe lock, so a slow backend delays later
    /// drains only; a backend that submits re-entrantly cannot deadlock.
    pub fn drain(&self, max_records: usize) -> usize {
        let Some(inner) = self.inner.as_ref() else {
            return 0;
        };
        let batch: Vec<ObservationSegmentRecord> = {
            let mut state = inner.lock();
            let take = max_records.min(state.buffer.len());
            state.buffer.drain(..take).collect()
        };
        for record in &batch {
            inner.backend.emit(record);
        }
        batch.len()
    }

    /// Queued records not yet drained.
    pub fn pending(&self) -> usize {
        self.inner
            .as_ref()
            .map_or(0, |inner| inner.lock().buffer.len())
    }

    /// Counted refusals so far.
    pub fn drop_counts(&self) -> ObservationDropCounts {
        self.inner
            .as_ref()
            .map_or_else(ObservationDropCounts::default, |inner| {
                let mut drops = inner.lock().drops;
                drops.buffer_contended = inner.contended.load(Ordering::Relaxed);
                drops
            })
    }
}

impl std::fmt::Debug for ObservationProbe {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ObservationProbe")
            .field("enabled", &self.is_enabled())
            .field("pending", &self.pending())
            .field("drops", &self.drop_counts())
            .finish()
    }
}

impl ProbeInner {
    /// A poisoned lock still holds a consistent buffer, so recovering is safe.
    fn lock(&self) -> MutexGuard<'_, ProbeState> {
        self.state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}

impl ProbeState {
    fn advance_window(&mut self, now_micros: u64, sampling: SamplingBudget) {
        let expired = !self.window_open
            || now_micros.saturating_sub(self.window_opened_micros) >= sampling.window_micros;
        if expired {
            self.window_open = true;
            self.window_opened_micros = now_micros;
            self.sampled_in_window = 0;
        }
    }
}
