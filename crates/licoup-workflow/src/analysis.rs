//! Privacy-safe, deterministic workflow validation.
//!
//! The raw-value and semantic passes are the generic engine's; this module
//! binds them to this crate's workflow schema and to the `PreflightDiagnostic`
//! record this crate surfaces (the engine's record is a strict subset of it).

use serde_json::Value;
use std::fmt::{Display, Formatter};

use crate::compile::CompiledWorkflow;
use crate::diagnostic::PreflightDiagnostic;
use crate::syntax::ParsedWorkflow;
use crate::{WorkflowDefinition, workflow_schema};

#[derive(Clone, Debug)]
pub struct WorkflowValidation {
    pub definition: Option<WorkflowDefinition>,
    pub diagnostics: Vec<PreflightDiagnostic>,
}

#[derive(Clone, Debug)]
pub struct WorkflowValidationFailure {
    pub diagnostics: Vec<PreflightDiagnostic>,
}

/// A canonical definition that has passed the single semantic analysis pass.
/// Lowering is the only consumer of this phase marker.
#[derive(Clone, Debug)]
pub struct AnalyzedWorkflow(acativa::AnalyzedWorkflow);

impl AnalyzedWorkflow {
    pub fn source(&self) -> Option<&Value> {
        self.0.source()
    }
}

impl From<acativa::WorkflowValidationFailure> for WorkflowValidationFailure {
    fn from(failure: acativa::WorkflowValidationFailure) -> Self {
        Self {
            diagnostics: failure
                .diagnostics
                .iter()
                .cloned()
                .map(PreflightDiagnostic::from)
                .collect(),
        }
    }
}

impl Display for WorkflowValidationFailure {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(
            self.diagnostics
                .first()
                .map_or("workflow_invalid", |diagnostic| diagnostic.code.wire()),
        )
    }
}

impl std::error::Error for WorkflowValidationFailure {}

pub fn compile_workflow_source(
    source: &[u8],
) -> Result<CompiledWorkflow, WorkflowValidationFailure> {
    acativa::compile_workflow_source(source, &workflow_schema())
        .map_err(WorkflowValidationFailure::from)
}

pub fn compile_workflow_value(
    value: &Value,
) -> Result<CompiledWorkflow, WorkflowValidationFailure> {
    acativa::compile_workflow_value(value, &workflow_schema())
        .map_err(WorkflowValidationFailure::from)
}

pub fn validate_workflow_value(value: &Value) -> WorkflowValidation {
    let validation = acativa::validate_workflow_value(value, &workflow_schema());
    WorkflowValidation {
        definition: validation.definition,
        diagnostics: validation
            .diagnostics
            .iter()
            .cloned()
            .map(PreflightDiagnostic::from)
            .collect(),
    }
}

pub fn analyze(parsed: ParsedWorkflow) -> Result<AnalyzedWorkflow, WorkflowValidationFailure> {
    acativa::analyze(parsed, &workflow_schema())
        .map(AnalyzedWorkflow)
        .map_err(WorkflowValidationFailure::from)
}
