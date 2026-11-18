mod catalog;
mod custody;
mod evaluation;
mod facts;
mod report;
mod taxonomy;

pub use catalog::{CapabilityCatalog, CapabilityDefinition, capability_catalog};
pub use custody::{CustodyRestartSemantics, CustodySelection, SecretCustodyStrategy};
pub use evaluation::CapabilityEvaluation;
pub use facts::{CapabilityEvidenceKind, CapabilityFact, CapabilityFactState};
pub use report::{CAPABILITY_REPORT_SCHEMA_VERSION, CapabilityEvaluationReport};
pub use taxonomy::SecurityCapability;

#[cfg(test)]
mod tests;
