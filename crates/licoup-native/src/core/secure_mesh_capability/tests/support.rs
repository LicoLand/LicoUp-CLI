use super::super::{
    CapabilityCatalog, CapabilityEvidenceKind, CapabilityFact, mandatory_protocol_facts,
};

pub(super) fn all_supported_facts(catalog: &CapabilityCatalog) -> Vec<CapabilityFact> {
    catalog
        .definitions()
        .filter(|definition| !definition.derived)
        .map(|definition| {
            CapabilityFact::supported(definition.capability, CapabilityEvidenceKind::TestFixture)
        })
        .collect()
}

pub(super) fn baseline_facts() -> Vec<CapabilityFact> {
    mandatory_protocol_facts(CapabilityEvidenceKind::TestFixture)
        .expect("the embedded capability catalog must be valid")
}
