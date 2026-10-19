//! Workflow source parsing.
//!
//! Parsing is the generic engine pass; this module binds it to this crate's
//! diagnostic record so parse failures carry the same `PreflightDiagnostic`
//! spelling the rest of the crate surfaces.

use crate::WorkflowValidationFailure;

/// Parsed workflow source. The JSON value retains source-level field paths;
/// semantic analysis consumes the canonical definition.
pub type ParsedWorkflow = acativa::workflow::syntax::ParsedWorkflow;

/// Parse one workflow source. Syntax and shape failures arrive as diagnostics
/// in this crate's vocabulary.
pub fn parse(source: &[u8]) -> Result<ParsedWorkflow, WorkflowValidationFailure> {
    acativa::workflow::syntax::parse(source).map_err(WorkflowValidationFailure::from)
}
