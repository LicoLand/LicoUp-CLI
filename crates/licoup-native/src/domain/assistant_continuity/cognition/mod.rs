//! Interpretation and discovered-knowledge ports. M1 replaces the bodies.

mod agent;
mod interpret;
mod knowledge;
mod persistent;
mod runtime;
mod types;

pub use agent::{ScriptedAgent, SemanticScript, SpanAxis};
pub use interpret::{AssemblySource, UnavailableInterpretationService};
pub(crate) use interpret::{proposal_from_assistant_turn_response, proposal_from_turn_output};
pub use knowledge::{KnowledgeDiscoveryDescriptor, UnavailableKnowledgeService};
pub(crate) use persistent::{
    AdmittedTurnRequest, CompleteAdmittedTurn, PersistentTurnCognition,
    apply_admitted_runtime_fields, collect_admitted_granted_facts, compose_admitted_turn_params,
    compose_continuity_guidance,
};
pub use runtime::{
    AdmittedAssistantInvoker, CognitionIntent, CognitionInvoker, CognitionReply, CognitionRequest,
    ScriptedCognitionInvoker, WakeReviewContext, proposal_creates_new_advancement,
    proposal_has_business_effect,
};
pub use types::{
    AssemblySnapshot, ContextRecord, InformationClass, continuity_failure, source_key,
};

pub fn interpretation_port() -> UnavailableInterpretationService {
    UnavailableInterpretationService::empty()
}
