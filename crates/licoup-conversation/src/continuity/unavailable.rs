use super::generated::{
    ContinuityCommitBasis, ContinuityContextCompositionRequest, ContinuityContextManifest,
    ContinuityDecisionLayer, ContinuityEffectClass, ContinuityFailure, ContinuityFailureCode,
    ContinuityFailureStage, ContinuityGoalProgress, ContinuityInterpretationProposal,
    ContinuityQualificationRecord, ContinuityRecoveryClass, ContinuitySourceRef, ContinuityWake,
};
use super::ports::{
    ContextCompositionPort, ContinuityCommitPort, ContinuityCommitReceipt, ContinuityReadPort,
    DiscoveredKnowledgePort, FollowUpPort, GoalEvaluationPort, InterpretationPort,
    QualificationPort,
};

pub fn unavailable_failure(stage: ContinuityFailureStage) -> ContinuityFailure {
    ContinuityFailure {
        code: ContinuityFailureCode::UnsupportedCapability,
        stage,
        recovery: ContinuityRecoveryClass::ReviewOrWait,
        effect_class: ContinuityEffectClass::None,
        decision_layer: ContinuityDecisionLayer::Effects,
        retryable: false,
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct UnavailableContinuityRead;

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct UnavailableInterpretation;

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct UnavailableContinuityCommit;

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct UnavailableContextComposition;

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct UnavailableGoalEvaluation;

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct UnavailableFollowUp;

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct UnavailableQualification;

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct UnavailableDiscoveredKnowledge;

impl ContinuityReadPort for UnavailableContinuityRead {
    fn commit_basis(&self, _: &str) -> Result<ContinuityCommitBasis, ContinuityFailure> {
        Err(unavailable_failure(
            ContinuityFailureStage::ContinuityAdmission,
        ))
    }

    fn list_matters(
        &self,
        _: &str,
        _: Option<&str>,
        _: usize,
    ) -> Result<Vec<super::ContinuityMatter>, ContinuityFailure> {
        Err(unavailable_failure(
            ContinuityFailureStage::ContinuityAdmission,
        ))
    }

    fn relation_for_goal(
        &self,
        _: &str,
    ) -> Result<super::ContinuityTaskConversationRelation, ContinuityFailure> {
        Err(unavailable_failure(
            ContinuityFailureStage::ContinuityAdmission,
        ))
    }

    fn list_child_relations(
        &self,
        _: &str,
        _: Option<&str>,
        _: usize,
    ) -> Result<Vec<super::ContinuityTaskConversationRelation>, ContinuityFailure> {
        Err(unavailable_failure(
            ContinuityFailureStage::ContinuityAdmission,
        ))
    }

    fn list_parent_grants(
        &self,
        _: &str,
        _: &str,
        _: Option<&str>,
        _: usize,
    ) -> Result<Vec<super::ContinuityParentContextGrant>, ContinuityFailure> {
        Err(unavailable_failure(
            ContinuityFailureStage::ContinuityAdmission,
        ))
    }
}

impl InterpretationPort for UnavailableInterpretation {
    fn interpret(
        &self,
        _: &ContinuityContextManifest,
    ) -> Result<ContinuityInterpretationProposal, ContinuityFailure> {
        Err(unavailable_failure(
            ContinuityFailureStage::ContinuityAdmission,
        ))
    }
}

impl ContinuityCommitPort for UnavailableContinuityCommit {
    fn commit(
        &self,
        _: &ContinuityInterpretationProposal,
    ) -> Result<ContinuityCommitReceipt, ContinuityFailure> {
        Err(unavailable_failure(
            ContinuityFailureStage::ContinuityCommit,
        ))
    }
}

impl ContextCompositionPort for UnavailableContextComposition {
    fn compose_authorized(
        &self,
        _: &ContinuityContextCompositionRequest,
    ) -> Result<ContinuityContextManifest, ContinuityFailure> {
        Err(unavailable_failure(
            ContinuityFailureStage::ContinuityAdmission,
        ))
    }
}

impl GoalEvaluationPort for UnavailableGoalEvaluation {
    fn evaluate(
        &self,
        _: &ContinuityGoalProgress,
    ) -> Result<ContinuityGoalProgress, ContinuityFailure> {
        Err(unavailable_failure(
            ContinuityFailureStage::ContinuityAdmission,
        ))
    }
}

impl FollowUpPort for UnavailableFollowUp {
    fn enqueue_wake(
        &self,
        _: &ContinuityWake,
    ) -> Result<ContinuityCommitReceipt, ContinuityFailure> {
        Err(unavailable_failure(
            ContinuityFailureStage::ContinuityEffects,
        ))
    }
}

impl QualificationPort for UnavailableQualification {
    fn lookup(
        &self,
        _: &ContinuityQualificationRecord,
    ) -> Result<ContinuityQualificationRecord, ContinuityFailure> {
        Err(unavailable_failure(
            ContinuityFailureStage::ContinuityQualification,
        ))
    }
}

impl DiscoveredKnowledgePort for UnavailableDiscoveredKnowledge {
    fn lookup(&self, _: &str) -> Result<ContinuitySourceRef, ContinuityFailure> {
        Err(unavailable_failure(
            ContinuityFailureStage::ContinuityAdmission,
        ))
    }
}
