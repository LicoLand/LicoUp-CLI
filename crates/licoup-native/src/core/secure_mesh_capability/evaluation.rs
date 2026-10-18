use anyhow::{Result, anyhow, ensure};
use std::collections::{BTreeMap, BTreeSet};

use super::catalog::CapabilityCatalog;
use super::custody::{CustodySelection, custody_selection_from_enabled};
use super::facts::{CapabilityFact, CapabilityFactState, validate_reason_code};
use super::taxonomy::SecurityCapability;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CapabilityEvaluation {
    pub(super) catalog_digest: String,
    pub(super) enabled: BTreeSet<SecurityCapability>,
    pub(super) available: BTreeSet<SecurityCapability>,
    pub(super) unavailable: BTreeSet<SecurityCapability>,
    pub(super) unverified: BTreeSet<SecurityCapability>,
    pub(super) reasons: BTreeMap<SecurityCapability, String>,
    pub(super) missing_mandatory: BTreeSet<SecurityCapability>,
    pub(super) mandatory_foundation_complete: bool,
    pub(super) custody: Option<CustodySelection>,
    visited_node_count: usize,
    traversed_edge_count: usize,
}

impl CapabilityEvaluation {
    pub fn catalog_digest(&self) -> &str {
        &self.catalog_digest
    }

    pub fn enabled(&self) -> &BTreeSet<SecurityCapability> {
        &self.enabled
    }

    pub fn available(&self) -> &BTreeSet<SecurityCapability> {
        &self.available
    }

    pub fn unavailable(&self) -> &BTreeSet<SecurityCapability> {
        &self.unavailable
    }

    pub fn unverified(&self) -> &BTreeSet<SecurityCapability> {
        &self.unverified
    }

    pub fn reasons(&self) -> &BTreeMap<SecurityCapability, String> {
        &self.reasons
    }

    pub fn missing_mandatory(&self) -> &BTreeSet<SecurityCapability> {
        &self.missing_mandatory
    }

    pub fn mandatory_foundation_complete(&self) -> bool {
        self.mandatory_foundation_complete
    }

    pub fn custody(&self) -> Option<&CustodySelection> {
        self.custody.as_ref()
    }

    pub fn require_mandatory_foundation(&self) -> Result<()> {
        ensure!(
            self.mandatory_foundation_complete,
            "secure mesh mandatory capability foundation is incomplete"
        );
        Ok(())
    }

    #[cfg(test)]
    pub(super) fn evaluation_work(&self) -> (usize, usize) {
        (self.visited_node_count, self.traversed_edge_count)
    }
}

impl CapabilityCatalog {
    pub fn evaluate(&self, facts: &[CapabilityFact]) -> Result<CapabilityEvaluation> {
        ensure!(
            facts.len() <= SecurityCapability::COUNT,
            "secure mesh capability fact set exceeds its bounded size"
        );
        let mut facts_by_index: [Option<&CapabilityFact>; SecurityCapability::COUNT] =
            [None; SecurityCapability::COUNT];
        for fact in facts {
            let definition = self.definition(fact.capability).ok_or_else(|| {
                anyhow!("secure mesh capability fact is not present in the catalog")
            })?;
            ensure!(
                !definition.derived,
                "secure mesh derived capability cannot be supplied as a platform fact"
            );
            validate_reason_code(fact.reason_code.as_deref())?;
            ensure!(
                facts_by_index[fact.capability.index()].is_none(),
                "secure mesh capability facts contain a duplicate identifier"
            );
            facts_by_index[fact.capability.index()] = Some(fact);
        }

        let mut enabled_flags = [false; SecurityCapability::COUNT];
        let mut enabled = BTreeSet::new();
        let mut available = BTreeSet::new();
        let mut unavailable = BTreeSet::new();
        let mut unverified = BTreeSet::new();
        let mut reasons = BTreeMap::new();
        let mut visited_node_count = 0usize;
        let mut traversed_edge_count = 0usize;

        for capability in self.topological_order() {
            visited_node_count = visited_node_count.saturating_add(1);
            let definition = self
                .definition(*capability)
                .ok_or_else(|| anyhow!("secure mesh capability definition is unavailable"))?;
            let mut dependencies_enabled = true;
            for prerequisite in &definition.prerequisites {
                traversed_edge_count = traversed_edge_count.saturating_add(1);
                dependencies_enabled &= enabled_flags[prerequisite.index()];
            }

            let fact = facts_by_index[capability.index()];
            let supported = if definition.derived {
                dependencies_enabled
            } else {
                fact.map(|fact| fact.state == CapabilityFactState::Supported)
                    .unwrap_or(false)
            };
            if supported {
                available.insert(*capability);
            }
            if supported && dependencies_enabled {
                enabled_flags[capability.index()] = true;
                enabled.insert(*capability);
                continue;
            }

            match fact.map(|fact| fact.state) {
                Some(CapabilityFactState::Unsupported)
                | Some(CapabilityFactState::TemporarilyUnavailable) => {
                    unavailable.insert(*capability);
                }
                Some(CapabilityFactState::Supported) if !dependencies_enabled => {
                    reasons.insert(*capability, "capability_dependency_unmet".to_string());
                }
                Some(CapabilityFactState::Unverified) | None => {
                    unverified.insert(*capability);
                }
                Some(CapabilityFactState::Supported) => {}
            }
            reasons.entry(*capability).or_insert_with(|| {
                fact.and_then(|fact| fact.reason_code.clone())
                    .unwrap_or_else(|| match fact.map(|fact| fact.state) {
                        Some(CapabilityFactState::Unsupported) => {
                            "capability_not_supported".to_string()
                        }
                        Some(CapabilityFactState::TemporarilyUnavailable) => {
                            "capability_temporarily_unavailable".to_string()
                        }
                        _ if definition.derived && !dependencies_enabled => {
                            "capability_dependency_unmet".to_string()
                        }
                        _ => "capability_unverified".to_string(),
                    })
            });
        }

        let missing_mandatory = self
            .definitions()
            .filter(|definition| definition.mandatory)
            .map(|definition| definition.capability)
            .filter(|capability| !enabled.contains(capability))
            .collect::<BTreeSet<_>>();
        let mandatory_foundation_complete = missing_mandatory.is_empty();
        let custody = custody_selection_from_enabled(&enabled);

        Ok(CapabilityEvaluation {
            catalog_digest: self.digest().to_string(),
            enabled,
            available,
            unavailable,
            unverified,
            reasons,
            missing_mandatory,
            mandatory_foundation_complete,
            custody,
            visited_node_count,
            traversed_edge_count,
        })
    }
}
