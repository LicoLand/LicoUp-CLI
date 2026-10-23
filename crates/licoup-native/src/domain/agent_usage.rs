//! Local-only agent token usage facade.
//!
//! The measurement engine lives in `agent-statistics`; this module keeps the
//! product readers and the host composition that answers the engine's catalog,
//! summary and readiness questions.

mod agent_usage_codex;
mod agent_usage_native;
pub mod host;

pub use agent_statistics::contract::supported_agents;
pub use agent_statistics::incremental::{UsageIncrementalAuthority, UsageWindowProjection};
pub use agent_statistics::journal::{
    EventOutcome, EventSettlement, FactOutcome, FactPage, FactReceipt, IngestReport, MeteringFact,
    MeteringFactPort, PendingObligation, SettlementEligibility, SourceAdmission, UsageJournal,
    UsageSourcePort,
};
pub use acativa::usage_ledger::{
    graph_admission_report, release_graph_command, reserve_graph_command, settle_graph_command,
};
pub use agent_statistics::source_host::{
    ReadingSettlement, UsageSourceHost, held_metering, install, source_host,
};
pub(crate) use agent_statistics::variant::{
    model_label as recorded_usage_model, prefer_recorded_model,
};
pub use agent_statistics::{report, scan};

#[cfg(test)]
mod tests;
