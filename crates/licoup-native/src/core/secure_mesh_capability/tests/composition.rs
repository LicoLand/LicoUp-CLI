use anyhow::Result;

use super::super::{
    CapabilityCatalog, CapabilityEvaluation, CapabilityEvaluationReport, CapabilityEvidenceKind,
    CapabilityFact, SecurityCapability, capability_catalog, mandatory_protocol_facts,
};

#[test]
fn facade_preserves_the_public_catalog_evaluation_fact_and_report_flow() {
    let _: fn() -> Result<&'static CapabilityCatalog> = capability_catalog;
    let _: fn(CapabilityEvidenceKind) -> Result<Vec<CapabilityFact>> = mandatory_protocol_facts;
    let _: fn(&CapabilityCatalog, &[CapabilityFact]) -> Result<CapabilityEvaluation> =
        CapabilityCatalog::evaluate;
    let _: fn(&CapabilityEvaluation) -> CapabilityEvaluationReport = CapabilityEvaluation::report;
    let _: fn(&str) -> Result<SecurityCapability> = SecurityCapability::from_id;
}
