//! Optional analytics and usage sources — `org.licoland.feature.analytics`.
//!
//! This is M29: the package that owns statistics sources, the query index, the
//! dashboard definitions and specialist metrics, and it *consumes* the core's
//! retained metering facts rather than keeping a second economic ledger. Three
//! separations are structural here, and each is enforced rather than described:
//!
//! - **The trusted account is not this package.** [`facts::CoreUsageFacts`] is
//!   the narrow port over the core slice (M33): the minimal metering facts,
//!   their pending obligations and their settlement. This package borrows it,
//!   records eligible facts through it and never owns it, so uninstalling the
//!   package cannot delete a budget or a run fact, and a panel cannot become a
//!   ledger.
//! - **A producer's quality is not settlement authority.** [`policy`] decides
//!   which sources may settle from a host grant table. A source that reports
//!   `reported` gains nothing by saying so; an unprivileged source is displayed
//!   with its provenance and never recorded as a settlement.
//! - **Display is a prepared declaration.** [`panels`] mounts declarative
//!   `metric-panel` contributions and hands out prepared values whose unknown
//!   readings are unknown, not zero. A redraw reads prepared values; it does not
//!   re-scan a source.
//!
//! The package is optional and lazy: nothing here starts a collector. A scraper
//! exists only for a source the host configured while the package is active, and
//! [`package::AnalyticsPackage::activate`] refuses to build anything at all when
//! the capability is not installed — the absence is reported through the
//! contract's capability vocabulary, not as a failure of the client.
//!
//! Deduplication, corrections, cumulative resets and the single settlement of a
//! call reported by several sources are [`index`] and [`correlation`]; the input
//! mapping they consume is the usage-source SDK's.

pub mod correlation;
pub mod facts;
pub mod index;
pub mod metrics;
pub mod package;
pub mod panels;
pub mod policy;

/// The package's own lifecycle, compiled from `resources/state-machines/`.
///
/// The JSON configuration is the transition authority: `analytics.package`
/// declares the states and events, and [`package::AnalyticsPackage`] moves only
/// along the edges it declares. Restating the table in Rust is what this module
/// exists to prevent.
#[allow(dead_code, clippy::collapsible_if, clippy::enum_variant_names)]
pub(crate) mod state_machine {
    include!(concat!(env!("OUT_DIR"), "/state_machines.rs"));
}

pub use correlation::{CorrelationPolicy, Reconciliation};
pub use facts::{
    CoreUsageFacts, FactOutcome, FactPage, FactReceipt, MeteringFact, PendingObligation,
    SettlementEligibility,
};
pub use index::{CounterStep, IngestOutcome, ObservationIndex};
pub use metrics::{Aggregate, MetricCatalog, MetricDefinition};
pub use package::{Admission, AnalyticsConfig, AnalyticsPackage, PackageState, UninstallReport};
pub use panels::{PanelRegistry, PreparedPanelValue, PreparedPoint};
pub use policy::{AuthorityPolicy, SourceGrant};

use licoup_extension_contracts::{ApplicationFailure, RecoveryAction};

/// The component every failure from this package names.
pub(crate) const COMPONENT: &str = "analytics";

/// The stage every refusal reports.
pub(crate) const STAGE: &str = "analytics";

/// A permanent refusal about this package's own data or policy.
pub(crate) fn refusal(code: &str) -> ApplicationFailure {
    ApplicationFailure::permanent(code, STAGE).with_component(COMPONENT)
}

/// A refusal whose recovery points at the real next step: install or enable the
/// package, or reconcile before retrying.
pub(crate) fn refusal_with(code: &str, recovery: RecoveryAction) -> ApplicationFailure {
    refusal(code).with_recovery(recovery)
}
