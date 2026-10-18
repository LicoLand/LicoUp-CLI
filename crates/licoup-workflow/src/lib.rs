//! Pure LicoUp workflow compiler and transition machine.
//!
//! Workflow definitions are parsed and analyzed once into immutable indexes.
//! Runtime state changes are pure: callers persist the returned snapshot and
//! commands, then execute effects through their owning adapters.

pub mod analysis;
pub mod compile;
pub mod diagnostic;
pub mod dispatch;
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

pub const WORKFLOW_SCHEMA_VERSION: &str = "licoup.adaptive-flywheel.workflow.v1";
pub const MAX_ACTIVE_EFFECTS: usize = 8;
pub const MAX_GRAPH_STATES: usize = 512;
pub const MAX_GRAPH_TRANSITIONS: usize = 2_048;
pub const MAX_BINDING_SLOTS: usize = 64;
pub const MAX_RUNTIME_REQUIREMENTS: usize = 16;
pub const MAX_WORKSET_ITEMS: usize = 256;
pub const MAX_RETRY_ATTEMPTS: u8 = 8;

/// Compile a canonical in-memory definition through the same analysis path as
/// source and value entry points.
pub fn compile_workflow(
    definition: WorkflowDefinition,
) -> Result<CompiledWorkflow, WorkflowValidationFailure> {
    analysis::compile_workflow_definition(definition)
}
