mod catalog;
mod custody;
mod evaluation;
mod facts;
mod report;
mod taxonomy;

pub use catalog::{CapabilityCatalog, CapabilityDefinition, capability_catalog};
pub(crate) use custody::custody_selection_from_enabled;
pub use custody::{CustodyRestartSemantics, CustodySelection, SecretCustodyStrategy};
pub use evaluation::CapabilityEvaluation;
pub use facts::{
    CapabilityEvidenceKind, CapabilityFact, CapabilityFactState, mandatory_protocol_facts,
};
pub use report::{CAPABILITY_REPORT_SCHEMA_VERSION, CapabilityEvaluationReport};
pub use taxonomy::{CapabilityScope, SecurityCapability};

#[cfg(test)]
mod tests;
