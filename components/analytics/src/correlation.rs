//! One call, several reports: settle once, or say that you cannot tell.
//!
//! An Agent adapter, a model gateway and the host's own execution graph may all
//! report usage for the same invocation. Adding them would charge the user three
//! times for one call; picking one silently would hide the disagreement. C11
//! resolves this with two facts, and this module requires one of them:
//!
//! - **A logical measurement**, issued by the host, says "these reports are one
//!   call". Reports that share it settle once.
//! - **An explicit priority** says "when these sources both report a call,
//!   believe this one". It is host configuration, not a heuristic: this module
//!   never guesses from a source's name, arrival order or claimed quality.
//!
//! When neither is available and more than one source reports the same scope and
//! metric, the outcome is [`Reconciliation::Ambiguous`]: the reports are kept
//! for display with their provenance and no definite amount is produced. That is
//! the honest answer — the alternative is a confident double charge.
//!
//! A single source reporting several observations for one scope is not a
//! conflict: those are separate calls, and each settles on its own identity.

use licoup_usage_source_sdk::binding::BoundObservation;
use std::collections::{BTreeMap, BTreeSet};

/// The explicit order in which sources are believed.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct CorrelationPolicy {
    priority: Vec<String>,
}

impl CorrelationPolicy {
    /// No correlation: only a shared logical measurement resolves a conflict.
    pub fn none() -> Self {
        Self::default()
    }

    /// Believe sources in this order. The list is host configuration; a source
    /// that is not on it has no rank and cannot be preferred.
    pub fn explicit(priority: impl IntoIterator<Item = impl Into<String>>) -> Self {
        Self {
            priority: priority.into_iter().map(Into::into).collect(),
        }
    }

    /// This source's rank, lowest first, or `None` when it has none.
    pub fn rank(&self, source_ref: &str) -> Option<usize> {
        self.priority.iter().position(|source| source == source_ref)
    }

    /// Whether every source in `sources` has a rank.
    ///
    /// A partial order is not an order: if one of three reporters is unranked,
    /// preferring the other two would be a guess about the third.
    pub fn covers<'a>(&self, sources: impl IntoIterator<Item = &'a str>) -> bool {
        sources
            .into_iter()
            .all(|source| self.rank(source).is_some())
    }

    pub fn priority(&self) -> &[String] {
        &self.priority
    }
}

/// What one scope and metric reconciled to.
#[derive(Clone, Debug, PartialEq)]
pub enum Reconciliation {
    /// One measurement: the authoritative report and the others that agree it
    /// happened.
    Settled {
        scope_ref: String,
        metric: String,
        /// Boxed because the variant is carried through lists, not stored in
        /// bulk: indirection here keeps the enum small without hiding the value.
        authoritative: Box<BoundObservation>,
        corroborating: Vec<BoundObservation>,
    },
    /// Reports that cannot be attributed to one measurement or to distinct
    /// ones. Nothing is settled and nothing is summed.
    Ambiguous {
        scope_ref: String,
        metric: String,
        candidates: Vec<BoundObservation>,
    },
}

impl Reconciliation {
    pub fn scope_ref(&self) -> &str {
        match self {
            Self::Settled { scope_ref, .. } | Self::Ambiguous { scope_ref, .. } => scope_ref,
        }
    }

    pub fn metric(&self) -> &str {
        match self {
            Self::Settled { metric, .. } | Self::Ambiguous { metric, .. } => metric,
        }
    }
}

/// Reconcile every observation into settled measurements or ambiguities.
pub fn reconcile(
    observations: &[BoundObservation],
    policy: &CorrelationPolicy,
) -> Vec<Reconciliation> {
    let mut groups: BTreeMap<(String, String), Vec<&BoundObservation>> = BTreeMap::new();
    for observation in observations {
        for metric in observation.observation.metrics.keys() {
            groups
                .entry((observation.scope_ref().to_owned(), metric.clone()))
                .or_default()
                .push(observation);
        }
    }
    let mut reconciled = Vec::new();
    for ((scope_ref, metric), group) in groups {
        reconciled.extend(settle_group(&scope_ref, &metric, &group, policy));
    }
    reconciled
}

fn settle_group(
    scope_ref: &str,
    metric: &str,
    group: &[&BoundObservation],
    policy: &CorrelationPolicy,
) -> Vec<Reconciliation> {
    let sources: BTreeSet<&str> = group.iter().map(|bound| bound.source_ref()).collect();
    if sources.len() == 1 {
        // One source reporting several observations is several calls, not a
        // conflict — unless the reports claim one host-issued measurement, in
        // which case they are one call and the highest revision is what it says.
        let mut linked: BTreeMap<&str, Vec<&BoundObservation>> = BTreeMap::new();
        let mut unlinked: Vec<&BoundObservation> = Vec::new();
        for bound in group {
            match bound.measurement_ref.as_deref() {
                Some(measurement) => linked.entry(measurement).or_default().push(bound),
                None => unlinked.push(bound),
            }
        }
        let mut reconciled: Vec<Reconciliation> = linked
            .values()
            .map(|measurement| settle_one_source_measurement(scope_ref, metric, measurement))
            .collect();
        reconciled.extend(unlinked.into_iter().map(|bound| Reconciliation::Settled {
            scope_ref: scope_ref.to_owned(),
            metric: metric.to_owned(),
            authoritative: Box::new(bound.clone()),
            corroborating: Vec::new(),
        }));
        return reconciled;
    }

    let mut linked: BTreeMap<&str, Vec<&BoundObservation>> = BTreeMap::new();
    let mut unlinked: Vec<&BoundObservation> = Vec::new();
    for bound in group {
        match bound.measurement_ref.as_deref() {
            Some(measurement) => linked.entry(measurement).or_default().push(bound),
            None => unlinked.push(bound),
        }
    }

    if linked.len() == 1 && unlinked.is_empty() {
        let measurement = linked.values().next().expect("one measurement");
        return vec![settle_measurement(scope_ref, metric, measurement, policy)];
    }
    if linked.is_empty() {
        // Several sources, no host-issued measurement: only an explicit
        // priority can say which report to believe.
        if policy.covers(sources.iter().copied()) {
            return vec![settle_measurement(scope_ref, metric, group, policy)];
        }
        return vec![ambiguous(scope_ref, metric, group)];
    }
    if linked.len() == 1 && !unlinked.is_empty() {
        // A measurement and an unlinked report for the same scope and metric:
        // an explicit priority covering every source resolves the pair, and
        // otherwise the unlinked report cannot be attributed to the measurement
        // or separated from it.
        if policy.covers(sources.iter().copied()) {
            return vec![settle_measurement(scope_ref, metric, group, policy)];
        }
        return vec![ambiguous(scope_ref, metric, group)];
    }

    // Several distinct measurements: each settles on its own. An unlinked
    // report cannot be attributed to one of them, so it makes the whole group
    // ambiguous rather than being guessed onto the nearest measurement.
    if !unlinked.is_empty() {
        return vec![ambiguous(scope_ref, metric, group)];
    }
    linked
        .values()
        .map(|measurement| settle_measurement(scope_ref, metric, measurement, policy))
        .collect()
}

/// One measurement reported several times by one source.
///
/// The reports claim one host-issued measurement, so they are one call: the
/// highest revision is authoritative. Two reports at the same revision disagree
/// about what the call was, and that is ambiguous rather than a coin toss.
fn settle_one_source_measurement(
    scope_ref: &str,
    metric: &str,
    measurement: &[&BoundObservation],
) -> Reconciliation {
    let mut ordered: Vec<&BoundObservation> = measurement.to_vec();
    ordered.sort_by_key(|bound| std::cmp::Reverse(bound.revision()));
    let authoritative = ordered[0];
    let tied = ordered.iter().skip(1).any(|bound| {
        bound.revision() == authoritative.revision()
            && bound.observation.metrics.get(metric)
                != authoritative.observation.metrics.get(metric)
    });
    if tied {
        return ambiguous(scope_ref, metric, measurement);
    }
    Reconciliation::Settled {
        scope_ref: scope_ref.to_owned(),
        metric: metric.to_owned(),
        authoritative: Box::new(authoritative.clone()),
        corroborating: ordered.into_iter().skip(1).cloned().collect(),
    }
}

fn settle_measurement(
    scope_ref: &str,
    metric: &str,
    measurement: &[&BoundObservation],
    policy: &CorrelationPolicy,
) -> Reconciliation {
    if measurement.len() == 1 {
        return Reconciliation::Settled {
            scope_ref: scope_ref.to_owned(),
            metric: metric.to_owned(),
            authoritative: Box::new(measurement[0].clone()),
            corroborating: Vec::new(),
        };
    }
    let sources: BTreeSet<&str> = measurement.iter().map(|bound| bound.source_ref()).collect();
    if !policy.covers(sources.iter().copied()) {
        return ambiguous(scope_ref, metric, measurement);
    }
    let (authoritative, corroborating) = select_by_priority(measurement, policy);
    Reconciliation::Settled {
        scope_ref: scope_ref.to_owned(),
        metric: metric.to_owned(),
        authoritative: Box::new(authoritative),
        corroborating,
    }
}

fn select_by_priority(
    measurement: &[&BoundObservation],
    policy: &CorrelationPolicy,
) -> (BoundObservation, Vec<BoundObservation>) {
    let mut ordered: Vec<&BoundObservation> = measurement.to_vec();
    ordered.sort_by_key(|bound| policy.rank(bound.source_ref()).unwrap_or(usize::MAX));
    let mut iter = ordered.into_iter();
    let authoritative = iter.next().expect("a measurement has a report").clone();
    let corroborating = iter.cloned().collect();
    (authoritative, corroborating)
}

fn ambiguous(scope_ref: &str, metric: &str, group: &[&BoundObservation]) -> Reconciliation {
    Reconciliation::Ambiguous {
        scope_ref: scope_ref.to_owned(),
        metric: metric.to_owned(),
        candidates: group.iter().map(|bound| (*bound).clone()).collect(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use licoup_extension_contracts::usage::{
        MetricValue, Quality, Temporality, UsageObservation, UsageOperation,
    };
    use licoup_usage_source_sdk::binding::SourceBinding;
    use std::collections::BTreeMap;

    fn bound(source: &str, id: &str, value: &str, measurement: Option<&str>) -> BoundObservation {
        let observation = UsageObservation {
            schema: licoup_extension_contracts::wire::USAGE.to_owned(),
            observation_id: id.to_owned(),
            revision: 1,
            operation: UsageOperation::Upsert,
            source_epoch: "epoch-1".to_owned(),
            scope_ref: "invocation-1".to_owned(),
            observed_at: "2026-09-21T00:00:00Z".to_owned(),
            interval_start: None,
            metrics: BTreeMap::from([(
                "licoup.tokens.input".to_owned(),
                MetricValue {
                    value: Some(value.to_owned()),
                    unit: "tokens".to_owned(),
                    temporality: Temporality::Delta,
                    quality: Quality::Reported,
                    includes: Vec::new(),
                },
            )]),
            cost: None,
        };
        SourceBinding::new(
            source,
            "example.agent",
            "instance-1",
            1,
            "epoch-1",
            ["invocation-1"],
        )
        .expect("binding")
        .bind_observation(observation, measurement)
        .expect("bound")
    }

    #[test]
    fn a_shared_measurement_settles_once_by_priority() {
        let reports = vec![
            bound("source:graph#1", "obs-graph", "120", Some("call-7")),
            bound("source:agent#1", "obs-agent", "120", Some("call-7")),
            bound("source:gateway#1", "obs-gateway", "118", Some("call-7")),
        ];
        let policy =
            CorrelationPolicy::explicit(["source:gateway#1", "source:agent#1", "source:graph#1"]);
        let reconciled = reconcile(&reports, &policy);
        assert_eq!(reconciled.len(), 1);
        match &reconciled[0] {
            Reconciliation::Settled {
                authoritative,
                corroborating,
                ..
            } => {
                assert_eq!(authoritative.source_ref(), "source:gateway#1");
                assert_eq!(corroborating.len(), 2);
            }
            other => panic!("expected one settlement, got {other:?}"),
        }
    }

    #[test]
    fn several_reports_without_a_link_or_a_priority_stay_ambiguous() {
        let reports = vec![
            bound("source:graph#1", "obs-graph", "120", None),
            bound("source:gateway#1", "obs-gateway", "118", None),
        ];
        assert!(matches!(
            reconcile(&reports, &CorrelationPolicy::none()).as_slice(),
            [Reconciliation::Ambiguous { candidates, .. }] if candidates.len() == 2
        ));
    }

    #[test]
    fn an_explicit_priority_resolves_an_unlinked_conflict() {
        let reports = vec![
            bound("source:graph#1", "obs-graph", "120", None),
            bound("source:gateway#1", "obs-gateway", "118", None),
        ];
        let policy = CorrelationPolicy::explicit(["source:gateway#1", "source:graph#1"]);
        match reconcile(&reports, &policy).as_slice() {
            [
                Reconciliation::Settled {
                    authoritative,
                    corroborating,
                    ..
                },
            ] => {
                assert_eq!(authoritative.source_ref(), "source:gateway#1");
                assert_eq!(corroborating.len(), 1);
            }
            other => panic!("expected one settlement, got {other:?}"),
        }
    }

    #[test]
    fn a_partial_priority_is_not_an_order() {
        let reports = vec![
            bound("source:graph#1", "obs-graph", "120", None),
            bound("source:gateway#1", "obs-gateway", "118", None),
            bound("source:agent#1", "obs-agent", "119", None),
        ];
        let policy = CorrelationPolicy::explicit(["source:gateway#1", "source:graph#1"]);
        assert!(matches!(
            reconcile(&reports, &policy).as_slice(),
            [Reconciliation::Ambiguous { .. }]
        ));
    }

    #[test]
    fn distinct_measurements_and_single_source_calls_settle_separately() {
        let distinct = vec![
            bound("source:graph#1", "obs-1", "10", Some("call-1")),
            bound("source:gateway#1", "obs-2", "20", Some("call-2")),
        ];
        let reconciled = reconcile(&distinct, &CorrelationPolicy::none());
        assert_eq!(reconciled.len(), 2);
        assert!(reconciled.iter().all(|item| matches!(item, Reconciliation::Settled { corroborating, .. } if corroborating.is_empty())));

        let same_source = vec![
            bound("source:agent#1", "obs-1", "10", None),
            bound("source:agent#1", "obs-2", "20", None),
        ];
        assert_eq!(reconcile(&same_source, &CorrelationPolicy::none()).len(), 2);
    }

    #[test]
    fn one_source_reporting_one_measurement_twice_is_one_call() {
        let mut first = bound("source:agent#1", "obs-1", "100", Some("call-1"));
        let mut second = bound("source:agent#1", "obs-2", "120", Some("call-1"));
        second.observation.revision = 2;

        match reconcile(&[first.clone(), second.clone()], &CorrelationPolicy::none()).as_slice() {
            [
                Reconciliation::Settled {
                    authoritative,
                    corroborating,
                    ..
                },
            ] => {
                assert_eq!(authoritative.revision(), 2);
                assert_eq!(
                    authoritative.observation.metrics["licoup.tokens.input"]
                        .value
                        .as_deref(),
                    Some("120")
                );
                assert_eq!(corroborating.len(), 1);
            }
            other => panic!("expected one settlement, got {other:?}"),
        }

        // Two reports at the same revision that disagree about the value cannot
        // be ordered, so they are ambiguous rather than a coin toss.
        first.observation.revision = 2;
        assert!(matches!(
            reconcile(&[first.clone(), second], &CorrelationPolicy::none()).as_slice(),
            [Reconciliation::Ambiguous { candidates, .. }] if candidates.len() == 2
        ));

        // The same revision with the same value is one call, corroborated.
        let mut same = first.clone();
        same.observation.observation_id = "obs-3".to_owned();
        assert!(matches!(
            reconcile(&[first, same], &CorrelationPolicy::none()).as_slice(),
            [Reconciliation::Settled { corroborating, .. }] if corroborating.len() == 1
        ));
    }
}
