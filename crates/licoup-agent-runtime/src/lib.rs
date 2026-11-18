//! Host-neutral Agent runtime boundary.
//!
//! The host-neutral contract vocabulary — provider identity, the capability
//! evidence and readiness reducers, opaque session bindings, the dispatch
//! receipt vocabulary, the caller-registration and target-runtime ports, the
//! lane-erased driver contracts and the immutable protocol selector — lives
//! in the shared adapter core `a4aa`; the re-exports below keep every former
//! `licoup_agent_runtime` path reachable.
//!
//! What this crate still owns is the product side: [`work_context`], the
//! Canonical Conversation work-context seam whose keys, bindings, generated
//! continuity vocabulary and per-Agent protocol tables are LicoUp policy,
//! not host-neutral contract.
//!
//! Concrete process, CLI, RPC, and approval drivers remain private to the
//! composing host.

pub mod work_context;

pub use a4aa::protocol_selector;
pub use a4aa::runtime::{
    AdapterFailure, CallerRegistrationPlan, CallerRegistrationReceipt, DurableNativeBinding,
    ExecutionAdmissionDecision, ExecutionAdmissionEvidence, InstructionPolicy,
    McpCallerIntegration, NativeResumeIdentity, PersistentTurnRuntime, ProviderId,
    ReadinessDecision, ReadinessEvidence, RegistrationApproval, RuntimeDispatchReceipt,
    RuntimeDriver, RuntimeDriverRegistry, RuntimeEventPart, RuntimeObservation, RuntimeTransition,
    SubagentCapabilities, SubagentContinueRequest, SubagentDispatchRequest, SubagentRuntimeAdapter,
    reduce_execution_admission, reduce_readiness,
};
