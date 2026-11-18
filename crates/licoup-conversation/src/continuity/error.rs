use super::generated::{
    ContinuityDecisionLayer, ContinuityEffectClass, ContinuityFailure, ContinuityFailureCode,
    ContinuityFailureStage, ContinuityRecoveryClass,
};
use anyhow::Error as AnyhowError;

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

pub fn sql_failure(error: rusqlite::Error) -> ContinuityFailure {
    let _ = error;
    continuity_failure(
        ContinuityFailureCode::InvalidRequest,
        ContinuityFailureStage::ContinuityCommit,
    )
}

pub fn store_to_continuity(error: AnyhowError) -> ContinuityFailure {
    let text = error.to_string();
    if text.contains("conversation_revision_stale") || text.contains("stale") {
        return continuity_failure(
            ContinuityFailureCode::StaleRevision,
            ContinuityFailureStage::ContinuityAdmission,
        );
    }
    if text.contains("conversation_not_found") || text.contains("membership_not_found") {
        return continuity_failure(
            ContinuityFailureCode::ScopeDenied,
            ContinuityFailureStage::ContinuityAdmission,
        );
    }
    if text.contains("reconciliation_required") {
        return continuity_failure(
            ContinuityFailureCode::ReconciliationRequired,
            ContinuityFailureStage::ContinuityEffects,
        );
    }
    continuity_failure(
        ContinuityFailureCode::InvalidRequest,
        ContinuityFailureStage::ContinuityCommit,
    )
}
