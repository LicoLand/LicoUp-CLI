//! Native work-context port. Exact resume stays distinct from rehydrate.
//!
//! Public path `licoup_agent_runtime::work_context` is frozen. M1 fills the
//! preregistered leaf modules without editing `lib.rs` or these signatures.

mod generated;
mod mutate;
mod negotiate;
mod protocol;
mod resume;
mod runtime;
mod types;
mod writer;

pub use generated::{
    ContinuityCapabilitySupport as NativeCapabilitySupport, ContinuityDecisionLayer,
    ContinuityEffectClass, ContinuityFailure as NativeWorkContextFailure, ContinuityFailureCode,
    ContinuityFailureStage, ContinuityNativeCapabilitySnapshot as NativeCapabilitySnapshot,
    ContinuityRecoveryClass,
};
pub use negotiate::dimensions_are_independent;
pub use protocol::{HermeticProtocol, NativeProtocolAdapter, ProtocolOutcome};
pub use runtime::{WorkContextConfig, WorkContextRuntime};
pub use types::{
    AuthorIdentity, BindingRecord, BindingStatus, CapabilityProfile, ChildBinding, CoordinatorKind,
    ForkInheritance, GoalInference, HandoffRecord, IsolationReview, IsolationVerdict, LateResult,
    NativeAttemptRef, NativeControlIntent, NativeControlRequest, NativeFidelity, NativeSurface,
    OperationKind, ParallelPolicy, ProtocolEffect, ProtocolFamily, ProtocolMethods, SafeReason,
    SessionPresence, SourceCheckpoint, TurnExit, WorkContextOperation, default_snapshot,
    goal_inference_from_turn, identity_conflict, invalid_request, isolation_unverified,
    native_binding_lost, protocol_methods, reconciliation_required, validate_control_request,
    validate_key, writer_busy,
};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NativeWorkContextKey {
    pub conversation_id: String,
    pub membership_id: String,
    pub matter_id: String,
    pub generation: i64,
}

pub const fn unsupported_capability() -> NativeWorkContextFailure {
    NativeWorkContextFailure {
        code: ContinuityFailureCode::UnsupportedCapability,
        stage: ContinuityFailureStage::ContinuityNative,
        recovery: ContinuityRecoveryClass::ReviewOrWait,
        effect_class: ContinuityEffectClass::None,
        decision_layer: ContinuityDecisionLayer::Effects,
        retryable: false,
    }
}

pub trait NativeWorkContextPort: Send + Sync {
    fn negotiate(
        &self,
        key: &NativeWorkContextKey,
    ) -> Result<NativeCapabilitySnapshot, NativeWorkContextFailure>;
    fn exact_resume(&self, key: &NativeWorkContextKey) -> Result<(), NativeWorkContextFailure>;
    fn rehydrate(&self, key: &NativeWorkContextKey) -> Result<i64, NativeWorkContextFailure>;
    fn fork(&self, key: &NativeWorkContextKey) -> Result<i64, NativeWorkContextFailure>;
    fn compact(&self, key: &NativeWorkContextKey) -> Result<(), NativeWorkContextFailure>;
    fn steer(&self, request: &NativeControlRequest) -> Result<(), NativeWorkContextFailure>;
    fn cancel(&self, request: &NativeControlRequest) -> Result<(), NativeWorkContextFailure>;
    fn claim_writer(&self, key: &NativeWorkContextKey) -> Result<(), NativeWorkContextFailure>;
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct UnavailableNativeWorkContext;

impl NativeWorkContextPort for UnavailableNativeWorkContext {
    fn negotiate(
        &self,
        key: &NativeWorkContextKey,
    ) -> Result<NativeCapabilitySnapshot, NativeWorkContextFailure> {
        negotiate::negotiate(key)
    }

    fn exact_resume(&self, key: &NativeWorkContextKey) -> Result<(), NativeWorkContextFailure> {
        resume::exact_resume(key)
    }

    fn rehydrate(&self, key: &NativeWorkContextKey) -> Result<i64, NativeWorkContextFailure> {
        resume::rehydrate(key)
    }

    fn fork(&self, key: &NativeWorkContextKey) -> Result<i64, NativeWorkContextFailure> {
        mutate::fork(key)
    }

    fn compact(&self, key: &NativeWorkContextKey) -> Result<(), NativeWorkContextFailure> {
        mutate::compact(key)
    }

    fn steer(&self, request: &NativeControlRequest) -> Result<(), NativeWorkContextFailure> {
        mutate::steer(request)
    }

    fn cancel(&self, request: &NativeControlRequest) -> Result<(), NativeWorkContextFailure> {
        mutate::cancel(request)
    }

    fn claim_writer(&self, key: &NativeWorkContextKey) -> Result<(), NativeWorkContextFailure> {
        writer::claim_writer(key)
    }
}

pub fn unavailable_work_context_port() -> UnavailableNativeWorkContext {
    UnavailableNativeWorkContext
}

/// Reusable production entry. M2 supplies a real adapter; M1 uses hermetic fixtures.
pub fn work_context_runtime(
    adapter: impl NativeProtocolAdapter + 'static,
    config: WorkContextConfig,
) -> WorkContextRuntime {
    WorkContextRuntime::new(adapter, config)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture_key() -> NativeWorkContextKey {
        NativeWorkContextKey {
            conversation_id: "conversation:fixture".into(),
            membership_id: "membership:fixture".into(),
            matter_id: "matter:fixture".into(),
            generation: 1,
        }
    }

    #[test]
    fn unavailable_ops_return_unsupported_capability_without_writer_busy() {
        let port = UnavailableNativeWorkContext;
        let key = fixture_key();
        let errors = [
            port.negotiate(&key).unwrap_err(),
            port.exact_resume(&key).unwrap_err(),
            port.rehydrate(&key).unwrap_err(),
            port.fork(&key).unwrap_err(),
            port.compact(&key).unwrap_err(),
            port.steer(&NativeControlRequest::steer(
                key.clone(),
                "steer-guidance",
                "turn:host",
                "turn:native",
            ))
            .unwrap_err(),
            port.cancel(&NativeControlRequest::cancel(
                key.clone(),
                "turn:host",
                "turn:native",
            ))
            .unwrap_err(),
            port.claim_writer(&key).unwrap_err(),
        ];
        for error in errors {
            assert_eq!(error.code, ContinuityFailureCode::UnsupportedCapability);
            assert_eq!(error.stage, ContinuityFailureStage::ContinuityNative);
            assert_eq!(error.effect_class, ContinuityEffectClass::None);
            assert!(!error.retryable);
        }
    }

    #[test]
    fn capability_dimensions_remain_independent() {
        let high = default_snapshot(ProtocolFamily::Codex, CapabilityProfile::High);
        let low = default_snapshot(ProtocolFamily::Codex, CapabilityProfile::Low);
        assert_eq!(high.exact_resume, NativeCapabilitySupport::Supported);
        assert_eq!(high.fork, NativeCapabilitySupport::Unsupported);
        assert_eq!(high.parallel_contexts, NativeCapabilitySupport::Supported);
        assert_eq!(low.parallel_contexts, NativeCapabilitySupport::Unsupported);
        assert_eq!(low.isolated_context, NativeCapabilitySupport::Unverified);
        assert!(negotiate::dimensions_are_independent(&high));
    }
}
