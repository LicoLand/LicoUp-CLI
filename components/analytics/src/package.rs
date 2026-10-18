//! The analytics package: admission, reconciliation, panels and uninstall.
//!
//! This is where the pieces meet, and where the two acceptance rules are
//! enforced end to end:
//!
//! - **One call settles once.** [`AnalyticsPackage::admit`] ingests an
//!   observation into the index, reconciles its scope and metric, and records at
//!   most one fact per measurement through the core port. A second source
//!   reporting the same call is either the authoritative report of the same fact
//!   (an update) or corroboration; a conflict without a host-issued measurement
//!   or an explicit priority is [`Admission::Ambiguous`] and records nothing.
//! - **Uninstall releases the package, not the history.** The package borrows
//!   the core facts; [`AnalyticsPackage::begin_uninstall`] stops new admission
//!   and releases panels, prepared values and scrapers, and
//!   [`AnalyticsPackage::complete_uninstall`] drops the index once in-flight
//!   reads have drained. The facts are not the package's to delete.
//!
//! Nothing here starts a collector on its own: a scraper exists only for a
//! source the host configured while the package is active, and activation itself
//! is refused when the capability is not installed or not enabled, so an
//! installation without the analytics package has no collector to start.

use licoup_extension_contracts::deployment::{CapabilityAvailability, PackageFacts, availability};
use licoup_extension_contracts::usage::{MetricValue, UsageOperation};
use licoup_extension_contracts::{ApplicationFailure, RecoveryAction};
use licoup_usage_source_sdk::binding::{BoundObservation, SourceBinding};
use licoup_usage_source_sdk::collection::{Cursor, QueryPage, QueryRequest};
use licoup_usage_source_sdk::metrics as general;
use std::collections::{BTreeMap, BTreeSet};

use crate::correlation::{CorrelationPolicy, Reconciliation, reconcile};
use crate::facts::{CoreUsageFacts, FactOutcome, FactReceipt, MeteringFact};
use crate::index::{IngestOutcome, ObservationIndex};
use crate::metrics::MetricCatalog;
use crate::panels::PanelRegistry;
use crate::policy::AuthorityPolicy;
use crate::{refusal, refusal_with};

/// The capability this package provides.
pub const CAPABILITY: &str = "analytics.v1";

/// How far along the package is.
///
/// The states and the events that move between them are the package's own
/// declaration in `resources/state-machines/package.json`, compiled by
/// `build.rs`; this alias is what lets the rest of the module name them.
pub use crate::state_machine::analytics_package::State as PackageState;

impl PackageState {
    /// The wire name of this state, as the declaration spells it.
    pub const fn id(self) -> &'static str {
        self.as_str()
    }
}

/// What the host configured when it activated the package.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AnalyticsConfig {
    /// Which sources may settle, and which may only be displayed.
    pub authority: AuthorityPolicy,
    /// How several reports of one call are believed, when the host has a
    /// priority at all.
    pub correlation: CorrelationPolicy,
    /// The registry generation the package's contributions belong to.
    pub generation: u64,
}

impl AnalyticsConfig {
    pub fn new(
        authority: AuthorityPolicy,
        correlation: CorrelationPolicy,
        generation: u64,
    ) -> Self {
        Self {
            authority,
            correlation,
            generation,
        }
    }
}

/// What one admission did.
#[derive(Clone, Debug, PartialEq)]
pub enum Admission {
    /// A fact was recorded or corrected in the core ledger.
    Recorded(FactReceipt),
    /// The observation was admitted for display only: this source may not
    /// settle.
    DisplayOnly,
    /// Admitted as corroboration of a measurement another source settled.
    Corroborating,
    /// Admitted, but the reports cannot be attributed to one measurement or to
    /// distinct ones. Nothing was recorded.
    Ambiguous,
    /// The observation was withdrawn. The receipt is the fact withdrawal when
    /// there was a fact to withdraw.
    Retracted(FactReceipt),
    /// A replay or an older revision: nothing changed.
    Stale,
}

/// One ticket for an in-flight read, so uninstall can drain instead of cutting
/// a caller off mid-page.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ReadTicket {
    pub id: u64,
}

/// What uninstall released.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct UninstallReport {
    pub state: PackageState,
    /// Contributions withdrawn from the registry.
    pub withdrawn_contributions: Vec<String>,
    /// Prepared values dropped.
    pub dropped_prepared: usize,
    /// Index entries dropped at completion.
    pub dropped_observations: usize,
    /// Host-configured source bindings released.
    pub released_sources: Vec<String>,
    /// Scrapers stopped.
    pub released_scrapers: Vec<String>,
    /// Always `true`: the metering facts belong to the core and are never the
    /// package's to remove. The field exists so the claim is in the report the
    /// user can read, not only in a comment.
    pub facts_preserved: bool,
}

/// What reconciling one scope and metric produced.
#[derive(Clone, Debug, PartialEq)]
enum GroupOutcome {
    Recorded {
        receipt: FactReceipt,
        authoritative_is_ingested: bool,
    },
    /// The measurement this observation belonged to no longer exists, so the
    /// fact it had was withdrawn.
    Withdrawn(FactReceipt),
    DisplayOnly,
    Ambiguous,
    Empty,
}

/// The optional analytics package.
///
/// The core facts are borrowed, never owned: dropping every surface of this
/// package leaves the ledger exactly where it was.
pub struct AnalyticsPackage<'a, F: CoreUsageFacts> {
    state: PackageState,
    facts: &'a mut F,
    authority: AuthorityPolicy,
    correlation: CorrelationPolicy,
    catalog: MetricCatalog,
    index: ObservationIndex,
    panels: PanelRegistry,
    sources: BTreeMap<String, SourceBinding>,
    recorded: BTreeMap<(String, String), BTreeSet<String>>,
    open_reads: BTreeSet<u64>,
    next_read: u64,
    scrapers: BTreeSet<String>,
    collection_attempts: u64,
}

impl<F: CoreUsageFacts> std::fmt::Debug for AnalyticsPackage<'_, F> {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("AnalyticsPackage")
            .field("state", &self.state)
            .field("observations", &self.index.len())
            .field("sources", &self.sources.len())
            .field("open_reads", &self.open_reads.len())
            .field("scrapers", &self.scrapers.len())
            .finish_non_exhaustive()
    }
}

impl<'a, F: CoreUsageFacts> AnalyticsPackage<'a, F> {
    /// What a client shows where capabilities are listed.
    pub fn capability_state(package_facts: PackageFacts) -> CapabilityAvailability {
        availability(CAPABILITY, package_facts)
    }

    /// Activate the package, refusing when the capability is not served.
    ///
    /// The refusal is the contract's unavailable-capability vocabulary with an
    /// actionable recovery, and it is returned before anything is constructed:
    /// a client without this package has no index, no panel registry and no
    /// scraper, because there is nothing to construct.
    pub fn activate(
        facts: &'a mut F,
        package_facts: PackageFacts,
        config: AnalyticsConfig,
    ) -> Result<Self, ApplicationFailure> {
        package_facts.validate()?;
        let availability = availability(CAPABILITY, package_facts);
        if let Some(failure) = availability.refusal(CAPABILITY) {
            return Err(failure);
        }
        let state = crate::state_machine::analytics_package::transition(
            crate::state_machine::analytics_package::INITIAL,
            crate::state_machine::analytics_package::Event::Activate,
        )
        .expect("the package configuration activates from its initial state");
        Ok(Self {
            state,
            facts,
            authority: config.authority,
            correlation: config.correlation,
            catalog: MetricCatalog::with_general(),
            index: ObservationIndex::new(),
            panels: PanelRegistry::new(config.generation),
            sources: BTreeMap::new(),
            recorded: BTreeMap::new(),
            open_reads: BTreeSet::new(),
            next_read: 1,
            scrapers: BTreeSet::new(),
            collection_attempts: 0,
        })
    }

    pub fn state(&self) -> PackageState {
        self.state
    }

    pub fn catalog(&self) -> &MetricCatalog {
        &self.catalog
    }

    pub fn catalog_mut(&mut self) -> &mut MetricCatalog {
        &mut self.catalog
    }

    pub fn panels(&self) -> &PanelRegistry {
        &self.panels
    }

    pub fn panels_mut(&mut self) -> &mut PanelRegistry {
        &mut self.panels
    }

    pub fn index(&self) -> &ObservationIndex {
        &self.index
    }

    /// The core port, for surfaces that read facts while the package is active.
    ///
    /// It is a borrow: the ledger outlives every surface of this package.
    pub fn facts(&self) -> &F {
        self.facts
    }

    pub fn collection_attempts(&self) -> u64 {
        self.collection_attempts
    }

    pub fn scrapers(&self) -> usize {
        self.scrapers.len()
    }

    /// Configure a source the host has bound and decided about.
    pub fn configure_source(
        &mut self,
        binding: SourceBinding,
        settlement: bool,
    ) -> Result<(), ApplicationFailure> {
        self.require_active()?;
        self.authority.grant(binding.source_ref.clone(), settlement);
        self.sources.insert(binding.source_ref.clone(), binding);
        Ok(())
    }

    /// Admit one bound observation.
    pub fn admit(&mut self, bound: BoundObservation) -> Result<Admission, ApplicationFailure> {
        self.require_active()?;
        if !self.authority.admits(bound.source_ref()) {
            return Err(self.authority.refusal(bound.source_ref()));
        }
        let key = bound.key();
        let is_retract = bound.observation.operation == UsageOperation::Retract;
        let metrics: Vec<String> = if is_retract {
            self.index
                .get(&key)
                .map(|existing| existing.observation.metrics.keys().cloned().collect())
                .unwrap_or_default()
        } else {
            bound.observation.metrics.keys().cloned().collect()
        };
        let outcome = self.index.ingest(bound.clone());
        if matches!(
            outcome,
            IngestOutcome::Stale | IngestOutcome::AlreadyRetracted
        ) {
            return Ok(Admission::Stale);
        }

        let mut recorded: Option<FactReceipt> = None;
        let mut withdrawn: Option<FactReceipt> = None;
        let mut ingested_is_authoritative = false;
        let mut display_only = false;
        let mut ambiguous = false;
        for metric in &metrics {
            match self.apply_reconciliation(bound.scope_ref(), metric, &key)? {
                GroupOutcome::Recorded {
                    receipt,
                    authoritative_is_ingested,
                } => {
                    recorded = Some(receipt);
                    ingested_is_authoritative |= authoritative_is_ingested;
                }
                GroupOutcome::Withdrawn(receipt) => withdrawn = Some(receipt),
                GroupOutcome::DisplayOnly => display_only = true,
                GroupOutcome::Ambiguous => ambiguous = true,
                GroupOutcome::Empty => {}
            }
        }

        if is_retract {
            return Ok(Admission::Retracted(withdrawn.or(recorded).unwrap_or(
                FactReceipt {
                    fact_id: String::new(),
                    outcome: FactOutcome::UnknownFact,
                },
            )));
        }
        if ambiguous {
            return Ok(Admission::Ambiguous);
        }
        match recorded {
            Some(receipt) if ingested_is_authoritative => Ok(Admission::Recorded(receipt)),
            Some(_) => Ok(Admission::Corroborating),
            None if display_only => Ok(Admission::DisplayOnly),
            None => Ok(Admission::Corroborating),
        }
    }

    /// Reconcile a scope, read-only, for display and diagnostics.
    pub fn reconcile(&self, scope_ref: &str) -> Vec<Reconciliation> {
        let observations: Vec<BoundObservation> = self
            .index
            .observations()
            .filter(|bound| bound.scope_ref() == scope_ref)
            .cloned()
            .collect();
        reconcile(&observations, &self.correlation)
    }

    /// Prepare one mounted panel from what the index already holds.
    ///
    /// The value comes from the settled report of each declared series, or from
    /// the single source that reported it. A series that is ambiguous or absent
    /// is prepared as unknown: a panel never picks a candidate out of a conflict
    /// and never draws a missing reading as zero. Nothing is read from a source
    /// and nothing is scraped, so a redraw costs a redraw.
    pub fn prepare_panel(
        &mut self,
        contribution_id: &str,
        scope_ref: &str,
    ) -> Result<(), ApplicationFailure> {
        self.require_active()?;
        let Some(contribution) = self.panels.mounted(contribution_id).cloned() else {
            return Err(refusal("analytics_panel_not_mounted").with_field("contributionId"));
        };
        let mut readings: BTreeMap<String, MetricValue> = BTreeMap::new();
        for series in &contribution.series {
            let observations: Vec<BoundObservation> = self
                .index
                .observations_for(scope_ref, &series.metric)
                .into_iter()
                .cloned()
                .collect();
            for outcome in reconcile(&observations, &self.correlation) {
                if let Reconciliation::Settled { authoritative, .. } = outcome
                    && let Some(reading) = authoritative.observation.metrics.get(&series.metric)
                {
                    readings.insert(series.metric.clone(), reading.clone());
                }
            }
        }
        let value = crate::panels::PreparedPanelValue::from_readings(
            &contribution,
            self.panels.generation(),
            &readings,
        );
        self.panels.install_prepared(value)
    }

    /// Read one bounded page of a source's observations, by cursor.
    ///
    /// This is a host-driven read of what the index already holds. It is not a
    /// scrape: no source is contacted, and the call is counted nowhere.
    pub fn pull(
        &mut self,
        source_ref: &str,
        request: &QueryRequest,
    ) -> Result<QueryPage, ApplicationFailure> {
        self.require_readable()?;
        let Some(binding) = self.sources.get(source_ref).cloned() else {
            return Err(self.authority.refusal(source_ref));
        };
        request.validate()?;
        if !binding.authorizes_scope(&request.scope_ref) {
            return Err(refusal("usage_scope_not_authorized").with_field("scopeRef"));
        }
        let start = match &request.cursor {
            None => 0usize,
            Some(text) => {
                let cursor = Cursor::parse(text)?;
                cursor.check_epoch(&binding.source_epoch)?;
                cursor.sequence() as usize
            }
        };
        let mut observations: Vec<&BoundObservation> = self
            .index
            .observations()
            .filter(|bound| {
                bound.source_ref() == source_ref && bound.scope_ref() == request.scope_ref
            })
            .collect();
        observations.sort_by_key(|bound| bound.key());
        let end = start
            .saturating_add(request.limit as usize)
            .min(observations.len());
        let page = if start >= observations.len() {
            QueryPage::empty(&binding.source_epoch, &request.scope_ref)
        } else {
            let next_cursor = if end < observations.len() {
                Some(Cursor::new(&binding.source_epoch, end as u64)?.encode())
            } else {
                None
            };
            QueryPage {
                source_epoch: binding.source_epoch.clone(),
                scope_ref: request.scope_ref.clone(),
                observations: observations[start..end]
                    .iter()
                    .map(|bound| bound.observation.clone())
                    .collect(),
                has_more: next_cursor.is_some(),
                next_cursor,
            }
        };
        page.validate()?;
        Ok(page)
    }

    /// Start a collector for one configured source.
    ///
    /// A collector exists only here: not installing the package means this
    /// method does not exist to call, and activating it with no configured
    /// source means there is nothing to collect.
    pub fn start_scraper(&mut self, source_ref: &str) -> Result<(), ApplicationFailure> {
        self.require_active()?;
        if !self.sources.contains_key(source_ref) {
            return Err(self.authority.refusal(source_ref));
        }
        if self.scrapers.insert(source_ref.to_owned()) {
            self.collection_attempts += 1;
        }
        Ok(())
    }

    /// Begin an in-flight read that uninstall must drain rather than cut off.
    pub fn begin_read(&mut self) -> Result<ReadTicket, ApplicationFailure> {
        self.require_readable()?;
        let ticket = ReadTicket { id: self.next_read };
        self.next_read += 1;
        self.open_reads.insert(ticket.id);
        Ok(ticket)
    }

    pub fn finish_read(&mut self, ticket: ReadTicket) {
        self.open_reads.remove(&ticket.id);
    }

    pub fn open_reads(&self) -> usize {
        self.open_reads.len()
    }

    /// Stop new admission and release everything the package can release now.
    ///
    /// The facts are untouched: they are the core's. If reads are still in
    /// flight the package stays [`PackageState::Draining`] and
    /// [`Self::complete_uninstall`] finishes once they are done.
    pub fn begin_uninstall(&mut self) -> Result<UninstallReport, ApplicationFailure> {
        self.state = crate::state_machine::analytics_package::transition(
            self.state,
            crate::state_machine::analytics_package::Event::BeginUninstall,
        )
        .ok_or_else(|| self.inactive_refusal())?;
        let withdrawn = self.panels.withdraw();
        let released_scrapers: Vec<String> = self.scrapers.iter().cloned().collect();
        self.scrapers.clear();
        Ok(UninstallReport {
            state: self.state,
            withdrawn_contributions: withdrawn.contributions,
            dropped_prepared: withdrawn.prepared,
            dropped_observations: 0,
            released_sources: Vec::new(),
            released_scrapers,
            facts_preserved: true,
        })
    }

    /// Drop the index and the source bindings once nothing is in flight.
    pub fn complete_uninstall(&mut self) -> Result<UninstallReport, ApplicationFailure> {
        if self.state != PackageState::Draining {
            return Err(self.inactive_refusal());
        }
        if !self.open_reads.is_empty() {
            return Err(refusal_with(
                "analytics_drain_pending",
                RecoveryAction::RetryAfterRecovery,
            )
            .with_presentation_arg("openReads", &self.open_reads.len().to_string()));
        }
        let dropped_observations = self.index.len();
        self.index = ObservationIndex::new();
        self.recorded.clear();
        let released_sources: Vec<String> = self.sources.keys().cloned().collect();
        self.sources.clear();
        self.state = crate::state_machine::analytics_package::transition(
            self.state,
            crate::state_machine::analytics_package::Event::CompleteUninstall,
        )
        .expect("a drained package can complete uninstall");
        Ok(UninstallReport {
            state: self.state,
            withdrawn_contributions: Vec::new(),
            dropped_prepared: 0,
            dropped_observations,
            released_sources,
            released_scrapers: Vec::new(),
            facts_preserved: true,
        })
    }

    /// Reconcile one scope and metric and apply the outcome to the core facts.
    fn apply_reconciliation(
        &mut self,
        scope_ref: &str,
        metric: &str,
        ingested: &licoup_usage_source_sdk::binding::BoundObservationKey,
    ) -> Result<GroupOutcome, ApplicationFailure> {
        let group: Vec<BoundObservation> = self
            .index
            .observations_for(scope_ref, metric)
            .into_iter()
            .cloned()
            .collect();
        let outcomes = reconcile(&group, &self.correlation);
        let mut fact_ids_now: BTreeSet<String> = BTreeSet::new();
        let mut recorded: Option<(FactReceipt, bool)> = None;
        let mut display_only = false;
        let mut ambiguous = false;

        for outcome in outcomes {
            match outcome {
                Reconciliation::Ambiguous { .. } => ambiguous = true,
                Reconciliation::Settled { authoritative, .. } => {
                    let identity = settlement_identity(&authoritative, &group);
                    let authoritative_is_ingested = authoritative.key() == *ingested;
                    let measurement_fact_id = fact_id(scope_ref, metric, &identity);
                    let reading = authoritative
                        .observation
                        .metrics
                        .get(metric)
                        .expect("the group was selected by this metric");
                    if self
                        .authority
                        .eligibility(authoritative.source_ref())
                        .is_settlement()
                    {
                        let receipt = self.facts.record(MeteringFact {
                            fact_id: measurement_fact_id.clone(),
                            scope_ref: scope_ref.to_owned(),
                            metric: metric.to_owned(),
                            value: reading.value.clone(),
                            unit: reading.unit.clone(),
                            quality: reading.quality,
                            eligibility: crate::facts::SettlementEligibility::SettlementEligible,
                            source_ref: authoritative.source_ref().to_owned(),
                            observed_at: authoritative.observation.observed_at.clone(),
                        });
                        fact_ids_now.insert(measurement_fact_id);
                        recorded = Some((receipt, authoritative_is_ingested));
                        if let Some(cost) = &authoritative.observation.cost {
                            let cost_fact_id = fact_id(scope_ref, general::COST, &identity);
                            self.facts.record(MeteringFact {
                                fact_id: cost_fact_id.clone(),
                                scope_ref: scope_ref.to_owned(),
                                metric: general::COST.to_owned(),
                                value: cost.amount.clone(),
                                unit: cost.currency.clone(),
                                quality: cost.quality,
                                eligibility:
                                    crate::facts::SettlementEligibility::SettlementEligible,
                                source_ref: authoritative.source_ref().to_owned(),
                                observed_at: authoritative.observation.observed_at.clone(),
                            });
                            fact_ids_now.insert(cost_fact_id);
                        }
                    } else {
                        display_only = true;
                    }
                }
            }
        }

        // A measurement that no longer has a settlement withdraws the facts it
        // had, and only those: a retraction cancels its own observation's effect
        // and nothing else.
        //
        // An ambiguity is not a withdrawal. When reports cannot be attributed to
        // one measurement or to distinct ones, nothing new is recorded, and the
        // last definite settlement is carried forward rather than erased —
        // otherwise a conflicting report would be a way to cancel a charge that
        // was already settled.
        let group_key = (scope_ref.to_owned(), metric.to_owned());
        let previous = self.recorded.remove(&group_key).unwrap_or_default();
        let mut withdrawn: Option<FactReceipt> = None;
        if ambiguous {
            fact_ids_now.extend(previous.iter().cloned());
        } else {
            for stale in previous.difference(&fact_ids_now) {
                let receipt = self.facts.retract(stale);
                withdrawn = Some(receipt);
            }
        }
        if fact_ids_now.is_empty() {
            self.recorded.remove(&group_key);
        } else {
            self.recorded.insert(group_key, fact_ids_now);
        }

        Ok(match recorded {
            Some((receipt, authoritative_is_ingested)) => GroupOutcome::Recorded {
                receipt,
                authoritative_is_ingested,
            },
            None if withdrawn.is_some() => GroupOutcome::Withdrawn(withdrawn.expect("checked")),
            None if display_only => GroupOutcome::DisplayOnly,
            None if ambiguous => GroupOutcome::Ambiguous,
            None => GroupOutcome::Empty,
        })
    }

    fn require_active(&self) -> Result<(), ApplicationFailure> {
        if self.state == PackageState::Active {
            return Ok(());
        }
        Err(self.inactive_refusal())
    }

    fn require_readable(&self) -> Result<(), ApplicationFailure> {
        if matches!(self.state, PackageState::Active) {
            return Ok(());
        }
        Err(self.inactive_refusal())
    }

    fn inactive_refusal(&self) -> ApplicationFailure {
        refusal_with(
            "analytics_package_inactive",
            RecoveryAction::InstallOrRetryRuntime,
        )
        .with_field("state")
        .with_presentation_arg("state", self.state.id())
    }
}

/// The identity one settled measurement's fact is keyed by.
///
/// A host-issued measurement is stable across the sources that report it; a
/// single source's observation is its own; an unlinked group resolved by
/// priority is one group identity, so a later higher-priority report updates the
/// same fact instead of adding a second one.
fn settlement_identity(authoritative: &BoundObservation, group: &[BoundObservation]) -> String {
    if let Some(measurement) = &authoritative.measurement_ref {
        return format!("measurement:{measurement}");
    }
    let sources: BTreeSet<&str> = group.iter().map(BoundObservation::source_ref).collect();
    if sources.len() == 1 {
        let key = authoritative.key();
        return format!("observation:{}:{}", key.source_ref, key.observation_id);
    }
    "unlinked".to_owned()
}

fn fact_id(scope_ref: &str, metric: &str, identity: &str) -> String {
    format!("{scope_ref}#{identity}#{metric}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::facts::{FactPage, PendingObligation};
    use crate::panels::PreparedPanelValue;
    use licoup_extension_contracts::ui::{Contribution, ContributionKind, Series};
    use licoup_extension_contracts::usage::{MetricValue, Quality, Temporality, UsageObservation};
    use std::collections::BTreeMap;

    #[derive(Default)]
    struct MemoryFacts {
        facts: BTreeMap<String, MeteringFact>,
        records: u64,
        retracts: u64,
        reads: std::cell::Cell<u64>,
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
            self.reads.set(self.reads.get() + 1);
            FactPage {
                facts: self.facts.values().take(limit).cloned().collect(),
                next_cursor: None,
            }
        }

        fn pending(&self) -> Vec<PendingObligation> {
            Vec::new()
        }
    }

    fn observation(
        source: &str,
        id: &str,
        scope: &str,
        value: &str,
        measurement: Option<&str>,
    ) -> BoundObservation {
        let observation = UsageObservation {
            schema: licoup_extension_contracts::wire::USAGE.to_owned(),
            observation_id: id.to_owned(),
            revision: 1,
            operation: UsageOperation::Upsert,
            source_epoch: "epoch-1".to_owned(),
            scope_ref: scope.to_owned(),
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
        SourceBinding::new(source, "example.agent", "instance-1", 1, "epoch-1", [scope])
            .expect("binding")
            .bind_observation(observation, measurement)
            .expect("bound")
    }

    fn retraction(source: &str, id: &str, scope: &str) -> BoundObservation {
        let observation = UsageObservation {
            schema: licoup_extension_contracts::wire::USAGE.to_owned(),
            observation_id: id.to_owned(),
            revision: 2,
            operation: UsageOperation::Retract,
            source_epoch: "epoch-1".to_owned(),
            scope_ref: scope.to_owned(),
            observed_at: "2026-09-21T00:01:00Z".to_owned(),
            interval_start: None,
            metrics: BTreeMap::new(),
            cost: None,
        };
        SourceBinding::new(source, "example.agent", "instance-1", 1, "epoch-1", [scope])
            .expect("binding")
            .bind_observation(observation, None)
            .expect("bound")
    }

    fn panel() -> Contribution {
        Contribution {
            schema: licoup_extension_contracts::wire::UI.to_owned(),
            id: "org.licoland.feature.analytics/usage-panel".to_owned(),
            kind: ContributionKind::MetricPanel,
            title: "Usage".to_owned(),
            required_profile: Some("usage-metric".to_owned()),
            resource_ref: None,
            resource_format: None,
            action_ref: None,
            fields: Vec::new(),
            series: vec![Series {
                metric: "licoup.tokens.input".to_owned(),
                label: "Input tokens".to_owned(),
                unit: "tokens".to_owned(),
            }],
        }
    }

    fn active<'a>(
        facts: &'a mut MemoryFacts,
        authority: AuthorityPolicy,
        correlation: CorrelationPolicy,
    ) -> AnalyticsPackage<'a, MemoryFacts> {
        AnalyticsPackage::activate(
            facts,
            PackageFacts::local_import(true),
            AnalyticsConfig::new(authority, correlation, 1),
        )
        .expect("active")
    }

    #[test]
    fn activating_requires_the_capability_and_starts_nothing_when_absent() {
        let mut facts = MemoryFacts::default();
        let failure = AnalyticsPackage::activate(
            &mut facts,
            PackageFacts::local_import(false),
            AnalyticsConfig::new(AuthorityPolicy::new(), CorrelationPolicy::none(), 1),
        )
        .expect_err("not enabled");
        assert_eq!(failure.code, "capability_unavailable");
        assert_eq!(failure.recovery, RecoveryAction::InstallOrRetryRuntime);
        assert_eq!(
            AnalyticsPackage::<MemoryFacts>::capability_state(PackageFacts::local_import(false))
                .describe(),
            "installed-not-enabled"
        );

        let mut active = active(
            &mut facts,
            AuthorityPolicy::new(),
            CorrelationPolicy::none(),
        );
        assert_eq!(active.state(), PackageState::Active);
        assert_eq!(active.scrapers(), 0);
        assert_eq!(active.collection_attempts(), 0);
        assert!(
            active
                .panels_mut()
                .mount(
                    &[panel()],
                    &[licoup_extension_contracts::profile::ExtensionProfile::UsageMetric]
                )
                .mounted
                .len()
                == 1
        );
    }

    #[test]
    fn one_call_from_three_sources_settles_once_and_corrects_in_place() {
        let mut facts = MemoryFacts::default();
        let mut package = active(
            &mut facts,
            AuthorityPolicy::host_configured([
                crate::policy::SourceGrant::new("source:graph#1", true),
                crate::policy::SourceGrant::new("source:agent#1", true),
                crate::policy::SourceGrant::new("source:gateway#1", true),
            ]),
            CorrelationPolicy::explicit(["source:gateway#1", "source:agent#1", "source:graph#1"]),
        );
        for source in ["source:graph#1", "source:agent#1", "source:gateway#1"] {
            package
                .configure_source(
                    SourceBinding::new(source, "example.agent", "i", 1, "epoch-1", ["scope-1"])
                        .expect("binding"),
                    true,
                )
                .expect("configured");
        }
        let first = package
            .admit(observation(
                "source:graph#1",
                "obs-graph",
                "scope-1",
                "120",
                Some("call-7"),
            ))
            .expect("admitted");
        assert!(matches!(first, Admission::Recorded(_)));
        let second = package
            .admit(observation(
                "source:gateway#1",
                "obs-gateway",
                "scope-1",
                "118",
                Some("call-7"),
            ))
            .expect("admitted");
        assert!(
            matches!(second, Admission::Recorded(_)),
            "the higher-priority report updates the same fact"
        );
        assert_eq!(package.facts().facts.len(), 1);
        let fact = package.facts().facts.values().next().expect("one fact");
        assert_eq!(fact.value.as_deref(), Some("118"));
        assert_eq!(fact.source_ref, "source:gateway#1");
        assert_eq!(package.facts().records, 2, "one fact, corrected once");
    }

    #[test]
    fn a_display_only_source_is_shown_but_never_settles() {
        let mut facts = MemoryFacts::default();
        let mut package = active(
            &mut facts,
            AuthorityPolicy::host_configured([crate::policy::SourceGrant::new(
                "source:agent#1",
                false,
            )]),
            CorrelationPolicy::none(),
        );
        package
            .configure_source(
                SourceBinding::new(
                    "source:agent#1",
                    "example.agent",
                    "i",
                    1,
                    "epoch-1",
                    ["scope-1"],
                )
                .expect("binding"),
                false,
            )
            .expect("configured");
        let admission = package
            .admit(observation(
                "source:agent#1",
                "obs-1",
                "scope-1",
                "120",
                None,
            ))
            .expect("admitted");
        assert_eq!(admission, Admission::DisplayOnly);
        assert!(package.facts().facts.is_empty());
        assert_eq!(package.facts().records, 0);
        assert_eq!(package.index().len(), 1, "still available for display");
    }

    #[test]
    fn an_unattributable_pair_records_nothing_new_and_erases_nothing() {
        let mut facts = MemoryFacts::default();
        let mut package = active(
            &mut facts,
            AuthorityPolicy::host_configured([
                crate::policy::SourceGrant::new("source:graph#1", true),
                crate::policy::SourceGrant::new("source:gateway#1", true),
            ]),
            CorrelationPolicy::none(),
        );
        for source in ["source:graph#1", "source:gateway#1"] {
            package
                .configure_source(
                    SourceBinding::new(source, "example.agent", "i", 1, "epoch-1", ["scope-1"])
                        .expect("binding"),
                    true,
                )
                .expect("configured");
        }
        package
            .admit(observation(
                "source:graph#1",
                "obs-graph",
                "scope-1",
                "120",
                None,
            ))
            .expect("admitted");
        let admission = package
            .admit(observation(
                "source:gateway#1",
                "obs-gateway",
                "scope-1",
                "118",
                None,
            ))
            .expect("admitted");
        assert_eq!(admission, Admission::Ambiguous);
        assert_eq!(
            package.facts().facts.len(),
            1,
            "the settled charge stands; a conflicting report is not a retraction"
        );
        assert_eq!(
            package
                .facts()
                .facts
                .values()
                .next()
                .expect("one fact")
                .value
                .as_deref(),
            Some("120")
        );
        assert!(matches!(
            package.reconcile("scope-1").as_slice(),
            [Reconciliation::Ambiguous { candidates, .. }] if candidates.len() == 2
        ));
    }

    #[test]
    fn a_retraction_withdraws_its_own_fact_only() {
        let mut facts = MemoryFacts::default();
        let mut package = active(
            &mut facts,
            AuthorityPolicy::host_configured([crate::policy::SourceGrant::new(
                "source:agent#1",
                true,
            )]),
            CorrelationPolicy::none(),
        );
        package
            .configure_source(
                SourceBinding::new(
                    "source:agent#1",
                    "example.agent",
                    "i",
                    1,
                    "epoch-1",
                    ["scope-1"],
                )
                .expect("binding"),
                true,
            )
            .expect("configured");
        package
            .admit(observation(
                "source:agent#1",
                "obs-1",
                "scope-1",
                "120",
                None,
            ))
            .expect("admitted");
        assert_eq!(package.facts().facts.len(), 1);
        let admission = package
            .admit(retraction("source:agent#1", "obs-1", "scope-1"))
            .expect("admitted");
        match admission {
            Admission::Retracted(receipt) => assert_eq!(receipt.outcome, FactOutcome::Withdrawn),
            other => panic!("expected a withdrawal, got {other:?}"),
        }
        assert!(package.facts().facts.is_empty());
    }

    #[test]
    fn uninstall_releases_panels_and_sources_but_not_facts() {
        let mut facts = MemoryFacts::default();
        let mut package = active(
            &mut facts,
            AuthorityPolicy::host_configured([crate::policy::SourceGrant::new(
                "source:agent#1",
                true,
            )]),
            CorrelationPolicy::none(),
        );
        package
            .configure_source(
                SourceBinding::new(
                    "source:agent#1",
                    "example.agent",
                    "i",
                    1,
                    "epoch-1",
                    ["scope-1"],
                )
                .expect("binding"),
                true,
            )
            .expect("configured");
        package.start_scraper("source:agent#1").expect("scraper");
        package.panels_mut().mount(
            &[panel()],
            &[licoup_extension_contracts::profile::ExtensionProfile::UsageMetric],
        );
        package
            .panels_mut()
            .install_prepared(PreparedPanelValue::from_readings(
                &panel(),
                1,
                &BTreeMap::new(),
            ))
            .expect("prepared");
        package
            .admit(observation(
                "source:agent#1",
                "obs-1",
                "scope-1",
                "120",
                None,
            ))
            .expect("admitted");
        assert_eq!(package.facts().facts.len(), 1);

        let ticket = package.begin_read().expect("read");
        let drain = package.begin_uninstall().expect("draining");
        assert_eq!(drain.state, PackageState::Draining);
        assert_eq!(drain.withdrawn_contributions.len(), 1);
        assert_eq!(drain.dropped_prepared, 1);
        assert_eq!(drain.released_scrapers.len(), 1);
        assert!(drain.facts_preserved);
        assert!(matches!(
            package.admit(observation("source:agent#1", "obs-2", "scope-1", "10", None)),
            Err(failure) if failure.code == "analytics_package_inactive"
        ));
        assert_eq!(
            package
                .complete_uninstall()
                .expect_err("drain pending")
                .code,
            "analytics_drain_pending"
        );

        package.finish_read(ticket);
        let done = package.complete_uninstall().expect("complete");
        assert_eq!(done.state, PackageState::Removed);
        assert_eq!(done.dropped_observations, 1);
        assert_eq!(done.released_sources, vec!["source:agent#1".to_owned()]);
        assert!(done.facts_preserved);
        drop(package);
        assert_eq!(
            facts.facts.len(),
            1,
            "the core fact survives every surface of the package"
        );
    }

    /// The lifecycle the package follows is the one it declares.
    ///
    /// The table lives in `resources/state-machines/package.json` and is
    /// compiled by `build.rs`, so a declaration this module does not follow is a
    /// build failure rather than a comment that has drifted.
    #[test]
    fn the_declared_state_machine_is_the_only_lifecycle_the_package_follows() {
        use crate::state_machine::analytics_package as machine;

        assert_eq!(machine::MACHINE_ID, "analytics.package");
        assert_eq!(machine::INITIAL, PackageState::NotInstalled);
        assert_eq!(machine::ALL_STATES.len(), 4);
        assert_eq!(machine::ALL_EVENTS.len(), 3);

        // The declared edges, and only those.
        assert_eq!(
            machine::transition(PackageState::NotInstalled, machine::Event::Activate),
            Some(PackageState::Active)
        );
        assert_eq!(
            machine::transition(PackageState::Active, machine::Event::BeginUninstall),
            Some(PackageState::Draining)
        );
        assert_eq!(
            machine::transition(PackageState::Draining, machine::Event::BeginUninstall),
            Some(PackageState::Draining),
            "asking twice to drain is the same state, not a second transition"
        );
        assert_eq!(
            machine::transition(PackageState::Draining, machine::Event::CompleteUninstall),
            Some(PackageState::Removed)
        );
        assert_eq!(
            machine::transition(PackageState::Removed, machine::Event::Activate),
            None,
            "a removed package is not reactivated in place"
        );
        assert_eq!(
            machine::transition(PackageState::Active, machine::Event::CompleteUninstall),
            None,
            "collecting before draining is not a representable transition"
        );

        // The wire names the host reads are the declaration's own spelling.
        assert_eq!(PackageState::Active.id(), "active");
        assert_eq!(PackageState::Draining.id(), "draining");
        assert_eq!(
            PackageState::from_name("removed"),
            Some(PackageState::Removed)
        );
        assert_eq!(PackageState::from_name("unknown"), None);

        // The package moves exactly there: activation lands on the declared
        // state, collection before the drain is refused, and a removed package
        // cannot be uninstalled a second time.
        let mut facts = MemoryFacts::default();
        let mut package = active(
            &mut facts,
            AuthorityPolicy::new(),
            CorrelationPolicy::none(),
        );
        assert_eq!(package.state(), PackageState::Active);
        assert_eq!(
            package.complete_uninstall().expect_err("not draining").code,
            "analytics_package_inactive"
        );
        package.begin_uninstall().expect("draining");
        package.complete_uninstall().expect("removed");
        assert_eq!(package.state(), PackageState::Removed);
        let refusal = package.begin_uninstall().expect_err("removed once");
        assert_eq!(refusal.code, "analytics_package_inactive");
        assert_eq!(refusal.presentation_args.get("state"), Some("removed"));
        assert_eq!(
            package.complete_uninstall().expect_err("nothing left").code,
            "analytics_package_inactive"
        );
    }

    #[test]
    fn a_pull_pages_a_source_without_scraping_and_a_stale_cursor_is_refused() {
        let mut facts = MemoryFacts::default();
        let mut package = active(
            &mut facts,
            AuthorityPolicy::host_configured([crate::policy::SourceGrant::new(
                "source:agent#1",
                true,
            )]),
            CorrelationPolicy::none(),
        );
        package
            .configure_source(
                SourceBinding::new(
                    "source:agent#1",
                    "example.agent",
                    "i",
                    1,
                    "epoch-1",
                    ["scope-1"],
                )
                .expect("binding"),
                true,
            )
            .expect("configured");
        for index in 0..3 {
            package
                .admit(observation(
                    "source:agent#1",
                    &format!("obs-{index}"),
                    "scope-1",
                    "1",
                    None,
                ))
                .expect("admitted");
        }
        let first = package
            .pull(
                "source:agent#1",
                &QueryRequest::new("scope-1", None, Some(2)).expect("request"),
            )
            .expect("page");
        assert_eq!(first.observations.len(), 2);
        assert!(first.has_more);
        assert_eq!(package.collection_attempts(), 0, "a read is not a scrape");
        let second = package
            .pull(
                "source:agent#1",
                &QueryRequest::new("scope-1", first.next_cursor.clone(), Some(2)).expect("request"),
            )
            .expect("page");
        assert_eq!(second.observations.len(), 1);
        assert!(!second.has_more);

        let stale = Cursor::new("epoch-0", 0).expect("cursor").encode();
        assert_eq!(
            package
                .pull(
                    "source:agent#1",
                    &QueryRequest::new("scope-1", Some(stale), Some(2)).expect("request")
                )
                .expect_err("stale cursor")
                .code,
            "usage_cursor_epoch_stale"
        );
    }
}
