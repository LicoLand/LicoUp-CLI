//! Strategy definition, binding, projection and error vocabulary.
//!
//! The persisted definition/binding/projection records are the generic engine
//! vocabulary, re-exported unchanged so the store's rows and the wire
//! projections keep their exact shape. What stays here is the host-facing
//! error surface: [`StrategyErrorCode`] is the `strategy.*` command's typed
//! refusal spelling, and [`ASSISTANT_TEMPORARY_DEFINITION_PREFIX`] names the
//! definition space the Assistant author owns.

pub use acativa::workflow::strategy::{
    BindingCandidate, BindingValue, StrategyAuthorization, StrategyDefinition,
    StrategyDefinitionSummary, StrategyDiagnostic, StrategyProjection,
};

use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum StrategyErrorCode {
    InvalidRequest,
    PackageUnavailable,
    PackageTooLarge,
    PackageEntryInvalid,
    PackageLayoutInvalid,
    PackageDuplicateEntry,
    PackageResourceLimit,
    WorkflowInvalid,
    DefinitionNotFound,
    PreparationNotFound,
    RevisionConflict,
    BindingIncomplete,
    AuthorizationRequired,
    AuthorizationStale,
    RuntimeUnavailable,
    RuntimeDrifted,
    SandboxUnavailable,
    PermitDenied,
    RunNotFound,
    RunNotRetryable,
    CallbackStale,
    CallbackConflict,
    EffectInDoubt,
    UnsupportedAction,
}

impl StrategyErrorCode {
    pub const fn wire(self) -> &'static str {
        match self {
            Self::InvalidRequest => "invalid_request",
            Self::PackageUnavailable => "package_unavailable",
            Self::PackageTooLarge => "package_too_large",
            Self::PackageEntryInvalid => "package_entry_invalid",
            Self::PackageLayoutInvalid => "package_layout_invalid",
            Self::PackageDuplicateEntry => "package_duplicate_entry",
            Self::PackageResourceLimit => "package_resource_limit",
            Self::WorkflowInvalid => "workflow_invalid",
            Self::DefinitionNotFound => "definition_not_found",
            Self::PreparationNotFound => "preparation_not_found",
            Self::RevisionConflict => "revision_conflict",
            Self::BindingIncomplete => "binding_incomplete",
            Self::AuthorizationRequired => "authorization_required",
            Self::AuthorizationStale => "authorization_stale",
            Self::RuntimeUnavailable => "runtime_unavailable",
            Self::RuntimeDrifted => "runtime_drifted",
            Self::SandboxUnavailable => "sandbox_unavailable",
            Self::PermitDenied => "permit_denied",
            Self::RunNotFound => "run_not_found",
            Self::RunNotRetryable => "run_not_retryable",
            Self::CallbackStale => "callback_stale",
            Self::CallbackConflict => "callback_conflict",
            Self::EffectInDoubt => "effect_in_doubt",
            Self::UnsupportedAction => "unsupported_action",
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct StrategyError {
    pub code: StrategyErrorCode,
    pub stage: &'static str,
    pub component: &'static str,
    pub retryable: bool,
    pub recovery: &'static str,
}

impl StrategyError {
    pub const fn new(
        code: StrategyErrorCode,
        stage: &'static str,
        component: &'static str,
        retryable: bool,
        recovery: &'static str,
    ) -> Self {
        Self {
            code,
            stage,
            component,
            retryable,
            recovery,
        }
    }
}

impl std::fmt::Display for StrategyError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(self.code.wire())
    }
}

impl std::error::Error for StrategyError {}

/// Source prefix naming a definition the Assistant created for one temporary
/// Graph run rather than one the user imported. The prefix is part of the
/// definition identity, so it stays with the definition vocabulary.
pub const ASSISTANT_TEMPORARY_DEFINITION_PREFIX: &str = "assistant-temporary";
