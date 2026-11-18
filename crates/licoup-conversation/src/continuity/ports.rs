use super::generated::{
    ContinuityCommitBasis, ContinuityContextCompositionRequest, ContinuityContextManifest,
    ContinuityEffectClass, ContinuityFailure, ContinuityGoalProgress,
    ContinuityInterpretationProposal, ContinuityParentContextGrant, ContinuityQualificationRecord,
    ContinuitySourceRef, ContinuityTaskConversationRelation,
};

/// Read-only continuity facts under an authorized scope. Implementations must
/// not default to an unbounded conversation dump.
pub trait ContinuityReadPort {
    fn commit_basis(
        &self,
        conversation_id: &str,
    ) -> Result<ContinuityCommitBasis, ContinuityFailure>;
    fn list_matters(
        &self,
        conversation_id: &str,
        after: Option<&str>,
        limit: usize,
    ) -> Result<Vec<super::ContinuityMatter>, ContinuityFailure>;
    fn relation_for_goal(
        &self,
        goal_id: &str,
    ) -> Result<ContinuityTaskConversationRelation, ContinuityFailure>;
    fn list_child_relations(
        &self,
        parent_conversation_id: &str,
        after: Option<&str>,
        limit: usize,
    ) -> Result<Vec<ContinuityTaskConversationRelation>, ContinuityFailure>;
    fn list_parent_grants(
        &self,
        recipient_conversation_id: &str,
        recipient_membership_id: &str,
        after: Option<&str>,
        limit: usize,
    ) -> Result<Vec<ContinuityParentContextGrant>, ContinuityFailure>;
}

/// Agent semantic output. A proposal is not an executable command.
pub trait InterpretationPort {
    fn interpret(
        &self,
        orientation: &ContinuityContextManifest,
    ) -> Result<ContinuityInterpretationProposal, ContinuityFailure>;
}

/// Deterministic Conversation admission and persist. Models never receive a
/// database connection.
pub trait ContinuityCommitPort {
    fn commit(
        &self,
        proposal: &ContinuityInterpretationProposal,
    ) -> Result<ContinuityCommitReceipt, ContinuityFailure>;
}

/// Authorized context selection, including continue/compact/fork/new/rehydrate.
/// Payload bodies stay private to the owner.
///
/// [`Self::compose_authorized`] is the only composition method. It is
/// recipient-scoped: the request names the current conversation, recipient
/// membership, authorized scopes, revocation generation, and paging. M1
/// implements it and reads grants through
/// [`ContinuityReadPort::list_parent_grants`]. There is no ambient two-argument
/// `compose`.
pub trait ContextCompositionPort {
    fn compose_authorized(
        &self,
        request: &ContinuityContextCompositionRequest,
    ) -> Result<ContinuityContextManifest, ContinuityFailure>;
}

/// Evidence versus acceptance policy. This port does not execute work.
pub trait GoalEvaluationPort {
    fn evaluate(
        &self,
        progress: &ContinuityGoalProgress,
    ) -> Result<ContinuityGoalProgress, ContinuityFailure>;
}

/// Reliable wake handoff to the existing Conversation host.
pub trait FollowUpPort {
    fn enqueue_wake(
        &self,
        wake: &super::ContinuityWake,
    ) -> Result<ContinuityCommitReceipt, ContinuityFailure>;
}

/// Responsibility qualification. Price, model, and Skill catalogs stay with
/// their existing owners.
pub trait QualificationPort {
    fn lookup(
        &self,
        record: &ContinuityQualificationRecord,
    ) -> Result<ContinuityQualificationRecord, ContinuityFailure>;
}

/// Discovered knowledge descriptor lookup. Unavailability is typed.
pub trait DiscoveredKnowledgePort {
    fn lookup(&self, capability: &str) -> Result<ContinuitySourceRef, ContinuityFailure>;
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ContinuityCommitReceipt {
    pub conversation_id: String,
    pub revision: i64,
    pub effect_class: ContinuityEffectClass,
}

pub const fn no_effect_receipt(conversation_id: String, revision: i64) -> ContinuityCommitReceipt {
    ContinuityCommitReceipt {
        conversation_id,
        revision,
        effect_class: ContinuityEffectClass::None,
    }
}
