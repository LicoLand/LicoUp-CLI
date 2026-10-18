use licoup_conversation::continuity::{
    ContinuityDecisionLayer, ContinuityEffectClass, ContinuityFailure, ContinuityFailureCode,
    ContinuityFailureStage, ContinuityRecoveryClass,
};

pub fn invalid_request(code: ContinuityFailureCode) -> ContinuityFailure {
    ContinuityFailure {
        code,
        stage: ContinuityFailureStage::ContinuityQualification,
        recovery: ContinuityRecoveryClass::CorrectRequest,
        effect_class: ContinuityEffectClass::None,
        decision_layer: ContinuityDecisionLayer::Admission,
        retryable: false,
    }
}

pub fn qualification_unknown() -> ContinuityFailure {
    ContinuityFailure {
        code: ContinuityFailureCode::QualificationUnknown,
        stage: ContinuityFailureStage::ContinuityQualification,
        recovery: ContinuityRecoveryClass::ReviewOrWait,
        effect_class: ContinuityEffectClass::None,
        decision_layer: ContinuityDecisionLayer::Admission,
        retryable: false,
    }
}

pub fn qualification_stale() -> ContinuityFailure {
    ContinuityFailure {
        code: ContinuityFailureCode::QualificationStale,
        stage: ContinuityFailureStage::ContinuityQualification,
        recovery: ContinuityRecoveryClass::ReviewOrWait,
        effect_class: ContinuityEffectClass::None,
        decision_layer: ContinuityDecisionLayer::Admission,
        retryable: false,
    }
}

pub fn scope_denied() -> ContinuityFailure {
    ContinuityFailure {
        code: ContinuityFailureCode::ScopeDenied,
        stage: ContinuityFailureStage::ContinuityQualification,
        recovery: ContinuityRecoveryClass::ObtainApproval,
        effect_class: ContinuityEffectClass::None,
        decision_layer: ContinuityDecisionLayer::Admission,
        retryable: false,
    }
}
