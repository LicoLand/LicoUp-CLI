//! Component-integration harness for V7-U6.
//!
//! Everything here is synthetic: the sources are names, the observations are
//! constructed in code or read from the `fixtures/` directory, and the core fact
//! port is an in-memory double that counts what it was asked to do. No real
//! account, ledger or usage file is read, and no network is touched.
//!
//! The harness exists so the acceptance scenarios can be written as ordinary
//! component calls: bind and normalize an input, admit it, reconcile, prepare a
//! panel, uninstall. The `MemoryFacts` double keeps byte accounting of what it
//! stores so "the facts survived the uninstall" is a comparison of values, not a
//! claim.

use licoup_analytics::facts::{
    CoreUsageFacts, FactOutcome, FactPage, FactReceipt, MeteringFact, PendingObligation,
};
use licoup_extension_contracts::ui::{Contribution, ContributionKind, Series};
use licoup_extension_contracts::usage::{
    CostObservation, MetricValue, Quality, Temporality, UsageObservation, UsageOperation,
};
use licoup_usage_source_sdk::binding::{BoundObservation, SourceBinding};
use licoup_usage_source_sdk::normalize::{
    CostFieldMapping, FieldMapping, MetricFieldMapping, UnknownMetric,
};
use serde_json::Value;
use std::cell::Cell;
use std::collections::BTreeMap;

/// One observation in the C11 wire form, as a pushed batch or a pulled page
/// carries it.
pub fn wire_observation(
    source_epoch: &str,
    scope_ref: &str,
    observation_id: &str,
    metric: &str,
    value: &str,
) -> Value {
    serde_json::json!({
        "schema": licoup_extension_contracts::wire::USAGE,
        "observationId": observation_id,
        "revision": 1,
        "operation": "upsert",
        "sourceEpoch": source_epoch,
        "scopeRef": scope_ref,
        "observedAt": "2026-09-21T00:00:00Z",
        "metrics": {
            metric: {
                "value": value,
                "unit": "tokens",
                "temporality": "delta",
                "quality": "reported",
            }
        }
    })
}

/// The synthetic core fact port.
///
/// It is a *double*, not a ledger: it stores what the analytics package records
/// and counts the calls, so a test can prove that an uninstall touched nothing
/// and that a redraw re-read nothing.
#[derive(Default)]
pub struct MemoryFacts {
    facts: BTreeMap<String, MeteringFact>,
    pub records: u64,
    pub retracts: u64,
    /// `read` calls, which a panel redraw must not cause.
    read_calls: Cell<u64>,
    pub pending: Vec<PendingObligation>,
}

impl MemoryFacts {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn len(&self) -> usize {
        self.facts.len()
    }

    pub fn is_empty(&self) -> bool {
        self.facts.is_empty()
    }

    pub fn fact(&self, fact_id: &str) -> Option<&MeteringFact> {
        self.facts.get(fact_id)
    }

    pub fn facts(&self) -> impl Iterator<Item = &MeteringFact> {
        self.facts.values()
    }

    pub fn read_calls(&self) -> u64 {
        self.read_calls.get()
    }

    /// The sum of the exact values recorded for one scope and metric.
    pub fn settled_total(&self, scope_ref: &str, metric: &str) -> Option<String> {
        let mut total: Option<licoup_extension_contracts::usage::ExactNumber> = None;
        for fact in self.facts.values() {
            if fact.scope_ref != scope_ref || fact.metric != metric {
                continue;
            }
            let Some(value) = fact.exact() else {
                continue;
            };
            total = Some(match total {
                None => value,
                Some(previous) => {
                    let canonical = value.to_canonical_string();
                    let negated = match canonical.strip_prefix('-') {
                        Some(magnitude) => magnitude.to_owned(),
                        None => format!("-{canonical}"),
                    };
                    previous
                        .checked_sub(
                            licoup_extension_contracts::usage::ExactNumber::parse(&negated)
                                .expect("negated exact"),
                        )
                        .expect("exact sum")
                }
            });
        }
        total.map(|value| value.to_canonical_string())
    }

    /// A stable snapshot of what the ledger holds, for before/after comparison.
    pub fn snapshot(&self) -> Vec<(String, Option<String>, String)> {
        self.facts
            .values()
            .map(|fact| (fact.fact_id.clone(), fact.value.clone(), fact.unit.clone()))
            .collect()
    }
}

impl CoreUsageFacts for MemoryFacts {
    fn record(&mut self, fact: MeteringFact) -> FactReceipt {
        self.records += 1;
        fact.validate().expect("the package records valid facts");
        let fact_id = fact.fact_id.clone();
        let outcome = match self.facts.get(&fact.fact_id) {
            None => FactOutcome::Inserted,
            Some(existing) if existing == &fact => FactOutcome::Unchanged,
            Some(_) => FactOutcome::Updated,
        };
        self.facts.insert(fact.fact_id.clone(), fact);
        FactReceipt { fact_id, outcome }
    }

    fn retract(&mut self, fact_id: &str) -> FactReceipt {
        self.retracts += 1;
        let outcome = if self.facts.remove(fact_id).is_some() {
            FactOutcome::Withdrawn
        } else {
            FactOutcome::UnknownFact
        };
        FactReceipt {
            fact_id: fact_id.to_owned(),
            outcome,
        }
    }

    fn read(&self, _cursor: Option<&str>, limit: usize) -> FactPage {
        self.read_calls.set(self.read_calls.get() + 1);
        FactPage {
            facts: self.facts.values().take(limit).cloned().collect(),
            next_cursor: None,
        }
    }

    fn pending(&self) -> Vec<PendingObligation> {
        self.pending.clone()
    }
}

/// A source binding over one synthetic scope.
pub fn binding(source_ref: &str, scope_ref: &str) -> SourceBinding {
    SourceBinding::new(
        source_ref,
        "example.analytics-source",
        "instance-1",
        1,
        "epoch-1",
        [scope_ref],
    )
    .expect("binding")
}

/// One upsert observation with a single metric.
///
/// The arguments are the fields the acceptance scenarios vary independently; a
/// builder struct would only rename them.
#[allow(clippy::too_many_arguments)]
pub fn observation(
    source_ref: &str,
    scope_ref: &str,
    observation_id: &str,
    revision: u32,
    metric: &str,
    value: Option<&str>,
    unit: &str,
    temporality: Temporality,
    quality: Quality,
    interval_start: Option<&str>,
    observed_at: &str,
    measurement: Option<&str>,
) -> BoundObservation {
    let reading = match value {
        Some(value) => MetricValue {
            value: Some(value.to_owned()),
            unit: unit.to_owned(),
            temporality,
            quality,
            includes: Vec::new(),
        },
        None => MetricValue::unknown(unit, temporality),
    };
    let observation = UsageObservation {
        schema: licoup_extension_contracts::wire::USAGE.to_owned(),
        observation_id: observation_id.to_owned(),
        revision,
        operation: UsageOperation::Upsert,
        source_epoch: "epoch-1".to_owned(),
        scope_ref: scope_ref.to_owned(),
        observed_at: observed_at.to_owned(),
        interval_start: interval_start.map(str::to_owned),
        metrics: BTreeMap::from([(metric.to_owned(), reading)]),
        cost: None,
    };
    binding(source_ref, scope_ref)
        .bind_observation(observation, measurement)
        .expect("bound")
}

/// One retraction of an observation identity.
pub fn retraction(
    source_ref: &str,
    scope_ref: &str,
    observation_id: &str,
    revision: u32,
) -> BoundObservation {
    let observation = UsageObservation {
        schema: licoup_extension_contracts::wire::USAGE.to_owned(),
        observation_id: observation_id.to_owned(),
        revision,
        operation: UsageOperation::Retract,
        source_epoch: "epoch-1".to_owned(),
        scope_ref: scope_ref.to_owned(),
        observed_at: "2026-09-21T00:30:00Z".to_owned(),
        interval_start: None,
        metrics: BTreeMap::new(),
        cost: None,
    };
    binding(source_ref, scope_ref)
        .bind_observation(observation, None)
        .expect("bound")
}

/// One observation carrying an exact cost.
pub fn cost_observation(
    source_ref: &str,
    scope_ref: &str,
    observation_id: &str,
    amount: Option<&str>,
    currency: &str,
    quality: Quality,
    measurement: Option<&str>,
) -> BoundObservation {
    let observation = UsageObservation {
        schema: licoup_extension_contracts::wire::USAGE.to_owned(),
        observation_id: observation_id.to_owned(),
        revision: 1,
        operation: UsageOperation::Upsert,
        source_epoch: "epoch-1".to_owned(),
        scope_ref: scope_ref.to_owned(),
        observed_at: "2026-09-21T00:00:00Z".to_owned(),
        interval_start: None,
        metrics: BTreeMap::from([(
            "licoup.tokens.input".to_owned(),
            MetricValue {
                value: Some("120".to_owned()),
                unit: "tokens".to_owned(),
                temporality: Temporality::Delta,
                quality: Quality::Reported,
                includes: Vec::new(),
            },
        )]),
        cost: Some(CostObservation {
            amount: amount.map(str::to_owned),
            currency: currency.to_owned(),
            quality,
        }),
    };
    binding(source_ref, scope_ref)
        .bind_observation(observation, measurement)
        .expect("bound")
}

/// The mapping one specialist Agent's records are read through.
///
/// The Agent reports items processed and duration, and declares that it does not
/// report tokens: the declaration is what makes "no tokens" different from "zero
/// tokens".
pub fn specialist_mapping() -> FieldMapping {
    FieldMapping {
        observation_id: "run".to_owned(),
        revision: Some("revision".to_owned()),
        operation: None,
        observed_at: "at".to_owned(),
        interval_start: None,
        scope_ref: None,
        metrics: BTreeMap::from([
            (
                "items".to_owned(),
                MetricFieldMapping::new(
                    "example.specialist/items",
                    "items",
                    Temporality::Delta,
                    Quality::Reported,
                ),
            ),
            (
                "durationMs".to_owned(),
                MetricFieldMapping::new(
                    "licoup.duration",
                    "ms",
                    Temporality::Delta,
                    Quality::Reported,
                ),
            ),
        ]),
        unknown_metrics: vec![UnknownMetric::new(
            "licoup.tokens.total",
            "tokens",
            Temporality::Delta,
        )],
        cost: Some(CostFieldMapping {
            amount: "cost.amount".to_owned(),
            currency: "cost.currency".to_owned(),
            quality: Quality::Estimated,
        }),
    }
}

/// A synthetic specialist record, as a boundary adapter would receive it.
pub fn specialist_record(run: &str, items: i64, duration_ms: i64, cost: Option<&str>) -> Value {
    let mut record = serde_json::json!({
        "run": run,
        "revision": 1,
        "at": "2026-09-21T00:00:00Z",
        "items": items,
        "durationMs": duration_ms,
    });
    if let Some(cost) = cost {
        record["cost"] = serde_json::json!({ "amount": cost, "currency": "USD" });
    }
    record
}

/// The panel the analytics package contributes.
pub fn usage_panel() -> Contribution {
    Contribution {
        schema: licoup_extension_contracts::wire::UI.to_owned(),
        id: "org.licoland.feature.analytics/usage-panel".to_owned(),
        kind: ContributionKind::MetricPanel,
        title: "Usage".to_owned(),
        required_profile: Some("usage-metric".to_owned()),
        resource_ref: Some("resource.analytics/usage".to_owned()),
        resource_format: None,
        action_ref: None,
        fields: Vec::new(),
        series: vec![
            Series {
                metric: "licoup.tokens.input".to_owned(),
                label: "Input tokens".to_owned(),
                unit: "tokens".to_owned(),
            },
            Series {
                metric: "licoup.tokens.cached-input".to_owned(),
                label: "Cached input".to_owned(),
                unit: "tokens".to_owned(),
            },
            Series {
                metric: "example.specialist/items".to_owned(),
                label: "Items".to_owned(),
                unit: "items".to_owned(),
            },
        ],
    }
}

/// A synthetic OTLP-shaped cumulative series fixture.
pub fn otlp_cumulative() -> Value {
    serde_json::from_str(include_str!("../fixtures/otlp-cumulative.json")).expect("fixture")
}

/// The same series after a producer restart: a new origin and a lower value.
pub fn otlp_restarted() -> Value {
    serde_json::from_str(include_str!("../fixtures/otlp-restarted.json")).expect("fixture")
}

/// The same series with a regressed value inside one origin.
pub fn otlp_regressed() -> Value {
    serde_json::from_str(include_str!("../fixtures/otlp-regressed.json")).expect("fixture")
}

/// One synthetic log-adapter input, not a collected runtime log.
pub fn log_line() -> &'static str {
    "run=log-run-1 revision=1 at=2026-09-21T00:00:00Z items=7 durationMs=900"
}
