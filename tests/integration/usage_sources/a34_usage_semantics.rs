//! A34 · statistics source semantics and trust — component-integration evidence.
//!
//! Every case here is synthetic: a specialist Agent that reports only its own
//! metrics, an Agent that reports no tokens, a log line, an OTLP-shaped
//! cumulative series, and several sources reporting one invocation. The oracle
//! is the acceptance's own wording:
//!
//! > deduplicate by observation identity; a correction replaces rather than adds;
//! > no double charge; an unknown is never drawn as zero; a producer's
//! > self-reported quality never becomes authoritative settlement.
//!
//! The level is component-integration: the SDK and the analytics package are
//! real, the sources and the core fact port are doubles. No account, ledger or
//! usage file is read.

use crate::support::{
    MemoryFacts, binding, cost_observation, log_line, observation, otlp_cumulative, otlp_regressed,
    otlp_restarted, retraction, specialist_mapping, specialist_record, usage_panel,
    wire_observation,
};
use licoup_analytics::correlation::CorrelationPolicy;
use licoup_analytics::index::{CounterStep, ObservationIndex};
use licoup_analytics::metrics::{Aggregate, MetricCatalog, MetricDefinition};
use licoup_analytics::package::{Admission, AnalyticsConfig, AnalyticsPackage};
use licoup_analytics::policy::{AuthorityPolicy, SourceGrant};
use licoup_extension_contracts::deployment::PackageFacts;
use licoup_extension_contracts::profile::ExtensionProfile;
use licoup_extension_contracts::usage::{Quality, Temporality};
use licoup_usage_source_sdk::binding::SourceBinding;
use licoup_usage_source_sdk::collection::{MAX_BATCH_OBSERVATIONS, PublishBatch, QueryRequest};
use licoup_usage_source_sdk::describe::{
    Aggregation, MetricField, SeriesDeclaration, SourceDescription,
};
use licoup_usage_source_sdk::metrics as general;
use licoup_usage_source_sdk::normalize::OtlpSumNormalizer;

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
    .expect("the analytics capability is served")
}

fn configured(
    package: &mut AnalyticsPackage<'_, MemoryFacts>,
    source: &str,
    scope: &str,
    settle: bool,
) {
    package
        .configure_source(binding(source, scope), settle)
        .expect("configured");
}

fn three_sources() -> AuthorityPolicy {
    AuthorityPolicy::host_configured([
        SourceGrant::new("source:graph#1", true),
        SourceGrant::new("source:agent#1", true),
        SourceGrant::new("source:gateway#1", true),
    ])
}

#[test]
fn a_specialist_agent_without_tokens_is_accepted_and_tokens_stay_unknown() {
    let mut facts = MemoryFacts::new();
    let mut package = active(
        &mut facts,
        AuthorityPolicy::host_configured([SourceGrant::new("source:specialist#1", true)]),
        CorrelationPolicy::none(),
    );
    configured(&mut package, "source:specialist#1", "run-1", true);

    // The boundary adapter maps a real record shape; the mapping declares that
    // this Agent does not report tokens, so "no tokens" is unknown, not zero.
    let record = specialist_record("run-1", 42, 1500, Some("0.001250"));
    let bound = specialist_mapping()
        .normalize(&record, &binding("source:specialist#1", "run-1"), None)
        .expect("mapped")
        .expect("the record claims metrics");
    let admission = package.admit(bound).expect("admitted");
    assert!(matches!(admission, Admission::Recorded(_)));

    let tokens = package
        .facts()
        .fact("run-1#observation:source:specialist#1:run-1#licoup.tokens.total")
        .expect("the call is known even though its token count is not");
    assert_eq!(tokens.value, None, "an unknown count carries no number");
    assert_eq!(tokens.quality, Quality::Unknown);
    assert_eq!(
        package
            .facts()
            .settled_total("run-1", "example.specialist/items")
            .as_deref(),
        Some("42")
    );
    let cost = package
        .facts()
        .fact("run-1#observation:source:specialist#1:run-1#licoup.cost")
        .expect("the estimated cost is recorded as its own fact");
    assert_eq!(cost.value.as_deref(), Some("0.001250"));
    assert_eq!(cost.unit, "USD");
    assert_eq!(cost.quality, Quality::Estimated);

    // The same record read from a log line lands on the same mapping.
    let from_log = licoup_usage_source_sdk::normalize::parse_key_value_line(log_line())
        .expect("a key=value line");
    let mapped = specialist_mapping()
        .normalize(&from_log, &binding("source:specialist#1", "run-1"), None)
        .expect("mapped")
        .expect("claims");
    assert_eq!(
        mapped.observation.metrics["example.specialist/items"]
            .value
            .as_deref(),
        Some("7")
    );

    // The panel the package contributes shows the known metric and prepares the
    // unreported ones as unknown.
    let report = package
        .panels_mut()
        .mount(&[usage_panel()], &[ExtensionProfile::UsageMetric]);
    assert_eq!(report.mounted.len(), 1);
    package
        .prepare_panel("org.licoland.feature.analytics/usage-panel", "run-1")
        .expect("prepared");
    let prepared = package
        .panels()
        .prepared("org.licoland.feature.analytics/usage-panel")
        .expect("prepared value");
    let items = prepared
        .points
        .iter()
        .find(|point| point.metric == "example.specialist/items")
        .expect("declared series");
    assert_eq!(items.value.as_deref(), Some("42"));
    let input = prepared
        .points
        .iter()
        .find(|point| point.metric == "licoup.tokens.input")
        .expect("declared series");
    assert_eq!(input.value, None);
    assert_eq!(input.quality, Quality::Unknown, "not reported, not zero");

    // A redraw is a redraw: no source is contacted and no fact is re-read.
    let reads_before = package.facts().read_calls();
    let attempts_before = package.collection_attempts();
    for _ in 0..5 {
        package
            .prepare_panel("org.licoland.feature.analytics/usage-panel", "run-1")
            .expect("prepared");
    }
    assert_eq!(
        package.facts().read_calls(),
        reads_before,
        "a redraw does not re-scan"
    );
    assert_eq!(package.collection_attempts(), attempts_before);
}

#[test]
fn push_bounded_pull_and_cursor_all_land_on_one_identity() {
    let mut facts = MemoryFacts::new();
    let mut package = active(
        &mut facts,
        AuthorityPolicy::host_configured([SourceGrant::new("source:agent#1", true)]),
        CorrelationPolicy::none(),
    );
    configured(&mut package, "source:agent#1", "scope-1", true);

    // push: one bounded batch, bound by the transport, mapped by the host.
    let batch = PublishBatch::new(vec![
        wire_observation("epoch-1", "scope-1", "obs-1", "licoup.tokens.input", "10"),
        wire_observation("epoch-1", "scope-1", "obs-2", "licoup.tokens.input", "20"),
        wire_observation("epoch-1", "scope-1", "obs-3", "licoup.tokens.input", "30"),
    ])
    .expect("a bounded batch");
    let admission = binding("source:agent#1", "scope-1").admit_batch(&batch);
    assert!(admission.is_complete());
    for bound in admission.accepted {
        package.admit(bound).expect("admitted");
    }
    assert_eq!(package.facts().len(), 3);
    assert_eq!(
        package
            .facts()
            .settled_total("scope-1", "licoup.tokens.input")
            .as_deref(),
        Some("60")
    );

    // bounded pull, then cursor: the same identities come back in pages.
    let first = package
        .pull(
            "source:agent#1",
            &QueryRequest::new("scope-1", None, Some(2)).expect("request"),
        )
        .expect("page");
    assert_eq!(first.observations.len(), 2);
    assert!(first.has_more);
    let second = package
        .pull(
            "source:agent#1",
            &QueryRequest::new("scope-1", first.next_cursor.clone(), Some(2)).expect("request"),
        )
        .expect("page");
    assert_eq!(second.observations.len(), 1);
    assert!(!second.has_more);
    assert_eq!(
        package.collection_attempts(),
        0,
        "reading the index is not scraping a source"
    );

    // A cursor from a replaced epoch is refused instead of silently resuming.
    let stale = licoup_usage_source_sdk::collection::Cursor::new("epoch-0", 0)
        .expect("cursor")
        .encode();
    let failure = package
        .pull(
            "source:agent#1",
            &QueryRequest::new("scope-1", Some(stale), Some(2)).expect("request"),
        )
        .expect_err("stale cursor");
    assert_eq!(failure.code, "usage_cursor_epoch_stale");

    // A batch beyond the count bound is refused before anything parses it.
    let oversize: Vec<_> = (0..=MAX_BATCH_OBSERVATIONS)
        .map(|index| {
            wire_observation(
                "epoch-1",
                "scope-1",
                &format!("obs-{index}"),
                "licoup.tokens.input",
                "1",
            )
        })
        .collect();
    assert_eq!(
        PublishBatch::new(oversize)
            .expect_err("over the bound")
            .code,
        "usage_batch_oversize"
    );
}

#[test]
fn a_replay_changes_nothing_and_a_correction_replaces_in_place() {
    let mut facts = MemoryFacts::new();
    let mut package = active(
        &mut facts,
        AuthorityPolicy::host_configured([SourceGrant::new("source:agent#1", true)]),
        CorrelationPolicy::none(),
    );
    configured(&mut package, "source:agent#1", "scope-1", true);

    let delta = |id: &str, revision: u32, value: &str| {
        observation(
            "source:agent#1",
            "scope-1",
            id,
            revision,
            "licoup.tokens.input",
            Some(value),
            "tokens",
            Temporality::Delta,
            Quality::Reported,
            None,
            "2026-09-21T00:00:00Z",
            None,
        )
    };
    assert!(matches!(
        package.admit(delta("obs-1", 1, "100")).expect("admitted"),
        Admission::Recorded(_)
    ));
    let records_after_first = package.facts().records;
    assert_eq!(
        package.admit(delta("obs-1", 1, "100")).expect("admitted"),
        Admission::Stale
    );
    assert_eq!(
        package.facts().records,
        records_after_first,
        "a replay records nothing"
    );
    let corrected = package.admit(delta("obs-1", 2, "120")).expect("admitted");
    match corrected {
        Admission::Recorded(receipt) => assert_eq!(
            receipt.outcome,
            licoup_analytics::facts::FactOutcome::Updated
        ),
        other => panic!("expected an in-place correction, got {other:?}"),
    }
    assert_eq!(
        package.facts().len(),
        1,
        "a correction replaces rather than adds"
    );
    assert_eq!(
        package
            .facts()
            .settled_total("scope-1", "licoup.tokens.input")
            .as_deref(),
        Some("120"),
        "not 100 + 120"
    );

    // Out-of-order: an older revision arriving late does not overwrite.
    assert_eq!(
        package.admit(delta("obs-1", 1, "100")).expect("admitted"),
        Admission::Stale
    );
    assert_eq!(
        package
            .facts()
            .settled_total("scope-1", "licoup.tokens.input")
            .as_deref(),
        Some("120")
    );

    let withdrawn = package
        .admit(retraction("source:agent#1", "scope-1", "obs-1", 3))
        .expect("admitted");
    match withdrawn {
        Admission::Retracted(receipt) => assert_eq!(
            receipt.outcome,
            licoup_analytics::facts::FactOutcome::Withdrawn
        ),
        other => panic!("expected a withdrawal, got {other:?}"),
    }
    assert!(
        package.facts().is_empty(),
        "a retraction cancels its own observation"
    );
}

#[test]
fn a_cumulative_series_is_differenced_in_order_and_a_reset_is_not_negative_usage() {
    let mut facts = MemoryFacts::new();
    let mut package = active(
        &mut facts,
        AuthorityPolicy::host_configured([SourceGrant::new("source:vendor#1", true)]),
        CorrelationPolicy::none(),
    );
    configured(&mut package, "source:vendor#1", "scope-1", true);

    // The OTLP-shaped adapter normalizes at the boundary; the host binds it.
    let normalizer =
        OtlpSumNormalizer::new("example.vendor", Quality::Reported).expect("normalizer");
    let source = binding("source:vendor#1", "scope-1");
    let series = normalizer
        .normalize(&otlp_cumulative(), &source)
        .expect("normalized");
    assert_eq!(series.len(), 2);
    let mut index = ObservationIndex::new();
    assert_eq!(
        index
            .counter_step(&series[0], "example.vendor/tokens.input")
            .expect("step"),
        CounterStep::FirstReading
    );
    assert_eq!(
        index
            .counter_step(&series[1], "example.vendor/tokens.input")
            .expect("step"),
        CounterStep::Delta(
            licoup_extension_contracts::usage::ExactNumber::parse("75").expect("75")
        ),
        "100 -> 175 is 75 tokens of work"
    );

    // A late point is placed by observation time, not by arrival time.
    let late = observation(
        "source:vendor#1",
        "scope-1",
        "late",
        1,
        "example.vendor/tokens.input",
        Some("140"),
        "tokens",
        Temporality::Cumulative,
        Quality::Reported,
        Some("2026-09-21T00:00:00Z"),
        "2026-09-21T00:07:00Z",
        None,
    );
    assert_eq!(
        index
            .counter_step(&late, "example.vendor/tokens.input")
            .expect("step"),
        CounterStep::Delta(
            licoup_extension_contracts::usage::ExactNumber::parse("40").expect("40")
        )
    );

    // A producer restart with a new origin starts a new series, and a counter
    // that went down inside one origin is a reset, never a negative amount.
    let restarted = normalizer
        .normalize(&otlp_restarted(), &source)
        .expect("normalized");
    assert_eq!(
        index
            .counter_step(&restarted[0], "example.vendor/tokens.input")
            .expect("step"),
        CounterStep::FirstReading
    );
    let regressed = normalizer
        .normalize(&otlp_regressed(), &source)
        .expect("normalized");
    assert!(matches!(
        index
            .counter_step(&regressed[0], "example.vendor/tokens.input")
            .expect("step"),
        CounterStep::Reset { .. }
    ));

    // Admitting the series still records a fact per reading; nothing is
    // negative and nothing is summed as if it were a delta.
    for bound in series
        .iter()
        .chain(restarted.iter())
        .chain(regressed.iter())
    {
        package.admit(bound.clone()).expect("admitted");
    }
    assert!(
        facts
            .facts()
            .all(|fact| fact.value.is_none() || !fact.exact().expect("exact").is_negative()),
        "no negative usage is recorded"
    );
}

#[test]
fn one_call_reported_by_graph_agent_and_gateway_settles_once() {
    let mut facts = MemoryFacts::new();
    let mut package = active(
        &mut facts,
        three_sources(),
        CorrelationPolicy::explicit(["source:gateway#1", "source:agent#1", "source:graph#1"]),
    );
    for source in ["source:graph#1", "source:agent#1", "source:gateway#1"] {
        configured(&mut package, source, "invocation-1", true);
    }
    let report = |source: &str, id: &str, value: &str, revision: u32| {
        observation(
            source,
            "invocation-1",
            id,
            revision,
            "licoup.tokens.input",
            Some(value),
            "tokens",
            Temporality::Delta,
            Quality::Reported,
            None,
            "2026-09-21T00:00:00Z",
            Some("call-7"),
        )
    };

    assert!(matches!(
        package
            .admit(report("source:graph#1", "obs-graph", "120", 1))
            .expect("admitted"),
        Admission::Recorded(_)
    ));
    assert!(matches!(
        package
            .admit(report("source:agent#1", "obs-agent", "120", 1))
            .expect("admitted"),
        Admission::Recorded(_)
    ));
    assert!(matches!(
        package
            .admit(report("source:gateway#1", "obs-gateway", "118", 1))
            .expect("admitted"),
        Admission::Recorded(_)
    ));
    assert_eq!(
        package.facts().len(),
        1,
        "three reports, one call, one fact"
    );
    assert_eq!(
        package
            .facts()
            .settled_total("invocation-1", "licoup.tokens.input")
            .as_deref(),
        Some("118"),
        "the explicit priority decides which report is authoritative"
    );
    let fact = package.facts().facts().next().expect("one fact");
    assert_eq!(fact.source_ref, "source:gateway#1");

    // A correction of the authoritative report updates the same fact.
    package
        .admit(report("source:gateway#1", "obs-gateway", "130", 2))
        .expect("admitted");
    assert_eq!(package.facts().len(), 1);
    assert_eq!(
        package
            .facts()
            .settled_total("invocation-1", "licoup.tokens.input")
            .as_deref(),
        Some("130")
    );

    // Retracting the authoritative report leaves the other reports standing:
    // the charge falls back to the next report instead of disappearing or
    // doubling.
    package
        .admit(retraction(
            "source:gateway#1",
            "invocation-1",
            "obs-gateway",
            3,
        ))
        .expect("admitted");
    assert_eq!(package.facts().len(), 1);
    assert_eq!(
        package
            .facts()
            .settled_total("invocation-1", "licoup.tokens.input")
            .as_deref(),
        Some("120"),
        "the Agent's report takes over the same measurement"
    );
}

#[test]
fn one_source_reporting_one_measurement_twice_settles_once() {
    let mut facts = MemoryFacts::new();
    let mut package = active(
        &mut facts,
        AuthorityPolicy::host_configured([SourceGrant::new("source:agent#1", true)]),
        CorrelationPolicy::none(),
    );
    configured(&mut package, "source:agent#1", "invocation-1", true);
    let report = |id: &str, revision: u32, value: &str| {
        observation(
            "source:agent#1",
            "invocation-1",
            id,
            revision,
            "licoup.tokens.input",
            Some(value),
            "tokens",
            Temporality::Delta,
            Quality::Reported,
            None,
            "2026-09-21T00:00:00Z",
            Some("call-9"),
        )
    };
    package.admit(report("obs-1", 1, "100")).expect("admitted");
    package.admit(report("obs-2", 2, "120")).expect("admitted");
    assert_eq!(
        package.facts().len(),
        1,
        "the same measurement from one source is one call"
    );
    assert_eq!(
        package
            .facts()
            .settled_total("invocation-1", "licoup.tokens.input")
            .as_deref(),
        Some("120"),
        "the higher revision is what the call says"
    );
}

#[test]
fn reports_that_cannot_be_attributed_are_ambiguous_and_never_summed() {
    let mut facts = MemoryFacts::new();
    let mut package = active(&mut facts, three_sources(), CorrelationPolicy::none());
    for source in ["source:graph#1", "source:gateway#1"] {
        configured(&mut package, source, "invocation-1", true);
    }
    let unlinked = |source: &str, id: &str, value: &str| {
        observation(
            source,
            "invocation-1",
            id,
            1,
            "licoup.tokens.input",
            Some(value),
            "tokens",
            Temporality::Delta,
            Quality::Reported,
            None,
            "2026-09-21T00:00:00Z",
            None,
        )
    };
    package
        .admit(unlinked("source:graph#1", "obs-graph", "120"))
        .expect("admitted");
    let admission = package
        .admit(unlinked("source:gateway#1", "obs-gateway", "118"))
        .expect("admitted");
    assert_eq!(admission, Admission::Ambiguous);
    assert_eq!(
        package.facts().len(),
        1,
        "the reports are not added together"
    );
    assert_eq!(
        package
            .facts()
            .settled_total("invocation-1", "licoup.tokens.input")
            .as_deref(),
        Some("120"),
        "the earlier settlement stands; a conflicting report is not a retraction"
    );

    // With an explicit priority the same pair settles once, on one report.
    let mut facts = MemoryFacts::new();
    let mut package = active(
        &mut facts,
        three_sources(),
        CorrelationPolicy::explicit(["source:gateway#1", "source:graph#1"]),
    );
    for source in ["source:graph#1", "source:gateway#1"] {
        configured(&mut package, source, "invocation-1", true);
    }
    package
        .admit(unlinked("source:graph#1", "obs-graph", "120"))
        .expect("admitted");
    package
        .admit(unlinked("source:gateway#1", "obs-gateway", "118"))
        .expect("admitted");
    assert_eq!(package.facts().len(), 1);
    assert_eq!(
        package
            .facts()
            .settled_total("invocation-1", "licoup.tokens.input")
            .as_deref(),
        Some("118")
    );
}

#[test]
fn a_cached_subset_is_never_added_to_the_total_that_contains_it() {
    let mut facts = MemoryFacts::new();
    let mut package = active(
        &mut facts,
        AuthorityPolicy::host_configured([SourceGrant::new("source:agent#1", true)]),
        CorrelationPolicy::none(),
    );
    configured(&mut package, "source:agent#1", "scope-1", true);

    // The subset relation is declared once and shared.
    assert!(general::includes(
        general::TOKENS_TOTAL,
        general::TOKENS_CACHED_INPUT
    ));
    assert!(general::includes(
        general::TOKENS_INPUT,
        general::TOKENS_CACHED_INPUT
    ));
    assert!(!general::includes(
        general::TOKENS_OUTPUT,
        general::TOKENS_INPUT
    ));

    let mut input = observation(
        "source:agent#1",
        "scope-1",
        "obs-input",
        1,
        general::TOKENS_INPUT,
        Some("1000"),
        "tokens",
        Temporality::Delta,
        Quality::Reported,
        None,
        "2026-09-21T00:00:00Z",
        None,
    );
    input
        .observation
        .metrics
        .get_mut(general::TOKENS_INPUT)
        .expect("metric")
        .includes = vec![general::TOKENS_CACHED_INPUT.to_owned()];
    package.admit(input).expect("admitted");
    package
        .admit(observation(
            "source:agent#1",
            "scope-1",
            "obs-cached",
            1,
            general::TOKENS_CACHED_INPUT,
            Some("400"),
            "tokens",
            Temporality::Delta,
            Quality::Reported,
            None,
            "2026-09-21T00:00:00Z",
            None,
        ))
        .expect("admitted");
    assert_eq!(
        package
            .facts()
            .settled_total("scope-1", general::TOKENS_INPUT)
            .as_deref(),
        Some("1000"),
        "the cached subset is inside the input total, not beside it"
    );

    // A panel or a total that would add them is refused by the catalog.
    let catalog = MetricCatalog::with_general();
    let failure = catalog
        .additive_check(&[general::TOKENS_TOTAL, general::TOKENS_CACHED_INPUT])
        .expect_err("a total contains its subset");
    assert_eq!(failure.code, "analytics_double_count_series");
    assert_eq!(
        failure.presentation_args.get("contains"),
        Some(general::TOKENS_TOTAL)
    );
    assert!(
        catalog
            .additive_check(&[general::TOKENS_INPUT, general::TOKENS_OUTPUT])
            .is_ok()
    );
    assert!(
        catalog
            .additive_check(&[general::TOKENS_INPUT, general::DURATION])
            .is_err(),
        "tokens and milliseconds are not added"
    );
}

#[test]
fn a_producers_self_reported_quality_never_becomes_authority() {
    let mut facts = MemoryFacts::new();
    let mut package = active(
        &mut facts,
        AuthorityPolicy::host_configured([SourceGrant::new("source:agent#1", false)]),
        CorrelationPolicy::none(),
    );
    let source = SourceBinding::new(
        "source:agent#1",
        "example.analytics-source",
        "instance-1",
        1,
        "epoch-1",
        ["scope-1", "scope-2"],
    )
    .expect("binding");
    package
        .configure_source(source.clone(), false)
        .expect("configured");

    // The source says "reported" and sends an exact cost; the host has not
    // granted settlement, so nothing settles.
    let admission = package
        .admit(cost_observation(
            "source:agent#1",
            "scope-1",
            "obs-1",
            Some("1.25"),
            "USD",
            Quality::Reported,
            None,
        ))
        .expect("admitted");
    assert_eq!(admission, Admission::DisplayOnly);
    assert!(package.facts().is_empty());
    assert_eq!(package.facts().records, 0);
    let settled = package.reconcile("scope-1");
    assert_eq!(settled.len(), 1, "the tokens reading is still visible");
    assert!(settled.iter().all(|item| matches!(
        item,
        licoup_analytics::correlation::Reconciliation::Settled { .. }
    )));

    // The host grants settlement and the same source can now settle: authority
    // came from the policy, not from the payload. The new report is in another
    // scope, so the grant's effect is visible on its own.
    package.configure_source(source, true).expect("granted");
    let admission = package
        .admit(observation(
            "source:agent#1",
            "scope-2",
            "obs-2",
            1,
            "licoup.tokens.input",
            Some("10"),
            "tokens",
            Temporality::Delta,
            Quality::Estimated,
            None,
            "2026-09-21T00:01:00Z",
            None,
        ))
        .expect("admitted");
    assert!(matches!(admission, Admission::Recorded(_)));
    assert_eq!(package.facts().len(), 1);
    let fact = package.facts().facts().next().expect("one fact");
    assert_eq!(fact.scope_ref, "scope-2");
    assert_eq!(fact.quality, Quality::Estimated);
    assert_eq!(
        fact.eligibility,
        licoup_analytics::facts::SettlementEligibility::SettlementEligible
    );
}

#[test]
fn a_specialist_declares_its_fields_and_the_panel_may_only_draw_them() {
    let description = SourceDescription::new(
        vec![
            MetricField::new(
                "example.specialist/items",
                "items",
                Temporality::Delta,
                Aggregation::Sum,
            ),
            MetricField::new(
                "example.specialist/latency",
                "ms",
                Temporality::Gauge,
                Aggregation::Quantile,
            ),
        ],
        vec![
            SeriesDeclaration::new("example.specialist/items", "Items", "items"),
            SeriesDeclaration::new("example.specialist/latency", "Latency", "ms"),
        ],
    );
    description.validate().expect("a well-formed declaration");

    // The catalog adopts the source's own declaration: a specialist metric is
    // defined, not merely named.
    let mut catalog = MetricCatalog::with_general();
    catalog.declare_source(&description).expect("adopted");
    assert!(catalog.definition("example.specialist/items").is_some());
    assert_eq!(
        catalog
            .definition("example.specialist/items")
            .expect("declared")
            .aggregation,
        Aggregation::Sum
    );

    // A quantile is declared but never averaged across sources.
    let mut catalog = MetricCatalog::with_general();
    catalog
        .define(MetricDefinition::new(
            "example.specialist/latency",
            "ms",
            Temporality::Gauge,
            Aggregation::Quantile,
        ))
        .expect("declared");
    let failure = catalog
        .aggregate(
            "example.specialist/latency",
            &[licoup_extension_contracts::usage::MetricValue::reported(
                "120",
                "ms",
                Temporality::Gauge,
            )],
        )
        .expect_err("a sampled quantile is not an average");
    assert_eq!(failure.code, "analytics_quantile_not_averaged");

    // The specialist metric aggregates exactly when it is declared summable.
    catalog
        .define(MetricDefinition::new(
            "example.specialist/items",
            "items",
            Temporality::Delta,
            Aggregation::Sum,
        ))
        .expect("declared");
    let aggregate = catalog
        .aggregate(
            "example.specialist/items",
            &[
                licoup_extension_contracts::usage::MetricValue::reported(
                    "1.50",
                    "items",
                    Temporality::Delta,
                ),
                licoup_extension_contracts::usage::MetricValue::unknown(
                    "items",
                    Temporality::Delta,
                ),
            ],
        )
        .expect("aggregated");
    assert_eq!(
        aggregate,
        Aggregate::Value {
            total: "1.50".to_owned(),
            unit: "items".to_owned(),
            unknown: 1,
        }
    );

    // A prepared value that carries a series the panel never declared is
    // refused by the registry.
    let panel = usage_panel();
    let mut registry = licoup_analytics::panels::PanelRegistry::new(1);
    registry.mount(
        std::slice::from_ref(&panel),
        &[ExtensionProfile::UsageMetric],
    );
    let mut value = licoup_analytics::panels::PreparedPanelValue::from_readings(
        &panel,
        1,
        &std::collections::BTreeMap::new(),
    );
    value
        .points
        .push(licoup_analytics::panels::PreparedPoint::known(
            "example.specialist/undeclared",
            "Other",
            "items",
            "1",
            Quality::Reported,
        ));
    assert_eq!(
        registry
            .install_prepared(value)
            .expect_err("undeclared series")
            .code,
        "analytics_panel_undeclared_series"
    );
}
