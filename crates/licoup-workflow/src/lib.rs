//! Pure LicoUp workflow compiler and transition machine.
//!
//! The generic compilation and transition machinery is owned by `acativa`;
//! this crate owns the LicoUp-bound vocabulary layered over it: the
//! `licoup.adaptive-flywheel.workflow.v1` schema identity this build compiles
//! under, the persisted run snapshot and reducer event spellings
//! (`conversationId`, assistant memberships, assistant failure events, the
//! three-way callback decision), and the strategy error vocabulary the host
//! surfaces.
//!
//! Workflow definitions are parsed and analyzed once into immutable indexes.
//! Runtime state changes are pure: callers persist the returned snapshot and
//! commands, then execute effects through their owning adapters.

pub use acativa::workflow::dispatch;

pub mod analysis;
pub mod compile;
pub mod diagnostic;
pub mod ir;
pub mod machine;
pub mod strategy;
pub mod syntax;

pub use analysis::{
    AnalyzedWorkflow, WorkflowValidation, WorkflowValidationFailure, analyze,
    compile_workflow_source, compile_workflow_value, validate_workflow_value,
};
pub use compile::CompiledWorkflow;
pub use diagnostic::{
    PreflightDiagnostic, WorkflowDiagnosticActualKind, WorkflowDiagnosticCode,
    WorkflowDiagnosticExpected, WorkflowDiagnosticRecovery, WorkflowDiagnosticStage,
};
pub use dispatch::{
    ClaimLease, ClaimVerdict, CompletionVerdict, DispatchDecision, DispatchGate, DispatchIntent,
    DispatchRecipient, DispatchRefusal, EffectCompletion, EffectProgress, RecoveryVerdict,
    SuccessorAdmission, SuccessorEdge, SuccessorGate, SuccessorRefusal, claim_effect,
    recover_claim, settle_effect,
};
pub use ir::*;
pub use machine::{
    CommandKind, CommandStatus, ReducerEvent, ReducerOutput, RunCommand, RunSnapshot, reduce,
};
pub use strategy::{
    ASSISTANT_TEMPORARY_DEFINITION_PREFIX, BindingCandidate, BindingValue, StrategyAuthorization,
    StrategyDefinition, StrategyDefinitionSummary, StrategyDiagnostic, StrategyError,
    StrategyErrorCode, StrategyProjection,
};
pub use syntax::{ParsedWorkflow, parse};

/// The workflow schema identity this build compiles and validates under. It is
/// bound once here so the store, the runtime and every caller share the same
/// vocabulary; the engine underneath stays schema-free.
pub const WORKFLOW_SCHEMA_VERSION: &str = "licoup.adaptive-flywheel.workflow.v1";

/// The schema identity the compiler is given, as the engine's own type.
pub(crate) fn workflow_schema() -> SchemaId {
    SchemaId::new(WORKFLOW_SCHEMA_VERSION)
}

pub use acativa::SchemaId;

pub const MAX_ACTIVE_EFFECTS: usize = acativa::MAX_ACTIVE_EFFECTS;
pub const MAX_GRAPH_STATES: usize = acativa::MAX_GRAPH_STATES;
pub const MAX_GRAPH_TRANSITIONS: usize = acativa::MAX_GRAPH_TRANSITIONS;
pub const MAX_BINDING_SLOTS: usize = acativa::MAX_BINDING_SLOTS;
pub const MAX_RUNTIME_REQUIREMENTS: usize = acativa::MAX_RUNTIME_REQUIREMENTS;
pub const MAX_WORKSET_ITEMS: usize = acativa::MAX_WORKSET_ITEMS;
pub const MAX_RETRY_ATTEMPTS: u8 = acativa::MAX_RETRY_ATTEMPTS;

/// Compile a canonical in-memory definition through the same analysis path as
/// source and value entry points.
pub fn compile_workflow(
    definition: WorkflowDefinition,
) -> Result<CompiledWorkflow, WorkflowValidationFailure> {
    acativa::compile_workflow(definition, &workflow_schema())
        .map_err(WorkflowValidationFailure::from)
}
