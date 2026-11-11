use super::super::{CapabilityCatalog, CapabilityEvidenceKind, CapabilityFact};

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
    Vec::new()
}
