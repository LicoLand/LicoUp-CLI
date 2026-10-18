use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

use crate::{FallbackReceipt, PendingCallback, StrategyRunStatus, WorkflowDefinition};

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct BindingCandidate {
    pub value_id: String,
    #[serde(default)]
    pub model: String,
    #[serde(default)]
    pub reasoning_effort: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct BindingValue {
    pub slot_id: String,
    #[serde(default)]
    pub ordinal: u8,
    pub value_id: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub model: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub reasoning_effort: String,
    #[serde(default)]
    pub revision: u64,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StrategyAuthorization {
    pub definition_digest: String,
    pub semantics_digest: String,
    pub binding_digest: String,
    pub authorization_digest: String,
    pub revision: u64,
    pub active: bool,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StrategyDefinitionSummary {
    pub definition_id: String,
    pub revision_digest: String,
    pub semantics_digest: String,
    pub name: String,
    pub version: String,
    pub imported_at_unix_ms: i64,
    #[serde(default)]
    pub authorized: bool,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StrategyDefinition {
    #[serde(flatten)]
    pub summary: StrategyDefinitionSummary,
    pub workflow: WorkflowDefinition,
    pub asset_count: usize,
    pub bindings: Vec<BindingValue>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub authorization: Option<StrategyAuthorization>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StrategyDiagnostic {
    pub code: String,
    pub component: String,
    pub retryable: bool,
    pub recovery: String,
    #[serde(default)]
    pub arguments: BTreeMap<String, String>,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StrategyProjection {
    pub schema: String,
    pub definition: StrategyDefinitionSummary,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub run_id: Option<String>,
    pub status: StrategyRunStatus,
    pub current_states: BTreeSet<String>,
    pub neighbor_states: BTreeSet<String>,
    pub allowed_operations: BTreeSet<String>,
    pub bindings: Vec<BindingValue>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub diagnostic: Option<StrategyDiagnostic>,
    pub history_count: u64,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub fallbacks: Vec<FallbackReceipt>,
    /// Callback-mode edges that settled and now wait for the master agent's
    /// decision, in the deterministic order the waits were entered.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub pending_callbacks: Vec<PendingCallback>,
    #[serde(default)]
    pub needs_human_input: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub entry_session_id: Option<String>,
}

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
