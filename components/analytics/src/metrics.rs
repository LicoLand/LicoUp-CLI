//! The professional metric catalog: declared units, declared aggregation.
//!
//! A specialist source reports what it measures — items processed, latency,
//! queue time — and this catalog is where those metrics become defined rather
//! than merely named. A metric has a unit, a temporality and an aggregation, and
//! the aggregation decides what may be done with readings:
//!
//! - a `sum` metric adds, and only readings of the same unit are added;
//! - a `last` or `max` metric takes one reading, because a level is not a
//!   quantity;
//! - a `quantile` is refused for averaging: a sampled p95 from two sources is
//!   not the p95 of their union;
//! - a cumulative reading is refused for direct summing: it is differenced
//!   first ([`crate::index::ObservationIndex::counter_step`]), because adding two
//!   running totals is exactly the double count C11 forbids.
//!
//! The general metrics come from the SDK's vocabulary, including the subset
//! relations: a panel that tries to add `licoup.tokens.total` to
//! `licoup.tokens.cached-input` is refused by [`MetricCatalog::additive_check`],
//! because the first already contains the second.

use licoup_extension_contracts::usage::{ExactNumber, MetricValue, Temporality};
use licoup_extension_contracts::{ApplicationFailure, is_namespaced};
use licoup_usage_source_sdk::describe::Aggregation;
use licoup_usage_source_sdk::metrics as general;
use std::collections::BTreeMap;

use crate::refusal;

/// One declared metric.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MetricDefinition {
    pub metric: String,
    pub unit: String,
    pub temporality: Temporality,
    pub aggregation: Aggregation,
}

impl MetricDefinition {
    pub fn new(
        metric: impl Into<String>,
        unit: impl Into<String>,
        temporality: Temporality,
        aggregation: Aggregation,
    ) -> Self {
        Self {
            metric: metric.into(),
            unit: unit.into(),
            temporality,
            aggregation,
        }
    }
}

/// What readings of one metric aggregate to.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Aggregate {
    /// A value, the unit it is in, and how many readings were unknown.
    Value {
        total: String,
        unit: String,
        unknown: usize,
    },
    /// No reading carried a number. It is unknown, not zero.
    Unknown { unit: String, observed: usize },
}

/// The metrics this installation knows.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct MetricCatalog {
    definitions: BTreeMap<String, MetricDefinition>,
}

impl MetricCatalog {
    pub fn new() -> Self {
        Self::default()
    }

    /// The catalog seeded with C11's general metrics.
    pub fn with_general() -> Self {
        let mut catalog = Self::new();
        for standard in general::standard() {
            catalog.definitions.insert(
                standard.metric.to_owned(),
                MetricDefinition {
                    metric: standard.metric.to_owned(),
                    unit: standard.unit.to_owned(),
                    temporality: standard.temporality,
                    aggregation: standard.aggregation,
                },
            );
        }
        catalog
    }

    /// Declare a specialist metric.
    ///
    /// A conflicting redefinition is refused: two units for one metric is how a
    /// total silently becomes meaningless.
    pub fn define(&mut self, definition: MetricDefinition) -> Result<(), ApplicationFailure> {
        if !is_namespaced(&definition.metric) || definition.unit.is_empty() {
            return Err(refusal("analytics_metric_invalid").with_field("metric"));
        }
        if let Some(existing) = self.definitions.get(&definition.metric) {
            if existing != &definition {
                return Err(refusal("analytics_metric_redefined").with_field("metric"));
            }
            return Ok(());
        }
        self.definitions
            .insert(definition.metric.clone(), definition);
        Ok(())
    }

    pub fn definition(&self, metric: &str) -> Option<&MetricDefinition> {
        self.definitions.get(metric)
    }

    pub fn metrics(&self) -> impl Iterator<Item = &MetricDefinition> {
        self.definitions.values()
    }

    /// Adopt the fields one source declared through `usage.describe`.
    ///
    /// A source's declaration is what makes its specialist metrics defined
    /// rather than merely named; a conflicting redeclaration is refused by
    /// [`Self::define`] exactly as if it had been written here.
    pub fn declare_source(
        &mut self,
        description: &licoup_usage_source_sdk::describe::SourceDescription,
    ) -> Result<(), ApplicationFailure> {
        description.validate()?;
        for field in &description.fields {
            self.define(MetricDefinition {
                metric: field.metric.clone(),
                unit: field.unit.clone(),
                temporality: field.temporality,
                aggregation: field.aggregation,
            })?;
        }
        Ok(())
    }

    /// Aggregate readings of one metric according to its declaration.
    pub fn aggregate(
        &self,
        metric: &str,
        readings: &[MetricValue],
    ) -> Result<Aggregate, ApplicationFailure> {
        let Some(definition) = self.definitions.get(metric) else {
            return Err(refusal("analytics_metric_undeclared").with_field("metric"));
        };
        if definition.aggregation == Aggregation::Quantile {
            return Err(refusal("analytics_quantile_not_averaged").with_field("aggregation"));
        }
        let unit = definition.unit.clone();
        if readings.is_empty() {
            return Ok(Aggregate::Unknown { unit, observed: 0 });
        }
        for reading in readings {
            if reading.unit != definition.unit {
                return Err(refusal("usage_unit_mismatch").with_field("unit"));
            }
            if definition.aggregation == Aggregation::Sum
                && reading.temporality == Temporality::Cumulative
            {
                return Err(refusal("analytics_cumulative_not_summed").with_field("temporality"));
            }
        }
        let mut unknown = 0;
        let mut total: Option<ExactNumber> = None;
        for reading in readings {
            match reading.exact() {
                None => unknown += 1,
                Some(value) => {
                    total = Some(match (definition.aggregation, total) {
                        (Aggregation::Sum, Some(previous)) => add(previous, value)
                            .ok_or_else(|| refusal("usage_decimal_overflow").with_field("value"))?,
                        (Aggregation::Max, Some(previous)) => previous.max(value),
                        (Aggregation::Last, _)
                        | (Aggregation::Sum, None)
                        | (Aggregation::Max, None) => value,
                        (Aggregation::Quantile, _) => unreachable!("refused above"),
                    });
                }
            }
        }
        match total {
            Some(total) => Ok(Aggregate::Value {
                total: total.to_canonical_string(),
                unit,
                unknown,
            }),
            None => Ok(Aggregate::Unknown {
                unit,
                observed: readings.len(),
            }),
        }
    }

    /// Refuse a set of series that must not be added together.
    ///
    /// Two refusals live here: a metric that already contains another (a total
    /// and its cached subset), and two summable metrics in different units.
    pub fn additive_check(&self, metrics: &[&str]) -> Result<(), ApplicationFailure> {
        for (index, metric) in metrics.iter().enumerate() {
            for other in &metrics[index + 1..] {
                if general::includes(metric, other) || general::includes(other, metric) {
                    return Err(refusal("analytics_double_count_series")
                        .with_field("series")
                        .with_presentation_arg("contains", metric)
                        .with_presentation_arg("contained", other));
                }
            }
        }
        let mut unit: Option<&str> = None;
        for metric in metrics {
            let Some(definition) = self.definitions.get(*metric) else {
                return Err(refusal("analytics_metric_undeclared").with_field("metric"));
            };
            if !definition.aggregation.is_summable() {
                continue;
            }
            match unit {
                None => unit = Some(&definition.unit),
                Some(existing) if existing != definition.unit => {
                    return Err(refusal("usage_unit_mismatch").with_field("unit"));
                }
                Some(_) => {}
            }
        }
        Ok(())
    }
}

/// Exact addition, built on the contract's own exact subtraction.
///
/// `a - (-b)` is `a + b` with the same scale alignment and the same overflow
/// checks, so this catalog adds no second decimal implementation.
fn add(left: ExactNumber, right: ExactNumber) -> Option<ExactNumber> {
    let canonical = right.to_canonical_string();
    let negated = match canonical.strip_prefix('-') {
        Some(magnitude) => magnitude.to_owned(),
        None => format!("-{canonical}"),
    };
    left.checked_sub(ExactNumber::parse(&negated)?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use licoup_extension_contracts::usage::Quality;

    fn reading(value: &str, unit: &str) -> MetricValue {
        MetricValue {
            value: Some(value.to_owned()),
            unit: unit.to_owned(),
            temporality: Temporality::Delta,
            quality: Quality::Reported,
            includes: Vec::new(),
        }
    }

    fn unknown(unit: &str) -> MetricValue {
        MetricValue::unknown(unit, Temporality::Delta)
    }

    fn catalog() -> MetricCatalog {
        let mut catalog = MetricCatalog::with_general();
        catalog
            .define(MetricDefinition::new(
                "example.specialist/items",
                "items",
                Temporality::Delta,
                Aggregation::Sum,
            ))
            .expect("declared");
        catalog
            .define(MetricDefinition::new(
                "example.specialist/latency",
                "ms",
                Temporality::Gauge,
                Aggregation::Quantile,
            ))
            .expect("declared");
        catalog
    }

    #[test]
    fn a_specialist_metric_adds_exactly_and_counts_the_unknowns() {
        let aggregate = catalog()
            .aggregate(
                "example.specialist/items",
                &[
                    reading("1.50", "items"),
                    reading("2.25", "items"),
                    unknown("items"),
                ],
            )
            .expect("aggregated");
        assert_eq!(
            aggregate,
            Aggregate::Value {
                total: "3.75".to_owned(),
                unit: "items".to_owned(),
                unknown: 1,
            }
        );
    }

    #[test]
    fn an_all_unknown_aggregate_is_unknown_not_zero() {
        let aggregate = catalog()
            .aggregate("example.specialist/items", &[unknown("items")])
            .expect("aggregated");
        assert_eq!(
            aggregate,
            Aggregate::Unknown {
                unit: "items".to_owned(),
                observed: 1,
            }
        );
        assert_eq!(
            catalog()
                .aggregate("example.specialist/items", &[])
                .expect("aggregated"),
            Aggregate::Unknown {
                unit: "items".to_owned(),
                observed: 0,
            }
        );
    }

    #[test]
    fn units_are_never_added_and_cumulative_readings_are_differenced_first() {
        assert_eq!(
            catalog()
                .aggregate(
                    "example.specialist/items",
                    &[reading("1", "items"), reading("2", "tokens")]
                )
                .expect_err("different units")
                .code,
            "usage_unit_mismatch"
        );
        let mut cumulative = reading("100", "items");
        cumulative.temporality = Temporality::Cumulative;
        assert_eq!(
            catalog()
                .aggregate("example.specialist/items", &[cumulative])
                .expect_err("running total")
                .code,
            "analytics_cumulative_not_summed"
        );
    }

    #[test]
    fn a_quantile_is_not_averaged_across_sources() {
        assert_eq!(
            catalog()
                .aggregate("example.specialist/latency", &[reading("120", "ms")])
                .expect_err("quantiles are not averaged")
                .code,
            "analytics_quantile_not_averaged"
        );
        assert_eq!(
            catalog()
                .aggregate("example.undeclared/metric", &[reading("1", "items")])
                .expect_err("undeclared")
                .code,
            "analytics_metric_undeclared"
        );
    }

    #[test]
    fn a_total_is_never_added_to_its_own_subset() {
        assert!(
            catalog()
                .additive_check(&[general::TOKENS_TOTAL, general::TOKENS_CACHED_INPUT])
                .is_err()
        );
        assert!(
            catalog()
                .additive_check(&[
                    general::TOKENS_TOTAL,
                    general::TOKENS_INPUT,
                    general::TOKENS_OUTPUT
                ])
                .is_err()
        );
        assert!(
            catalog()
                .additive_check(&[general::TOKENS_INPUT, general::TOKENS_OUTPUT])
                .is_ok()
        );
        assert!(
            catalog()
                .additive_check(&[general::TOKENS_INPUT, general::DURATION])
                .is_err(),
            "tokens and milliseconds do not add"
        );
    }

    #[test]
    fn a_conflicting_redefinition_is_refused() {
        let mut catalog = catalog();
        assert!(
            catalog
                .define(MetricDefinition::new(
                    "example.specialist/items",
                    "items",
                    Temporality::Delta,
                    Aggregation::Sum,
                ))
                .is_ok(),
            "the same declaration is not a conflict"
        );
        assert_eq!(
            catalog
                .define(MetricDefinition::new(
                    "example.specialist/items",
                    "tokens",
                    Temporality::Delta,
                    Aggregation::Sum,
                ))
                .expect_err("two units")
                .code,
            "analytics_metric_redefined"
        );
    }

    #[test]
    fn last_and_max_take_one_reading() {
        let mut catalog = catalog();
        catalog
            .define(MetricDefinition::new(
                "example.specialist/queue",
                "ms",
                Temporality::Gauge,
                Aggregation::Last,
            ))
            .expect("declared");
        assert_eq!(
            catalog
                .aggregate(
                    "example.specialist/queue",
                    &[reading("10", "ms"), reading("25", "ms"), unknown("ms")]
                )
                .expect("aggregated"),
            Aggregate::Value {
                total: "25".to_owned(),
                unit: "ms".to_owned(),
                unknown: 1,
            }
        );
    }

    #[test]
    fn a_source_declaration_defines_its_specialist_metrics() {
        let description = licoup_usage_source_sdk::describe::SourceDescription::new(
            vec![licoup_usage_source_sdk::describe::MetricField::new(
                "example.specialist/items",
                "items",
                Temporality::Delta,
                Aggregation::Sum,
            )],
            vec![licoup_usage_source_sdk::describe::SeriesDeclaration::new(
                "example.specialist/items",
                "Items",
                "items",
            )],
        );
        let mut catalog = MetricCatalog::with_general();
        catalog.declare_source(&description).expect("adopted");
        assert!(catalog.definition("example.specialist/items").is_some());
        assert_eq!(
            catalog
                .aggregate("example.specialist/items", &[reading("3", "items")])
                .expect("aggregated"),
            Aggregate::Value {
                total: "3".to_owned(),
                unit: "items".to_owned(),
                unknown: 0,
            }
        );

        // A declaration with an undeclared series is refused before it can
        // define anything.
        let mut bad = description.clone();
        bad.series[0].metric = "example.specialist/other".to_owned();
        let mut catalog = MetricCatalog::new();
        assert!(catalog.declare_source(&bad).is_err());
        assert!(catalog.metrics().next().is_none());
    }
}
