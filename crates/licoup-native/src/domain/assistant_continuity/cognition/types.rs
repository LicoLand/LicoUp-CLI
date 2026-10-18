//! Shared cognition types. Information classes stay separate.

use licoup_conversation::continuity::{
    ContinuityAgreement, ContinuityContextCompositionRequest, ContinuityDecisionLayer,
    ContinuityEffectClass, ContinuityFailure, ContinuityFailureCode, ContinuityFailureStage,
    ContinuityRecoveryClass, ContinuitySourceRef,
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum InformationClass {
    ConversationFact,
    Agreement,
    KnowledgeReference,
    WorkingNote,
    Responsibility,
    CallbackFact,
}

#[derive(Clone, Debug)]
pub struct ContextRecord {
    pub conversation_id: String,
    pub matter_id: Option<String>,
    pub class: InformationClass,
    pub source: ContinuitySourceRef,
    pub agreement: Option<ContinuityAgreement>,
    pub membership_id: Option<String>,
    pub recency: i64,
    pub entities: Vec<String>,
    pub text_bytes: u64,
    pub explicit_refs: Vec<ContinuitySourceRef>,
    pub conversation_level: bool,
    pub is_current_input: bool,
    pub is_malicious_data: bool,
    pub is_summary: bool,
    pub is_worker_or_turn_exit: bool,
    pub is_mcp_return: bool,
}

#[derive(Clone, Debug)]
pub struct AssemblySnapshot {
    pub invocation_id: String,
    pub conversation_id: String,
    pub request: ContinuityContextCompositionRequest,
    pub records: Vec<ContextRecord>,
    pub input_refs: Vec<ContinuitySourceRef>,
    pub retrieved_reads: Vec<ContinuitySourceRef>,
    pub replay_key: String,
    pub observed_revision: i64,
    pub designation_epoch: i64,
}

pub fn continuity_failure(
    code: ContinuityFailureCode,
    stage: ContinuityFailureStage,
) -> ContinuityFailure {
    let recovery = match code {
        ContinuityFailureCode::StaleRevision | ContinuityFailureCode::DesignationChanged => {
            ContinuityRecoveryClass::RecomputeProposal
        }
        ContinuityFailureCode::ReconciliationRequired | ContinuityFailureCode::PrematureClosure => {
            ContinuityRecoveryClass::ReconcileEffects
        }
        ContinuityFailureCode::ApprovalRequired => ContinuityRecoveryClass::ObtainApproval,
        ContinuityFailureCode::UnsupportedCapability
        | ContinuityFailureCode::SourceUnavailable
        | ContinuityFailureCode::WriterBusy => ContinuityRecoveryClass::ReviewOrWait,
        _ => ContinuityRecoveryClass::CorrectRequest,
    };
    ContinuityFailure {
        code,
        stage,
        recovery,
        effect_class: ContinuityEffectClass::None,
        decision_layer: ContinuityDecisionLayer::Admission,
        retryable: false,
    }
}

pub fn source_key(source: &ContinuitySourceRef) -> String {
    format!(
        "{}:{}:{}:{}:{}:{}",
        owner_label(source),
        source.opaque_id,
        source.part_id.as_deref().unwrap_or(""),
        span_label(source),
        source.source_revision,
        source.digest
    )
}

fn owner_label(source: &ContinuitySourceRef) -> &'static str {
    use licoup_conversation::continuity::ContinuitySourceOwnerKind::*;
    match source.owner_kind {
        Event => "event",
        Part => "part",
        Span => "span",
        Agreement => "agreement",
        Goal => "goal",
        Artifact => "artifact",
        Knowledge => "knowledge",
    }
}

fn span_label(source: &ContinuitySourceRef) -> String {
    match &source.span {
        Some(span) => format!("{}-{}", span.start_byte, span.end_byte),
        None => "none".into(),
    }
}
