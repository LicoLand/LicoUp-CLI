//! Admitted Assistant cognition. ScriptedAgent is a hermetic test substitute.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use licoup_conversation::continuity::{
    ContinuityContextManifest, ContinuityFailure, ContinuityInterpretationProposal,
};

use super::agent::{ScriptedAgent, SemanticScript};
use super::interpret::{apply_scripted_interpretation, unavailable_abstain};
use super::knowledge::UnavailableKnowledgeService;
use super::types::AssemblySnapshot;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CognitionIntent {
    UserPosted,
    WakeReevaluation,
    ChildWork,
}

#[derive(Clone, Debug)]
pub struct WakeReviewContext {
    pub goal_id: String,
    pub goal_revision: i64,
    pub brief: String,
    pub review_policy: String,
    pub parent_conversation_id: String,
    pub continuity_kind: &'static str,
}

#[derive(Clone, Debug)]
pub struct CognitionRequest {
    pub conversation_id: String,
    pub recipient_membership_id: String,
    pub event_id: String,
    pub intent: CognitionIntent,
    pub orientation: ContinuityContextManifest,
    pub assembly: AssemblySnapshot,
    pub review: Option<WakeReviewContext>,
}

#[derive(Clone, Debug)]
pub struct CognitionReply {
    pub proposal: ContinuityInterpretationProposal,
    pub unavailable: bool,
    pub abstained: bool,
}

pub trait CognitionInvoker: Send + Sync {
    fn invoke(&self, request: &CognitionRequest) -> Result<CognitionReply, ContinuityFailure>;
    fn invocation_count(&self) -> u64;
}

/// Production invoker: admits the designated Assistant, then calls the bound
/// runtime transport. No keyword, length, or count classifier.
pub struct AdmittedAssistantInvoker {
    invocations: AtomicU64,
    transport: Mutex<Option<Arc<dyn CognitionInvoker>>>,
}

impl AdmittedAssistantInvoker {
    pub fn new() -> Self {
        Self {
            invocations: AtomicU64::new(0),
            transport: Mutex::new(None),
        }
    }

    pub fn bind(&self, transport: Arc<dyn CognitionInvoker>) {
        *lock(&self.transport) = Some(transport);
    }

    pub fn unbind(&self) {
        *lock(&self.transport) = None;
    }

    pub fn is_bound(&self) -> bool {
        lock(&self.transport).is_some()
    }
}

impl Default for AdmittedAssistantInvoker {
    fn default() -> Self {
        Self::new()
    }
}

impl CognitionInvoker for AdmittedAssistantInvoker {
    fn invoke(&self, request: &CognitionRequest) -> Result<CognitionReply, ContinuityFailure> {
        self.invocations.fetch_add(1, Ordering::SeqCst);
        if request.recipient_membership_id.trim().is_empty() {
            return Ok(unavailable_reply(&request.assembly));
        }
        if let Some(transport) = lock(&self.transport).clone() {
            return transport.invoke(request);
        }
        Ok(unavailable_reply(&request.assembly))
    }

    fn invocation_count(&self) -> u64 {
        self.invocations.load(Ordering::SeqCst)
    }
}

/// Hermetic test substitute. Exact Event identity only.
pub struct ScriptedCognitionInvoker {
    invocations: AtomicU64,
    agent: Mutex<ScriptedAgent>,
    knowledge: UnavailableKnowledgeService,
}

impl ScriptedCognitionInvoker {
    pub fn new() -> Self {
        Self {
            invocations: AtomicU64::new(0),
            agent: Mutex::new(ScriptedAgent::new()),
            knowledge: UnavailableKnowledgeService::default(),
        }
    }

    pub fn insert(&self, script: SemanticScript) {
        lock(&self.agent).insert(script);
    }
}

impl Default for ScriptedCognitionInvoker {
    fn default() -> Self {
        Self::new()
    }
}

impl CognitionInvoker for ScriptedCognitionInvoker {
    fn invoke(&self, request: &CognitionRequest) -> Result<CognitionReply, ContinuityFailure> {
        self.invocations.fetch_add(1, Ordering::SeqCst);
        let agent = lock(&self.agent);
        let proposal = apply_scripted_interpretation(&agent, &request.assembly, &self.knowledge)?;
        let abstained = !proposal_has_business_effect(&proposal);
        Ok(CognitionReply {
            proposal,
            unavailable: false,
            abstained,
        })
    }

    fn invocation_count(&self) -> u64 {
        self.invocations.load(Ordering::SeqCst)
    }
}

pub fn proposal_creates_new_advancement(proposal: &ContinuityInterpretationProposal) -> bool {
    proposal.task_child_admission.is_some()
        || proposal
            .commitment_proposals
            .iter()
            .any(|commitment| commitment.create_goal)
}

pub fn proposal_has_business_effect(proposal: &ContinuityInterpretationProposal) -> bool {
    proposal.task_child_admission.is_some()
        || proposal
            .commitment_proposals
            .iter()
            .any(|commitment| commitment.create_goal)
        || !proposal.agreement_proposals.is_empty()
        || !proposal.matter_associations.is_empty()
        || matches!(
            proposal.speech_act,
            licoup_conversation::continuity::ContinuitySpeechAct::Pause
                | licoup_conversation::continuity::ContinuitySpeechAct::Cancellation
        )
}

fn unavailable_reply(assembly: &AssemblySnapshot) -> CognitionReply {
    CognitionReply {
        proposal: unavailable_abstain(assembly),
        unavailable: true,
        abstained: true,
    }
}

fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}
