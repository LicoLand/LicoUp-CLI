use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

use super::custody::CustodySelection;
use super::evaluation::CapabilityEvaluation;
use super::taxonomy::SecurityCapability;

pub const CAPABILITY_REPORT_SCHEMA_VERSION: u32 = 1;

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct CapabilityEvaluationReport {
    pub schema_version: u32,
    pub catalog_digest: String,
    pub enabled: BTreeSet<SecurityCapability>,
    pub available: BTreeSet<SecurityCapability>,
    pub unavailable: BTreeSet<SecurityCapability>,
    pub unverified: BTreeSet<SecurityCapability>,
    pub reasons: BTreeMap<String, String>,
    pub custody: Option<CustodySelection>,
}

impl CapabilityEvaluation {
    pub fn report(&self) -> CapabilityEvaluationReport {
        CapabilityEvaluationReport {
            schema_version: CAPABILITY_REPORT_SCHEMA_VERSION,
            catalog_digest: self.catalog_digest.clone(),
            enabled: self.enabled.clone(),
            available: self.available.clone(),
            unavailable: self.unavailable.clone(),
            unverified: self.unverified.clone(),
            reasons: self
                .reasons
                .iter()
                .map(|(capability, reason)| (capability.id().to_string(), reason.clone()))
                .collect(),
            custody: self.custody.clone(),
        }
    }
}
