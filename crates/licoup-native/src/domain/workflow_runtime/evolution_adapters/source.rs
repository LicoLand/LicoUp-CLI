//! Where a cost number came from, and what that number may authorize.
//!
//! Two facts that are easy to conflate are kept apart here:
//!
//! - **Provenance** — the ledger's accuracy marker for a recorded sample
//!   (`exact`, `estimated`, `unknown`) combined with where the number entered
//!   the system.
//! - **Authority** — what the number is allowed to do. Only a number the
//!   provider reported for the effect that actually ran can charge a budget
//!   reservation. An imported usage marker carries provenance and nothing else.
//!
//! The rule lives in the type surface, not in a comment:
//! [`CostProvenance::authorizes_budget_settlement`] is `false` for everything
//! except [`CostProvenance::ProviderReported`], and [`SettleableUsage`] — the
//! only value a settlement accepts — has private fields and a single
//! constructor that refuses every other provenance with a typed reason. A
//! caller holding only an imported marker cannot build the argument a
//! settlement needs, so "imported usage" cannot quietly become "approved
//! settlement" by passing a larger struct.
//!
//! Provenance is also not a context. Knowing a number was imported says nothing
//! about who authorized the work, which session it writes through, or what
//! scope it may touch; nothing here converts into an authorization, and
//! [`super::policy::AuthorityContextState`] can only be built from a real
//! authorization.
//!
//! Catalog facts come from the installed pricing catalog
//! ([`crate::domain::provider_model_pricing`]). When that catalog publishes no
//! route for a model, the honest answer is unknown — never a zero rate. The
//! catalog's unit differs per provider table and is not projected through this
//! API, so this adapter reports published rates instead of inventing a currency
//! total.

use serde::{Deserialize, Serialize};

use crate::domain::provider_model_pricing::{self, ModelTokenPrice};

use super::{AdapterError, AdapterName, AdapterResult, CostUsage, FactState};

const ADAPTER: AdapterName = AdapterName::Source;

/// A refusal this adapter makes before any owner is consulted.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum SourceErrorReason {
    /// An imported usage marker was offered as settlement evidence.
    NotSettleableImportedUsage,
    /// An estimate was offered as settlement evidence. Estimates belong to
    /// admission, where they protect concurrent work; they are not actual spend.
    NotSettleableEstimate,
    /// A number with no known origin was offered as settlement evidence.
    NotSettleableUnknown,
    /// The installed catalog was asked for a model it does not publish.
    CatalogUnavailable,
}

impl SourceErrorReason {
    pub const fn code(self) -> &'static str {
        match self {
            Self::NotSettleableImportedUsage => "source_not_settleable_imported_usage",
            Self::NotSettleableEstimate => "source_not_settleable_estimate",
            Self::NotSettleableUnknown => "source_not_settleable_unknown",
            Self::CatalogUnavailable => "source_catalog_unavailable",
        }
    }

    pub const fn recovery(self) -> &'static str {
        match self {
            Self::NotSettleableImportedUsage => "settle_from_the_effects_own_outcome",
            Self::NotSettleableEstimate => "wait_for_the_reported_outcome",
            Self::NotSettleableUnknown => "read_the_cost_from_its_owner",
            Self::CatalogUnavailable => "configure_or_refresh_the_pricing_catalog",
        }
    }

    pub const fn error(self) -> AdapterError {
        AdapterError::refused(ADAPTER, self.code(), self.recovery())
    }
}

/// A marker that a number came from an imported usage source.
///
/// It describes the import and nothing else: it names no reservation, no run
/// and no grant, and it authorizes no settlement.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ImportedUsageMarker {
    pub source_id: String,
    pub revision: String,
    /// How many records the import carried. A count, not a permission.
    pub records: u64,
}

impl ImportedUsageMarker {
    /// Always false, and that is the point: importing usage is evidence about
    /// history, not approval to charge a budget reservation.
    pub const fn authorizes_budget_settlement(&self) -> bool {
        false
    }
}

/// The accuracy marker the ledger itself recorded for a sample.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum RecordedAccuracy {
    /// Counters reported by the provider or runtime that ran the effect.
    Exact,
    /// Counters derived by estimation rather than reported.
    Estimated,
    /// No counters: the ledger recorded the row as unknown.
    Unknown,
}

impl RecordedAccuracy {
    /// Map the ledger's own accuracy string.
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "exact" => Some(Self::Exact),
            "estimated" => Some(Self::Estimated),
            "unknown" => Some(Self::Unknown),
            _ => None,
        }
    }
}

/// Where a number entered the system.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", tag = "origin")]
pub enum NumberOrigin {
    /// The authenticated outcome of the effect that ran.
    ProviderReport { model_id: Option<String> },
    /// The installed pricing catalog.
    Catalog { model_id: String },
    /// An imported usage source.
    ImportedUsage { marker: ImportedUsageMarker },
    /// Nothing recorded where it came from.
    Unattributed,
}

/// Why a number's origin is unknown.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ProvenanceUnknownReason {
    /// The origin was never recorded, or the recorded markers disagree.
    Unattributed,
    /// The installed catalog publishes no route for this model.
    CatalogHasNoRoute,
}

/// Where a cost number came from.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", tag = "provenance")]
pub enum CostProvenance {
    /// Reported by the provider or runtime that actually ran the effect. This
    /// is the only provenance that may charge a reservation.
    ProviderReported {
        model_id: Option<String>,
    },
    /// Derived from the installed catalog's published rates. Useful for
    /// admission estimates; it is not actual spend.
    EstimatedFromCatalog {
        model_id: String,
    },
    /// Imported from a usage source that is not this effect's outcome.
    ImportedUsage {
        marker: ImportedUsageMarker,
    },
    Unknown {
        reason: ProvenanceUnknownReason,
    },
}

impl CostProvenance {
    pub const fn state(&self) -> FactState {
        match self {
            Self::ProviderReported { .. } | Self::EstimatedFromCatalog { .. } => FactState::Present,
            Self::ImportedUsage { .. } | Self::Unknown { .. } => FactState::Unknown,
        }
    }

    /// Whether this provenance may settle a budget reservation.
    ///
    /// Only the effect's own provider-reported outcome may. An estimate is an
    /// admission input, an imported marker is history, and an unknown origin is
    /// nothing at all — settling on any of them would spend the reserved budget
    /// on a number that does not describe this effect.
    pub const fn authorizes_budget_settlement(&self) -> bool {
        matches!(self, Self::ProviderReported { .. })
    }

    /// Turn these numbers into settlement evidence, if the provenance allows it.
    ///
    /// Every other provenance is refused with the reason for the refusal, so a
    /// caller holding an imported marker gets a typed answer rather than a
    /// settlement.
    pub fn settlement_evidence(&self, usage: CostUsage) -> AdapterResult<SettleableUsage> {
        match self {
            Self::ProviderReported { model_id } => Ok(SettleableUsage {
                usage,
                model_id: model_id.clone(),
            }),
            Self::EstimatedFromCatalog { .. } => {
                Err(SourceErrorReason::NotSettleableEstimate.error())
            }
            Self::ImportedUsage { .. } => {
                Err(SourceErrorReason::NotSettleableImportedUsage.error())
            }
            Self::Unknown { .. } => Err(SourceErrorReason::NotSettleableUnknown.error()),
        }
    }
}

/// Evidence that a settlement may charge a reservation.
///
/// The fields are private and the only constructors take provider-reported
/// numbers. An imported marker cannot become this value, so "this number came
/// from somewhere" never turns into "this number may spend the reserved
/// budget".
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SettleableUsage {
    usage: CostUsage,
    model_id: Option<String>,
}

impl SettleableUsage {
    /// Evidence from the effect's own reported outcome.
    pub const fn from_provider_report(usage: CostUsage, model_id: Option<String>) -> Self {
        Self { usage, model_id }
    }

    pub const fn usage(&self) -> CostUsage {
        self.usage
    }

    pub fn model_id(&self) -> Option<&str> {
        self.model_id.as_deref()
    }

    /// The ledger's accuracy marker for provider-reported counters.
    pub const fn ledger_accuracy(&self) -> &'static str {
        "exact"
    }
}

/// The provenance adapter over the installed catalog and the ledger's markers.
#[derive(Default)]
pub struct SourceAdapter;

impl SourceAdapter {
    pub const fn new() -> Self {
        Self
    }

    /// Combine the ledger's recorded accuracy with where the number came from.
    ///
    /// The two are independent: an imported report can carry counters the
    /// importer calls exact and it is still imported, because the ledger's
    /// marker describes how the counters were obtained, not whether they may
    /// charge a reservation. When the two facts disagree — the ledger says
    /// `exact` while the number is catalog-derived — the origin is reported as
    /// unknown instead of being resolved in favour of settlement.
    pub fn classify(&self, accuracy: RecordedAccuracy, origin: &NumberOrigin) -> CostProvenance {
        if let NumberOrigin::ImportedUsage { marker } = origin {
            return CostProvenance::ImportedUsage {
                marker: marker.clone(),
            };
        }
        match accuracy {
            RecordedAccuracy::Unknown => CostProvenance::Unknown {
                reason: ProvenanceUnknownReason::Unattributed,
            },
            RecordedAccuracy::Estimated => match origin {
                NumberOrigin::Catalog { model_id }
                | NumberOrigin::ProviderReport {
                    model_id: Some(model_id),
                } => CostProvenance::EstimatedFromCatalog {
                    model_id: model_id.clone(),
                },
                _ => CostProvenance::Unknown {
                    reason: ProvenanceUnknownReason::Unattributed,
                },
            },
            RecordedAccuracy::Exact => match origin {
                NumberOrigin::ProviderReport { model_id } => CostProvenance::ProviderReported {
                    model_id: model_id.clone(),
                },
                _ => CostProvenance::Unknown {
                    reason: ProvenanceUnknownReason::Unattributed,
                },
            },
        }
    }

    /// Provenance for a model rate read from the installed catalog.
    pub fn catalog_estimate(&self, model_id: &str) -> CostProvenance {
        match self.catalog_rate(model_id) {
            Some(_) => CostProvenance::EstimatedFromCatalog {
                model_id: model_id.to_owned(),
            },
            None => CostProvenance::Unknown {
                reason: ProvenanceUnknownReason::CatalogHasNoRoute,
            },
        }
    }

    /// Provenance for an Agent route read from the installed catalog.
    pub fn agent_catalog_estimate(
        &self,
        agent_id: &str,
        model_id: &str,
        thinking: &str,
    ) -> CostProvenance {
        match provider_model_pricing::agent_model_price(agent_id, model_id, thinking) {
            Some(_) => CostProvenance::EstimatedFromCatalog {
                model_id: model_id.to_owned(),
            },
            None => CostProvenance::Unknown {
                reason: ProvenanceUnknownReason::CatalogHasNoRoute,
            },
        }
    }

    /// The published rates for one model, when the catalog has a route.
    ///
    /// The catalog's unit varies per provider table and is not projected
    /// through this API, so rates are reported as published and no currency
    /// total is derived here. `None` is the honest "the catalog publishes
    /// nothing for this model" answer and must not be replaced with zeros.
    pub fn catalog_rate(&self, model_id: &str) -> Option<ModelTokenPrice> {
        provider_model_pricing::model_price(model_id)
    }
}
