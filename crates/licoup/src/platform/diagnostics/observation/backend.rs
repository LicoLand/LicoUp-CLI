//! Replaceable telemetry backends (contract C07).
//!
//! The backend is the only replaceable half of the port. Backends are invoked
//! exclusively from [`super::probe::ObservationProbe::drain`], so a backend can
//! never delay or fail a business path; refused records are counted by the probe
//! instead.
//!
//! Two backends ship here and both reuse owners that already exist:
//!
//! * [`DisabledObservationTelemetryBackend`] keeps the diagnostic default off;
//! * [`LogObservationTelemetryBackend`] writes through the existing `log` sink
//!   that the native host already initializes with `env_logger`. It opens no
//!   file, no socket, and no second log sink.

use log::Level;

use super::privacy::PrivacyBudget;
use super::segment::ObservationSegmentRecord;

/// Log target used by [`LogObservationTelemetryBackend`].
pub const OBSERVATION_LOG_TARGET: &str = "licoup.observation";

/// Consumer of drained observation records.
///
/// Implementations must not block for long: a slow backend delays the next
/// drain, and the buffer then refuses records instead of stalling its caller.
pub trait ObservationTelemetryBackend: Send + Sync {
    /// Consumes one drained record.
    fn emit(&self, record: &ObservationSegmentRecord);
}

/// The diagnostic default: consume and discard.
#[derive(Clone, Copy, Debug, Default)]
pub struct DisabledObservationTelemetryBackend;

impl ObservationTelemetryBackend for DisabledObservationTelemetryBackend {
    fn emit(&self, _record: &ObservationSegmentRecord) {}
}

/// Forwards records to the existing `log`/`env_logger` sink.
///
/// The line carries the phase, the wait/work kind, the timing, and the
/// correlation ids — trace-grade, high-cardinality data that belongs in logs.
/// Values must be opaque, non-secret ids supplied by their owning domain, not
/// user content or credentials. The standard privacy budget is applied here too
/// so direct backend calls cannot bypass path, size, or link validation.
#[derive(Clone, Copy, Debug, Default)]
pub struct LogObservationTelemetryBackend;

impl LogObservationTelemetryBackend {
    /// Renders the single line this backend writes.
    ///
    /// Exposed so the line's shape is testable without installing a process-wide
    /// logger; the fields are exactly the ones a reader needs to separate
    /// waiting from work and to join a segment to its causal context.
    pub fn render(record: &ObservationSegmentRecord) -> String {
        if PrivacyBudget::STANDARD.record_violation(record).is_some() {
            return "observation_record_redacted".to_owned();
        }
        // Keep the actual link targets: a count alone loses the causal join.
        let Ok(links) = serde_json::to_string(&record.links) else {
            return "observation_record_redacted".to_owned();
        };
        format!(
            "phase={} kind={} started_us={} duration_us={} links={} {} span_links={}",
            record.phase.wire(),
            record.kind().wire(),
            record.started_micros,
            record.duration_micros,
            record.links.len(),
            record.correlation,
            links,
        )
    }
}

impl ObservationTelemetryBackend for LogObservationTelemetryBackend {
    fn emit(&self, record: &ObservationSegmentRecord) {
        if !log::log_enabled!(target: OBSERVATION_LOG_TARGET, Level::Debug) {
            return;
        }
        log::debug!(target: OBSERVATION_LOG_TARGET, "{}", Self::render(record));
    }
}
