//! Host-private persisted adoption policy. This is not a user chat mode.

use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum AdoptionStage {
    #[default]
    Offline,
    AdmittedShadow,
    QualifiedLowRisk,
    Expanded,
}

impl AdoptionStage {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Offline => "offline",
            Self::AdmittedShadow => "admitted_shadow",
            Self::QualifiedLowRisk => "qualified_low_risk",
            Self::Expanded => "expanded",
        }
    }

    pub fn parse(value: &str) -> Self {
        match value {
            "admitted_shadow" => Self::AdmittedShadow,
            "qualified_low_risk" => Self::QualifiedLowRisk,
            "expanded" => Self::Expanded,
            _ => Self::Offline,
        }
    }

    pub fn allows_automatic_interpretation(self) -> bool {
        matches!(self, Self::QualifiedLowRisk | Self::Expanded)
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AdoptionPolicy {
    pub enabled: bool,
    pub stage: AdoptionStage,
}

impl Default for AdoptionPolicy {
    fn default() -> Self {
        Self {
            enabled: true,
            stage: AdoptionStage::Offline,
        }
    }
}

impl AdoptionPolicy {
    pub fn automatic_interpretation_allowed(&self) -> bool {
        self.enabled && self.stage.allows_automatic_interpretation()
    }

    pub fn from_stored(enabled: bool, stage: &str) -> Self {
        Self {
            enabled,
            stage: AdoptionStage::parse(stage),
        }
    }
}

/// Expanded means broader responsibility coverage, not raw stored-row count.
/// Two identities of one responsibility do not expand coverage.
pub const EXPANDED_DISTINCT_RESPONSIBILITY_MINIMUM: u64 = 2;

/// Derive the host adoption stage from distinct live-authorized responsibilities.
///
/// Host policy basis: `qualified_low_risk` is a single qualified responsibility;
/// `expanded` requires at least [`EXPANDED_DISTINCT_RESPONSIBILITY_MINIMUM`]
/// distinct qualified responsibilities. Only admission-class
/// (live-authorized, still revalidated) rows count toward shadow. Synthetic
/// imports never count as admitted.
pub fn stage_from_coverage(
    admitted_responsibilities: u64,
    qualified_responsibilities: u64,
) -> AdoptionStage {
    if qualified_responsibilities >= EXPANDED_DISTINCT_RESPONSIBILITY_MINIMUM {
        AdoptionStage::Expanded
    } else if qualified_responsibilities == 1 {
        AdoptionStage::QualifiedLowRisk
    } else if admitted_responsibilities > 0 {
        AdoptionStage::AdmittedShadow
    } else {
        AdoptionStage::Offline
    }
}
