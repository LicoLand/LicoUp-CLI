//! A31 · real removal and data preservation — component-integration evidence.
//!
//! A31's full procedure is installed-product level: directories, processes,
//! handles, subscriptions and disk bytes are measured on a real installation.
//! This file is the component-level part the V7-U6 Task owns, and it is written
//! so the boundary is visible:
//!
//! - **The package's surfaces are really released.** Panels are withdrawn from
//!   the registry, prepared values are dropped, scrapers stop, the observation
//!   index and the source bindings are gone. Nothing is hidden behind a menu.
//! - **The facts are not.** They belong to the core ledger, which this package
//!   borrows; a snapshot before the uninstall compares equal after it, through
//!   the package's removal and a reinstall.
//! - **Uninstall drains instead of cutting work off.** New admission and new
//!   reads are refused immediately; in-flight reads must finish before the
//!   index is dropped.
//! - **An installation without the package has nothing to scrape.** Activation
//!   is refused with the contract's unavailable-capability vocabulary, so no
//!   index, no registry and no scraper exist to run in the background.
//!
//! Everything is synthetic; no real directory, process or account is touched.

use crate::support::{MemoryFacts, binding, observation, usage_panel};
use licoup_analytics::package::{AnalyticsConfig, AnalyticsPackage, PackageState};
use licoup_analytics::panels::PreparedPanelValue;
use licoup_analytics::policy::{AuthorityPolicy, SourceGrant};
use licoup_extension_contracts::deployment::PackageFacts;
use licoup_extension_contracts::profile::ExtensionProfile;
use licoup_extension_contracts::usage::{Quality, Temporality};
use licoup_usage_source_sdk::collection::QueryRequest;

fn active<'a>(
    facts: &'a mut MemoryFacts,
    authority: AuthorityPolicy,
) -> AnalyticsPackage<'a, MemoryFacts> {
    AnalyticsPackage::activate(
        facts,
        PackageFacts::local_import(true),
        AnalyticsConfig::new(
            authority,
            licoup_analytics::correlation::CorrelationPolicy::none(),
            1,
        ),
    )
    .expect("the analytics capability is served")
}

fn observation_for(
    scope: &str,
    id: &str,
    value: &str,
) -> licoup_usage_source_sdk::binding::BoundObservation {
    observation(
        "source:agent#1",
        scope,
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
}

#[test]
fn uninstall_releases_every_surface_and_preserves_the_facts() {
    let mut facts = MemoryFacts::new();
    let mut package = active(
        &mut facts,
        AuthorityPolicy::host_configured([SourceGrant::new("source:agent#1", true)]),
    );
    package
        .configure_source(binding("source:agent#1", "scope-1"), true)
        .expect("configured");
    package.start_scraper("source:agent#1").expect("scraper");
    let report = package
        .panels_mut()
        .mount(&[usage_panel()], &[ExtensionProfile::UsageMetric]);
    assert_eq!(report.mounted.len(), 1);
    package
        .panels_mut()
        .install_prepared(PreparedPanelValue::from_readings(
            &usage_panel(),
            1,
            &std::collections::BTreeMap::new(),
        ))
        .expect("prepared");
    package
        .admit(observation_for("scope-1", "obs-1", "120"))
        .expect("admitted");
    package
        .admit(observation_for("scope-1", "obs-2", "80"))
        .expect("admitted");
    assert_eq!(package.facts().len(), 2);
    let before = package.facts().snapshot();
    assert_eq!(package.index().len(), 2);
    assert_eq!(package.scrapers(), 1);

    // Stop new admission and release what can be released now.
    let drain = package.begin_uninstall().expect("draining");
    assert_eq!(drain.state, PackageState::Draining);
    assert_eq!(
        drain.withdrawn_contributions,
        vec!["org.licoland.feature.analytics/usage-panel".to_owned()]
    );
    assert_eq!(drain.dropped_prepared, 1);
    assert_eq!(drain.released_scrapers, vec!["source:agent#1".to_owned()]);
    assert!(drain.facts_preserved);
    assert!(package.panels().mounted_ids().is_empty());
    assert_eq!(package.scrapers(), 0);

    // New work is refused while the package is draining.
    let refusal = package
        .admit(observation_for("scope-1", "obs-3", "1"))
        .expect_err("new admission is refused");
    assert_eq!(refusal.code, "analytics_package_inactive");
    assert_eq!(refusal.presentation_args.get("state"), Some("draining"));
    assert_eq!(
        package
            .pull(
                "source:agent#1",
                &QueryRequest::new("scope-1", None, Some(10)).expect("request")
            )
            .expect_err("new reads are refused")
            .code,
        "analytics_package_inactive"
    );
    assert_eq!(package.facts().len(), 2, "the refusal changed no fact");

    // Finish the removal: the index and the bindings are dropped.
    let done = package.complete_uninstall().expect("complete");
    assert_eq!(done.state, PackageState::Removed);
    assert_eq!(done.dropped_observations, 2);
    assert_eq!(done.released_sources, vec!["source:agent#1".to_owned()]);
    assert!(package.index().is_empty());
    assert!(done.facts_preserved);

    // The package is gone; the ledger is exactly where it was.
    drop(package);
    assert_eq!(facts.len(), 2);
    assert_eq!(facts.snapshot(), before);
    assert_eq!(
        facts
            .settled_total("scope-1", "licoup.tokens.input")
            .as_deref(),
        Some("200")
    );

    // Reinstalling reads the same facts and duplicates nothing.
    let mut reinstalled = active(
        &mut facts,
        AuthorityPolicy::host_configured([SourceGrant::new("source:agent#1", true)]),
    );
    reinstalled
        .configure_source(binding("source:agent#1", "scope-1"), true)
        .expect("configured");
    assert_eq!(reinstalled.facts().len(), 2);
    assert_eq!(reinstalled.facts().snapshot(), before);
    assert_eq!(
        reinstalled
            .panels_mut()
            .mount(&[usage_panel()], &[ExtensionProfile::UsageMetric])
            .mounted
            .len(),
        1,
        "a reinstall mounts the panel again"
    );
    reinstalled
        .prepare_panel("org.licoland.feature.analytics/usage-panel", "scope-1")
        .expect("prepared");
    let prepared = reinstalled
        .panels()
        .prepared("org.licoland.feature.analytics/usage-panel")
        .expect("prepared value");
    let input = prepared
        .points
        .iter()
        .find(|point| point.metric == "licoup.tokens.input")
        .expect("declared series");
    assert_eq!(
        input.value, None,
        "an empty index prepares unknown, not a re-read of a source"
    );
    assert_eq!(input.quality, Quality::Unknown);
}

#[test]
fn uninstall_drains_in_flight_reads_before_dropping_the_index() {
    let mut facts = MemoryFacts::new();
    let mut package = active(
        &mut facts,
        AuthorityPolicy::host_configured([SourceGrant::new("source:agent#1", true)]),
    );
    package
        .configure_source(binding("source:agent#1", "scope-1"), true)
        .expect("configured");
    package
        .admit(observation_for("scope-1", "obs-1", "120"))
        .expect("admitted");

    let ticket = package.begin_read().expect("an in-flight read");
    assert_eq!(package.open_reads(), 1);
    package.begin_uninstall().expect("draining");
    assert_eq!(
        package
            .complete_uninstall()
            .expect_err("still draining")
            .code,
        "analytics_drain_pending"
    );
    assert_eq!(package.index().len(), 1, "the index waits for the drain");
    assert_eq!(package.facts().len(), 1);

    package.finish_read(ticket);
    let done = package.complete_uninstall().expect("complete");
    assert_eq!(done.state, PackageState::Removed);
    assert_eq!(done.dropped_observations, 1);
}

#[test]
fn an_installation_without_the_package_has_nothing_to_scrape() {
    // Not installed at all: the capability is a catalogue fact, and the
    // refusal points at installing or enabling the package.
    let absent = PackageFacts {
        available: false,
        installed: false,
        enabled: false,
        active: false,
    };
    assert_eq!(
        AnalyticsPackage::<MemoryFacts>::capability_state(absent).describe(),
        "not-installed"
    );
    let mut facts = MemoryFacts::new();
    let refusal = AnalyticsPackage::activate(
        &mut facts,
        absent,
        AnalyticsConfig::new(
            AuthorityPolicy::new(),
            licoup_analytics::correlation::CorrelationPolicy::none(),
            1,
        ),
    )
    .expect_err("nothing is constructed without the package");
    assert_eq!(refusal.code, "capability_unavailable");
    assert_eq!(
        refusal.recovery,
        licoup_extension_contracts::RecoveryAction::InstallOrRetryRuntime
    );
    assert_eq!(
        AnalyticsPackage::<MemoryFacts>::capability_state(PackageFacts::local_import(false))
            .describe(),
        "installed-not-enabled"
    );

    // Installed and enabled, but no source configured: there is no scraper, and
    // a panel redraw reads no fact.
    let mut package = active(&mut facts, AuthorityPolicy::new());
    assert_eq!(package.scrapers(), 0);
    assert_eq!(package.collection_attempts(), 0);
    assert_eq!(
        package
            .start_scraper("source:agent#1")
            .expect_err("an unconfigured source cannot be collected")
            .code,
        "analytics_source_not_admitted"
    );
    package
        .panels_mut()
        .mount(&[usage_panel()], &[ExtensionProfile::UsageMetric]);
    let reads_before = package.facts().read_calls();
    for _ in 0..3 {
        package
            .prepare_panel("org.licoland.feature.analytics/usage-panel", "scope-1")
            .expect("prepared");
    }
    assert_eq!(package.facts().read_calls(), reads_before);
    assert_eq!(package.collection_attempts(), 0);
}

#[test]
fn an_uninstall_reports_only_the_surface_it_actually_held() {
    // Nothing mounted, nothing prepared, no source configured: the package holds
    // no surface, so its removal reports none. "Removed nothing" is a fact about
    // this installation, not a completed removal of something else.
    let mut facts = MemoryFacts::new();
    let before = facts.snapshot();
    let mut package = active(&mut facts, AuthorityPolicy::new());
    let drain = package.begin_uninstall().expect("draining");
    assert!(drain.withdrawn_contributions.is_empty());
    assert_eq!(drain.dropped_prepared, 0);
    assert!(drain.released_sources.is_empty());
    assert!(drain.released_scrapers.is_empty());
    assert_eq!(drain.dropped_observations, 0);

    let done = package.complete_uninstall().expect("complete");
    assert_eq!(done.state, PackageState::Removed);
    assert!(done.withdrawn_contributions.is_empty());
    assert!(done.released_sources.is_empty());
    assert_eq!(done.dropped_observations, 0);
    assert!(done.facts_preserved);
    drop(package);
    assert_eq!(facts.snapshot(), before);
}

#[test]
fn uninstalling_the_panel_package_leaves_other_scopes_budgets_and_obligations_alone() {
    let mut facts = MemoryFacts::new();
    facts
        .pending
        .push(licoup_analytics::facts::PendingObligation {
            scope_ref: "scope-other".to_owned(),
            metric: "licoup.tokens.input".to_owned(),
            reason: "awaiting settlement".to_owned(),
        });
    let mut package = active(
        &mut facts,
        AuthorityPolicy::host_configured([SourceGrant::new("source:agent#1", true)]),
    );
    package
        .configure_source(binding("source:agent#1", "scope-1"), true)
        .expect("configured");
    package
        .admit(observation_for("scope-1", "obs-1", "120"))
        .expect("admitted");
    let other_scope_before = package.facts().snapshot();
    let pending_before = package.facts().pending.clone();

    package.begin_uninstall().expect("draining");
    package.complete_uninstall().expect("complete");
    drop(package);

    assert_eq!(facts.snapshot(), other_scope_before);
    assert_eq!(
        facts.pending, pending_before,
        "obligations are not the panel's"
    );
    assert_eq!(facts.retracts, 0, "uninstall withdraws no fact");
    assert_eq!(
        facts
            .settled_total("scope-1", "licoup.tokens.input")
            .as_deref(),
        Some("120"),
        "the run fact survives the panel"
    );
}
