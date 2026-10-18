//! Declarative metric panels and their prepared values.
//!
//! C13 says a contribution is data: it names a host primitive, the resource or
//! action it binds to and the least profile it needs, and the host owns focus,
//! accessibility, theming and prepared-value consistency. This module is the
//! analytics package's side of that: it mounts `metric-panel` contributions,
//! hands the host **prepared values** built from observations, and refuses a
//! prepared value that belongs to a replaced generation.
//!
//! The rules that matter for A34 are in the value, not the widget:
//!
//! - a reading nobody reported is [`Quality::Unknown`] with **no number**, so a
//!   panel can draw "not reported" instead of a zero;
//! - a prepared value may only carry metrics its own contribution declared;
//! - a value prepared for an older generation is refused by generation, not by
//!   arrival time.
//!
//! Withdrawing the registry releases every mounted contribution and every
//! prepared value it held. It does not touch an observation, a fact or a budget:
//! those live elsewhere ([`crate::facts`]), which is why uninstalling the panel
//! package cannot delete history.

use licoup_extension_contracts::ApplicationFailure;
use licoup_extension_contracts::profile::ExtensionProfile;
use licoup_extension_contracts::ui::{
    Contribution, ContributionKind, plan_mount, prepared_generation_accepted,
};
use licoup_extension_contracts::usage::{MetricValue, Quality};
use std::collections::BTreeMap;

use crate::refusal;

/// One prepared series point: what the panel draws, and what it must not invent.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PreparedPoint {
    pub metric: String,
    pub label: String,
    pub unit: String,
    /// The exact decimal text, or `None` when the reading is unknown.
    pub value: Option<String>,
    pub quality: Quality,
}

impl PreparedPoint {
    pub fn known(
        metric: impl Into<String>,
        label: impl Into<String>,
        unit: impl Into<String>,
        value: impl Into<String>,
        quality: Quality,
    ) -> Self {
        Self {
            metric: metric.into(),
            label: label.into(),
            unit: unit.into(),
            value: Some(value.into()),
            quality,
        }
    }

    /// A point the source did not report. There is no number, because a zero
    /// would be drawn as a measurement.
    pub fn unknown(
        metric: impl Into<String>,
        label: impl Into<String>,
        unit: impl Into<String>,
    ) -> Self {
        Self {
            metric: metric.into(),
            label: label.into(),
            unit: unit.into(),
            value: None,
            quality: Quality::Unknown,
        }
    }
}

/// A prepared value for one mounted panel.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PreparedPanelValue {
    pub contribution_id: String,
    pub generation: u64,
    pub points: Vec<PreparedPoint>,
}

impl PreparedPanelValue {
    /// Build a prepared value from the readings the panel declared.
    ///
    /// A declared metric with no reading becomes an unknown point. The function
    /// is pure: preparing a panel reads the values it is given and never asks a
    /// source for more, so a redraw is not a re-scan.
    pub fn from_readings(
        contribution: &Contribution,
        generation: u64,
        readings: &BTreeMap<String, MetricValue>,
    ) -> Self {
        let points = contribution
            .series
            .iter()
            .map(|series| match readings.get(&series.metric) {
                None => PreparedPoint::unknown(&series.metric, &series.label, &series.unit),
                Some(reading) => PreparedPoint {
                    metric: series.metric.clone(),
                    label: series.label.clone(),
                    unit: reading.unit.clone(),
                    value: reading.value.clone(),
                    quality: reading.quality,
                },
            })
            .collect();
        Self {
            contribution_id: contribution.id.clone(),
            generation,
            points,
        }
    }
}

/// One contribution the registry refused to mount, and why.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BlockedPanel {
    pub contribution_id: String,
    pub reason: &'static str,
}

/// What mounting did.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct MountReport {
    pub mounted: Vec<String>,
    pub blocked: Vec<BlockedPanel>,
}

/// What withdrawal released.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct WithdrawnPanels {
    pub contributions: Vec<String>,
    pub prepared: usize,
}

/// The panels this package has mounted, and their prepared values.
#[derive(Clone, Debug)]
pub struct PanelRegistry {
    generation: u64,
    mounted: BTreeMap<String, Contribution>,
    prepared: BTreeMap<String, PreparedPanelValue>,
}

impl PanelRegistry {
    pub fn new(generation: u64) -> Self {
        Self {
            generation,
            mounted: BTreeMap::new(),
            prepared: BTreeMap::new(),
        }
    }

    pub fn generation(&self) -> u64 {
        self.generation
    }

    /// Mount the contributions this host can serve.
    ///
    /// A contribution whose profile is missing or unpublished blocks only
    /// itself, exactly as the contract's [`plan_mount`] decides; a contribution
    /// that is not a metric panel is not this registry's to mount.
    pub fn mount(
        &mut self,
        contributions: &[Contribution],
        served: &[ExtensionProfile],
    ) -> MountReport {
        let mut report = MountReport::default();
        for mount in plan_mount(contributions, served) {
            let contribution = mount.contribution;
            if contribution.validate().is_err() {
                report.blocked.push(BlockedPanel {
                    contribution_id: contribution.id.clone(),
                    reason: "contribution_invalid",
                });
                continue;
            }
            if contribution.kind != ContributionKind::MetricPanel {
                report.blocked.push(BlockedPanel {
                    contribution_id: contribution.id.clone(),
                    reason: "not_a_metric_panel",
                });
                continue;
            }
            if let Some(reason) = mount.blocked {
                report.blocked.push(BlockedPanel {
                    contribution_id: contribution.id.clone(),
                    reason,
                });
                continue;
            }
            self.mounted
                .insert(contribution.id.clone(), contribution.clone());
            report.mounted.push(contribution.id.clone());
        }
        report
    }

    pub fn mounted_ids(&self) -> Vec<String> {
        self.mounted.keys().cloned().collect()
    }

    pub fn mounted(&self, contribution_id: &str) -> Option<&Contribution> {
        self.mounted.get(contribution_id)
    }

    /// Install a prepared value, refusing one that does not belong here.
    pub fn install_prepared(
        &mut self,
        value: PreparedPanelValue,
    ) -> Result<(), ApplicationFailure> {
        let Some(contribution) = self.mounted.get(&value.contribution_id) else {
            return Err(refusal("analytics_panel_not_mounted").with_field("contributionId"));
        };
        prepared_generation_accepted(self.generation, value.generation)?;
        for point in &value.points {
            let Some(series) = contribution
                .series
                .iter()
                .find(|series| series.metric == point.metric)
            else {
                return Err(
                    refusal("analytics_panel_undeclared_series").with_field("points.metric")
                );
            };
            if point.unit != series.unit {
                return Err(refusal("analytics_panel_invalid").with_field("points.unit"));
            }
            if point.quality.has_value() != point.value.is_some() {
                return Err(refusal("analytics_panel_invalid").with_field("points.value"));
            }
        }
        self.prepared.insert(value.contribution_id.clone(), value);
        Ok(())
    }

    /// The prepared value for a panel. This is a pure read: a redraw does not
    /// re-read a source or recompute an index.
    pub fn prepared(&self, contribution_id: &str) -> Option<&PreparedPanelValue> {
        self.prepared.get(contribution_id)
    }

    /// Withdraw every contribution and prepared value, reporting what was
    /// released.
    pub fn withdraw(&mut self) -> WithdrawnPanels {
        let withdrawn = WithdrawnPanels {
            contributions: self.mounted.keys().cloned().collect(),
            prepared: self.prepared.len(),
        };
        self.mounted.clear();
        self.prepared.clear();
        withdrawn
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use licoup_extension_contracts::ui::Series;

    fn panel() -> Contribution {
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
                    metric: "example.specialist/items".to_owned(),
                    label: "Items".to_owned(),
                    unit: "items".to_owned(),
                },
            ],
        }
    }

    #[test]
    fn a_panel_mounts_only_with_the_profile_it_needs() {
        let mut registry = PanelRegistry::new(1);
        let report = registry.mount(&[panel()], &[]);
        assert!(report.mounted.is_empty());
        assert_eq!(report.blocked[0].reason, "profile_not_installed");

        let report = registry.mount(&[panel()], &[ExtensionProfile::UsageMetric]);
        assert_eq!(
            report.mounted,
            vec!["org.licoland.feature.analytics/usage-panel"]
        );
        assert!(registry.mounted_ids().len() == 1);
    }

    #[test]
    fn an_unreported_metric_is_prepared_as_unknown_not_zero() {
        let contribution = panel();
        let readings = BTreeMap::from([(
            "licoup.tokens.input".to_owned(),
            MetricValue {
                value: Some("120".to_owned()),
                unit: "tokens".to_owned(),
                temporality: licoup_extension_contracts::usage::Temporality::Delta,
                quality: Quality::Reported,
                includes: Vec::new(),
            },
        )]);
        let prepared = PreparedPanelValue::from_readings(&contribution, 1, &readings);
        assert_eq!(prepared.points[0].value.as_deref(), Some("120"));
        assert_eq!(prepared.points[1].value, None);
        assert_eq!(prepared.points[1].quality, Quality::Unknown);

        let mut registry = PanelRegistry::new(1);
        registry.mount(&[contribution], &[ExtensionProfile::UsageMetric]);
        registry.install_prepared(prepared).expect("prepared");
        let kept = registry
            .prepared("org.licoland.feature.analytics/usage-panel")
            .expect("kept");
        assert_eq!(kept.points.len(), 2);
    }

    #[test]
    fn a_stale_generation_or_an_undeclared_series_is_refused() {
        let contribution = panel();
        let mut registry = PanelRegistry::new(7);
        registry.mount(
            std::slice::from_ref(&contribution),
            &[ExtensionProfile::UsageMetric],
        );

        let mut stale = PreparedPanelValue::from_readings(&contribution, 6, &BTreeMap::new());
        assert_eq!(
            registry
                .install_prepared(stale.clone())
                .expect_err("stale")
                .code,
            "ui_prepared_generation_stale"
        );

        stale.generation = 7;
        stale.points.push(PreparedPoint::known(
            "example.undeclared/metric",
            "Other",
            "items",
            "1",
            Quality::Reported,
        ));
        assert_eq!(
            registry
                .install_prepared(stale)
                .expect_err("undeclared")
                .code,
            "analytics_panel_undeclared_series"
        );

        let mut zero = PreparedPanelValue::from_readings(&contribution, 7, &BTreeMap::new());
        zero.points[0].value = Some("0".to_owned());
        zero.points[0].quality = Quality::Unknown;
        assert_eq!(
            registry
                .install_prepared(zero)
                .expect_err("unknown with a number")
                .code,
            "analytics_panel_invalid"
        );
    }

    #[test]
    fn withdrawal_releases_every_panel_and_prepared_value() {
        let contribution = panel();
        let mut registry = PanelRegistry::new(1);
        registry.mount(
            std::slice::from_ref(&contribution),
            &[ExtensionProfile::UsageMetric],
        );
        registry
            .install_prepared(PreparedPanelValue::from_readings(
                &contribution,
                1,
                &BTreeMap::new(),
            ))
            .expect("prepared");

        let withdrawn = registry.withdraw();
        assert_eq!(withdrawn.contributions.len(), 1);
        assert_eq!(withdrawn.prepared, 1);
        assert!(registry.mounted_ids().is_empty());
        assert!(
            registry
                .prepared("org.licoland.feature.analytics/usage-panel")
                .is_none()
        );
        assert_eq!(
            registry
                .install_prepared(PreparedPanelValue::from_readings(
                    &contribution,
                    1,
                    &BTreeMap::new()
                ))
                .expect_err("no longer mounted")
                .code,
            "analytics_panel_not_mounted"
        );
    }

    #[test]
    fn a_non_panel_contribution_is_not_mounted_here() {
        let mut settings = panel();
        settings.kind = ContributionKind::Settings;
        settings.series = Vec::new();
        let mut registry = PanelRegistry::new(1);
        let report = registry.mount(&[settings], &[ExtensionProfile::UsageMetric]);
        assert_eq!(report.blocked[0].reason, "not_a_metric_panel");
    }
}
