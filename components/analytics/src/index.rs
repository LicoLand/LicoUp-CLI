//! The observation index: identity, revision, retraction and cumulative series.
//!
//! This is a *view*, not a ledger. It holds what has been reported so the
//! consumer can deduplicate, supersede, retract and difference it; the durable
//! facts are the core's ([`crate::facts`]). Four rules are enforced here, and
//! each is the difference between a total and a wrong total:
//!
//! - **Identity is `(source, epoch, observationId)`.** The source is the
//!   host-bound one, so the same id from two sources stays two observations and
//!   a replayed payload from one source stays one.
//! - **A correction replaces.** A strictly higher revision supersedes; an equal
//!   or lower revision is [`IngestOutcome::Stale`] and changes nothing, so
//!   out-of-order delivery cannot overwrite a newer reading.
//! - **A retraction withdraws one observation.** It records a tombstone at its
//!   revision, so a replayed upsert cannot resurrect what was withdrawn; only a
//!   strictly higher revision can report that identity again.
//! - **A cumulative counter is differenced inside one series.** Readings are
//!   ordered by observation time, not arrival time, and a counter that went down
//!   is a reset — never a negative amount of work.

use licoup_extension_contracts::ApplicationFailure;
use licoup_extension_contracts::usage::{
    ExactNumber, MetricValue, SeriesOrigin, Temporality, cumulative_delta,
};
use licoup_usage_source_sdk::binding::{BoundObservation, BoundObservationKey};
use std::collections::BTreeMap;

use crate::refusal;

/// What one ingestion did.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum IngestOutcome {
    /// A new observation.
    Inserted,
    /// A higher revision replaced the previous one.
    Corrected,
    /// An equal or lower revision, or a replay: nothing changed.
    Stale,
    /// The observation was withdrawn.
    Retracted,
    /// A retraction for an already-withdrawn identity.
    AlreadyRetracted,
}

/// The growth of a cumulative counter, or why there is none.
#[derive(Clone, Debug, PartialEq)]
pub enum CounterStep {
    /// The first reading of this series: there is nothing to subtract from.
    FirstReading,
    /// The growth since the previous reading of the same series.
    Delta(ExactNumber),
    /// The counter went down: the series restarted, and the current reading is
    /// the new baseline. The difference across a reset is not a quantity of
    /// anything.
    Reset {
        previous_origin: SeriesOrigin,
        new_origin: SeriesOrigin,
    },
    /// The reading carries no number, so no delta can be computed. It is not a
    /// zero.
    UnknownReading,
}

/// The observations this consumer has seen, keyed by host-bound identity.
#[derive(Clone, Debug, Default)]
pub struct ObservationIndex {
    entries: BTreeMap<BoundObservationKey, BoundObservation>,
    retracted: BTreeMap<BoundObservationKey, u32>,
    series: BTreeMap<SeriesKey, BTreeMap<ReadingOrder, MetricValue>>,
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
struct SeriesKey {
    source_ref: String,
    source_epoch: String,
    metric: String,
    interval_start: Option<String>,
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
struct ReadingOrder {
    observed_at: String,
    observation_id: String,
}

impl ObservationIndex {
    pub fn new() -> Self {
        Self::default()
    }

    /// Ingest one bound observation.
    pub fn ingest(&mut self, bound: BoundObservation) -> IngestOutcome {
        let key = bound.key();
        let revision = bound.revision();
        if bound.observation.operation == licoup_extension_contracts::usage::UsageOperation::Retract
        {
            if self
                .retracted
                .get(&key)
                .is_some_and(|seen| *seen >= revision)
            {
                return IngestOutcome::AlreadyRetracted;
            }
            self.retracted.insert(key.clone(), revision);
            self.entries.remove(&key);
            return IngestOutcome::Retracted;
        }
        if self
            .retracted
            .get(&key)
            .is_some_and(|withdrawn| revision <= *withdrawn)
        {
            return IngestOutcome::Stale;
        }
        match self.entries.get(&key) {
            None => {
                self.entries.insert(key, bound);
                IngestOutcome::Inserted
            }
            Some(existing) if revision > existing.revision() => {
                self.entries.insert(key, bound);
                IngestOutcome::Corrected
            }
            Some(_) => IngestOutcome::Stale,
        }
    }

    pub fn get(&self, key: &BoundObservationKey) -> Option<&BoundObservation> {
        self.entries.get(key)
    }

    /// The revision at which this identity was withdrawn, if it was.
    pub fn retracted_revision(&self, key: &BoundObservationKey) -> Option<u32> {
        self.retracted.get(key).copied()
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Every live observation, in identity order.
    pub fn observations(&self) -> impl Iterator<Item = &BoundObservation> {
        self.entries.values()
    }

    /// The live observations that carry `metric` for one scope.
    pub fn observations_for(&self, scope_ref: &str, metric: &str) -> Vec<&BoundObservation> {
        self.entries
            .values()
            .filter(|bound| {
                bound.scope_ref() == scope_ref && bound.observation.metrics.contains_key(metric)
            })
            .collect()
    }

    /// Every scope this index has seen.
    pub fn scopes(&self) -> Vec<&str> {
        let mut scopes: Vec<&str> = self
            .entries
            .values()
            .map(BoundObservation::scope_ref)
            .collect();
        scopes.sort_unstable();
        scopes.dedup();
        scopes
    }

    /// Difference one cumulative reading against the previous reading of the
    /// same series.
    ///
    /// The predecessor is chosen by observation time, so a point that arrives
    /// late is inserted in its right place rather than read as a reset. The
    /// result is stable while no new reading arrives: reading the same series
    /// twice returns the same step, which is what lets a redraw compute a delta
    /// without re-reading a source.
    pub fn counter_step(
        &mut self,
        bound: &BoundObservation,
        metric: &str,
    ) -> Result<CounterStep, ApplicationFailure> {
        let Some(reading) = bound.observation.metrics.get(metric) else {
            return Err(refusal("analytics_metric_missing").with_field("metrics"));
        };
        if reading.temporality != Temporality::Cumulative {
            return Err(refusal("usage_temporality_not_cumulative").with_field("temporality"));
        }
        let key = SeriesKey {
            source_ref: bound.binding.source_ref.clone(),
            source_epoch: bound.binding.source_epoch.clone(),
            metric: metric.to_owned(),
            interval_start: bound.observation.interval_start.clone(),
        };
        let order = ReadingOrder {
            observed_at: bound.observation.observed_at.clone(),
            observation_id: bound.observation.observation_id.clone(),
        };
        let readings = self.series.entry(key).or_default();
        readings.insert(order.clone(), reading.clone());
        let predecessor = readings
            .range(..order.clone())
            .next_back()
            .map(|(_, value)| value.clone());
        let Some(previous) = predecessor else {
            return Ok(CounterStep::FirstReading);
        };
        let origin = SeriesOrigin::of(&bound.observation, metric);
        match cumulative_delta(&previous, &origin, reading, &origin) {
            Ok(delta) => Ok(CounterStep::Delta(delta)),
            Err(failure) if failure.code == "usage_cumulative_regressed" => {
                Ok(CounterStep::Reset {
                    previous_origin: origin.clone(),
                    new_origin: origin,
                })
            }
            Err(failure) if failure.code == "usage_metric_unknown" => {
                Ok(CounterStep::UnknownReading)
            }
            Err(failure) => Err(failure),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use licoup_extension_contracts::usage::{
        MetricValue, Quality, UsageObservation, UsageOperation,
    };
    use licoup_usage_source_sdk::binding::SourceBinding;
    use std::collections::BTreeMap;

    fn binding() -> SourceBinding {
        SourceBinding::new(
            "source:example.agent#1",
            "example.agent",
            "instance-1",
            1,
            "epoch-1",
            ["scope-1"],
        )
        .expect("binding")
    }

    fn observation(
        id: &str,
        revision: u32,
        metric: &str,
        value: &str,
        temporality: Temporality,
        interval_start: Option<&str>,
        observed_at: &str,
    ) -> BoundObservation {
        let observation = UsageObservation {
            schema: licoup_extension_contracts::wire::USAGE.to_owned(),
            observation_id: id.to_owned(),
            revision,
            operation: UsageOperation::Upsert,
            source_epoch: "epoch-1".to_owned(),
            scope_ref: "scope-1".to_owned(),
            observed_at: observed_at.to_owned(),
            interval_start: interval_start.map(str::to_owned),
            metrics: BTreeMap::from([(
                metric.to_owned(),
                MetricValue {
                    value: Some(value.to_owned()),
                    unit: "tokens".to_owned(),
                    temporality,
                    quality: Quality::Reported,
                    includes: Vec::new(),
                },
            )]),
            cost: None,
        };
        binding()
            .bind_observation(observation, None)
            .expect("bound")
    }

    fn retraction(id: &str, revision: u32) -> BoundObservation {
        let observation = UsageObservation {
            schema: licoup_extension_contracts::wire::USAGE.to_owned(),
            observation_id: id.to_owned(),
            revision,
            operation: UsageOperation::Retract,
            source_epoch: "epoch-1".to_owned(),
            scope_ref: "scope-1".to_owned(),
            observed_at: "2026-09-21T00:30:00Z".to_owned(),
            interval_start: None,
            metrics: BTreeMap::new(),
            cost: None,
        };
        binding()
            .bind_observation(observation, None)
            .expect("bound")
    }

    #[test]
    fn a_correction_replaces_and_a_replay_does_not() {
        let mut index = ObservationIndex::new();
        assert_eq!(
            index.ingest(observation(
                "obs-1",
                1,
                "licoup.tokens.input",
                "100",
                Temporality::Delta,
                None,
                "2026-09-21T00:00:00Z"
            )),
            IngestOutcome::Inserted
        );
        assert_eq!(
            index.ingest(observation(
                "obs-1",
                2,
                "licoup.tokens.input",
                "120",
                Temporality::Delta,
                None,
                "2026-09-21T00:00:00Z"
            )),
            IngestOutcome::Corrected
        );
        assert_eq!(
            index.ingest(observation(
                "obs-1",
                1,
                "licoup.tokens.input",
                "100",
                Temporality::Delta,
                None,
                "2026-09-21T00:00:00Z"
            )),
            IngestOutcome::Stale
        );
        assert_eq!(index.len(), 1);
        let key = index.observations().next().expect("one observation").key();
        assert_eq!(
            index.get(&key).expect("kept").observation.metrics["licoup.tokens.input"]
                .value
                .as_deref(),
            Some("120")
        );
    }

    #[test]
    fn a_retraction_withdraws_and_a_replay_cannot_resurrect() {
        let mut index = ObservationIndex::new();
        index.ingest(observation(
            "obs-1",
            1,
            "licoup.tokens.input",
            "100",
            Temporality::Delta,
            None,
            "2026-09-21T00:00:00Z",
        ));
        assert_eq!(
            index.ingest(retraction("obs-1", 2)),
            IngestOutcome::Retracted
        );
        assert!(index.is_empty());
        assert_eq!(
            index.ingest(observation(
                "obs-1",
                1,
                "licoup.tokens.input",
                "100",
                Temporality::Delta,
                None,
                "2026-09-21T00:00:00Z"
            )),
            IngestOutcome::Stale
        );
        assert_eq!(
            index.ingest(retraction("obs-1", 2)),
            IngestOutcome::AlreadyRetracted
        );
        assert_eq!(
            index.ingest(observation(
                "obs-1",
                3,
                "licoup.tokens.input",
                "130",
                Temporality::Delta,
                None,
                "2026-09-21T00:00:00Z"
            )),
            IngestOutcome::Inserted,
            "a strictly higher revision is a new report, not a resurrection of the old claim"
        );
    }

    #[test]
    fn cumulative_readings_are_differenced_in_time_order() {
        let mut index = ObservationIndex::new();
        let first = observation(
            "obs-1",
            1,
            "licoup.tokens.input",
            "100",
            Temporality::Cumulative,
            Some("2026-09-21T00:00:00Z"),
            "2026-09-21T00:05:00Z",
        );
        assert_eq!(
            index
                .counter_step(&first, "licoup.tokens.input")
                .expect("step"),
            CounterStep::FirstReading
        );
        let second = observation(
            "obs-2",
            1,
            "licoup.tokens.input",
            "175",
            Temporality::Cumulative,
            Some("2026-09-21T00:00:00Z"),
            "2026-09-21T00:10:00Z",
        );
        assert_eq!(
            index
                .counter_step(&second, "licoup.tokens.input")
                .expect("step"),
            CounterStep::Delta(ExactNumber::parse("75").expect("75"))
        );

        // The same step read again is the same step: a redraw is not a re-scan.
        assert_eq!(
            index
                .counter_step(&second, "licoup.tokens.input")
                .expect("step"),
            CounterStep::Delta(ExactNumber::parse("75").expect("75"))
        );

        // A late point is inserted in its right place instead of read as a reset.
        let between = observation(
            "obs-3",
            1,
            "licoup.tokens.input",
            "140",
            Temporality::Cumulative,
            Some("2026-09-21T00:00:00Z"),
            "2026-09-21T00:07:00Z",
        );
        assert_eq!(
            index
                .counter_step(&between, "licoup.tokens.input")
                .expect("step"),
            CounterStep::Delta(ExactNumber::parse("40").expect("40"))
        );
    }

    #[test]
    fn a_counter_that_went_down_is_a_reset_not_negative_usage() {
        let mut index = ObservationIndex::new();
        let before = observation(
            "obs-1",
            1,
            "licoup.tokens.input",
            "100",
            Temporality::Cumulative,
            Some("2026-09-21T00:00:00Z"),
            "2026-09-21T00:05:00Z",
        );
        index
            .counter_step(&before, "licoup.tokens.input")
            .expect("step");
        let after = observation(
            "obs-2",
            1,
            "licoup.tokens.input",
            "5",
            Temporality::Cumulative,
            Some("2026-09-21T00:00:00Z"),
            "2026-09-21T00:10:00Z",
        );
        assert!(matches!(
            index
                .counter_step(&after, "licoup.tokens.input")
                .expect("step"),
            CounterStep::Reset { .. }
        ));
        let restarted = observation(
            "obs-3",
            1,
            "licoup.tokens.input",
            "20",
            Temporality::Cumulative,
            Some("2026-09-21T01:00:00Z"),
            "2026-09-21T01:05:00Z",
        );
        assert_eq!(
            index
                .counter_step(&restarted, "licoup.tokens.input")
                .expect("step"),
            CounterStep::FirstReading,
            "a new origin starts a new series"
        );
    }

    #[test]
    fn an_unknown_reading_is_not_a_zero_delta() {
        let mut index = ObservationIndex::new();
        let first = observation(
            "obs-1",
            1,
            "licoup.tokens.input",
            "100",
            Temporality::Cumulative,
            Some("2026-09-21T00:00:00Z"),
            "2026-09-21T00:05:00Z",
        );
        index
            .counter_step(&first, "licoup.tokens.input")
            .expect("step");
        let mut unknown = observation(
            "obs-2",
            1,
            "licoup.tokens.input",
            "0",
            Temporality::Cumulative,
            Some("2026-09-21T00:00:00Z"),
            "2026-09-21T00:10:00Z",
        );
        let metric = unknown
            .observation
            .metrics
            .get_mut("licoup.tokens.input")
            .expect("metric");
        metric.value = None;
        metric.quality = Quality::Unknown;
        assert_eq!(
            index
                .counter_step(&unknown, "licoup.tokens.input")
                .expect("step"),
            CounterStep::UnknownReading
        );
    }

    #[test]
    fn observations_for_a_scope_are_filtered_by_metric() {
        let mut index = ObservationIndex::new();
        index.ingest(observation(
            "obs-1",
            1,
            "licoup.tokens.input",
            "100",
            Temporality::Delta,
            None,
            "2026-09-21T00:00:00Z",
        ));
        assert_eq!(
            index
                .observations_for("scope-1", "licoup.tokens.input")
                .len(),
            1
        );
        assert!(
            index
                .observations_for("scope-1", "licoup.tokens.output")
                .is_empty()
        );
        assert!(
            index
                .observations_for("scope-2", "licoup.tokens.input")
                .is_empty()
        );
    }
}
