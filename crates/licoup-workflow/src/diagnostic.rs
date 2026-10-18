use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkflowDiagnosticCode {
    WorkflowSyntaxInvalid,
    WorkflowShapeInvalid,
    WorkflowRequiredFieldMissing,
    WorkflowFieldTypeInvalid,
    WorkflowFieldValueInvalid,
    WorkflowUnknownField,
    WorkflowSchemaUnsupported,
    WorkflowMetadataIdInvalid,
    WorkflowMetadataNameInvalid,
    WorkflowMetadataVersionInvalid,
    WorkflowStateLimit,
    WorkflowTransitionLimit,
    WorkflowBindingLimit,
    WorkflowRuntimeLimit,
    WorkflowParallelismInvalid,
    WorkflowWorksetLimitInvalid,
    WorkflowRetryLimitInvalid,
    WorkflowBindingIdInvalid,
    WorkflowBindingDuplicate,
    WorkflowBindingLabelInvalid,
    WorkflowFallbackInvalid,
    WorkflowEntrySlotInvalid,
    WorkflowRuntimeIdInvalid,
    WorkflowRuntimeBindingInvalid,
    WorkflowWorksetIdInvalid,
    WorkflowWorksetItemBindingInvalid,
    WorkflowWorksetPredecessorFieldInvalid,
    WorkflowWorksetFieldConflict,
    WorkflowStateIdInvalid,
    WorkflowStateLabelInvalid,
    WorkflowStateInstructionInvalid,
    WorkflowStateDuplicate,
    WorkflowStateRetryInvalid,
    WorkflowActorBindingInvalid,
    WorkflowStateFieldInvalid,
    WorkflowScriptRuntimeInvalid,
    WorkflowScriptEntryMissing,
    WorkflowScriptEntryInvalid,
    WorkflowWorksetReferenceInvalid,
    WorkflowWorksetBindingInvalid,
    WorkflowInitialUnknown,
    WorkflowTransitionIdInvalid,
    WorkflowTransitionDuplicate,
    WorkflowTransitionStateUnknown,
    WorkflowGuardInvalid,
    WorkflowGuardAmbiguous,
    WorkflowRoutingInvalid,
    WorkflowTransitionModeInvalid,
    WorkflowFlowTargetIncomplete,
    WorkflowTopologyInvalid,
    WorkflowStateUnreachable,
    WorkflowTerminalUnreachable,
    WorkflowEffectCycle,
    WorkflowInvalid,
    GraphIdentityNotAssistantTemporary,
    GraphRuntimeAssetUnavailable,
    GraphAssistantMembershipRejected,
    GraphAssistantDesignationRejected,
    GraphAssistantSkillUnavailable,
    GraphAuthorityRejected,
    GraphBindingDuplicate,
    GraphBindingUnknown,
    GraphBindingKindRejected,
    GraphBindingIncomplete,
    GraphMembershipRejected,
    GraphModelUnavailable,
    GraphModelRejected,
    GraphReadinessRejected,
    GraphEnvironmentUnavailable,
    GraphProfileRejected,
    ConversationStateUnavailable,
    ConversationNotFound,
    GraphRouteStale,
}

impl WorkflowDiagnosticCode {
    pub const fn wire(self) -> &'static str {
        match self {
            Self::WorkflowSyntaxInvalid => "workflow_syntax_invalid",
            Self::WorkflowShapeInvalid => "workflow_shape_invalid",
            Self::WorkflowRequiredFieldMissing => "workflow_required_field_missing",
            Self::WorkflowFieldTypeInvalid => "workflow_field_type_invalid",
            Self::WorkflowFieldValueInvalid => "workflow_field_value_invalid",
            Self::WorkflowUnknownField => "workflow_unknown_field",
            Self::WorkflowSchemaUnsupported => "workflow_schema_unsupported",
            Self::WorkflowMetadataIdInvalid => "workflow_metadata_id_invalid",
            Self::WorkflowMetadataNameInvalid => "workflow_metadata_name_invalid",
            Self::WorkflowMetadataVersionInvalid => "workflow_metadata_version_invalid",
            Self::WorkflowStateLimit => "workflow_state_limit",
            Self::WorkflowTransitionLimit => "workflow_transition_limit",
            Self::WorkflowBindingLimit => "workflow_binding_limit",
            Self::WorkflowRuntimeLimit => "workflow_runtime_limit",
            Self::WorkflowParallelismInvalid => "workflow_parallelism_invalid",
            Self::WorkflowWorksetLimitInvalid => "workflow_workset_limit_invalid",
            Self::WorkflowRetryLimitInvalid => "workflow_retry_limit_invalid",
            Self::WorkflowBindingIdInvalid => "workflow_binding_id_invalid",
            Self::WorkflowBindingDuplicate => "workflow_binding_duplicate",
            Self::WorkflowBindingLabelInvalid => "workflow_binding_label_invalid",
            Self::WorkflowFallbackInvalid => "workflow_fallback_invalid",
            Self::WorkflowEntrySlotInvalid => "workflow_entry_slot_invalid",
            Self::WorkflowRuntimeIdInvalid => "workflow_runtime_id_invalid",
            Self::WorkflowRuntimeBindingInvalid => "workflow_runtime_binding_invalid",
            Self::WorkflowWorksetIdInvalid => "workflow_workset_id_invalid",
            Self::WorkflowWorksetItemBindingInvalid => "workflow_workset_item_binding_invalid",
            Self::WorkflowWorksetPredecessorFieldInvalid => {
                "workflow_workset_predecessor_field_invalid"
            }
            Self::WorkflowWorksetFieldConflict => "workflow_workset_field_conflict",
            Self::WorkflowStateIdInvalid => "workflow_state_id_invalid",
            Self::WorkflowStateLabelInvalid => "workflow_state_label_invalid",
            Self::WorkflowStateInstructionInvalid => "workflow_state_instruction_invalid",
            Self::WorkflowStateDuplicate => "workflow_state_duplicate",
            Self::WorkflowStateRetryInvalid => "workflow_state_retry_invalid",
            Self::WorkflowActorBindingInvalid => "workflow_actor_binding_invalid",
            Self::WorkflowStateFieldInvalid => "workflow_state_field_invalid",
            Self::WorkflowScriptRuntimeInvalid => "workflow_script_runtime_invalid",
            Self::WorkflowScriptEntryMissing => "workflow_script_entry_missing",
            Self::WorkflowScriptEntryInvalid => "workflow_script_entry_invalid",
            Self::WorkflowWorksetReferenceInvalid => "workflow_workset_reference_invalid",
            Self::WorkflowWorksetBindingInvalid => "workflow_workset_binding_invalid",
            Self::WorkflowInitialUnknown => "workflow_initial_unknown",
            Self::WorkflowTransitionIdInvalid => "workflow_transition_id_invalid",
            Self::WorkflowTransitionDuplicate => "workflow_transition_duplicate",
            Self::WorkflowTransitionStateUnknown => "workflow_transition_state_unknown",
            Self::WorkflowGuardInvalid => "workflow_guard_invalid",
            Self::WorkflowGuardAmbiguous => "workflow_guard_ambiguous",
            Self::WorkflowRoutingInvalid => "workflow_routing_invalid",
            Self::WorkflowTransitionModeInvalid => "workflow_transition_mode_invalid",
            Self::WorkflowFlowTargetIncomplete => "workflow_flow_target_incomplete",
            Self::WorkflowTopologyInvalid => "workflow_topology_invalid",
            Self::WorkflowStateUnreachable => "workflow_state_unreachable",
            Self::WorkflowTerminalUnreachable => "workflow_terminal_unreachable",
            Self::WorkflowEffectCycle => "workflow_effect_cycle",
            Self::WorkflowInvalid => "workflow_invalid",
            Self::GraphIdentityNotAssistantTemporary => "graph_identity_not_assistant_temporary",
            Self::GraphRuntimeAssetUnavailable => "graph_runtime_asset_unavailable",
            Self::GraphAssistantMembershipRejected => "graph_assistant_membership_rejected",
            Self::GraphAssistantDesignationRejected => "graph_assistant_designation_rejected",
            Self::GraphAssistantSkillUnavailable => "graph_assistant_skill_unavailable",
            Self::GraphAuthorityRejected => "graph_authority_rejected",
            Self::GraphBindingDuplicate => "graph_binding_duplicate",
            Self::GraphBindingUnknown => "graph_binding_unknown",
            Self::GraphBindingKindRejected => "graph_binding_kind_rejected",
            Self::GraphBindingIncomplete => "graph_binding_incomplete",
            Self::GraphMembershipRejected => "graph_membership_rejected",
            Self::GraphModelUnavailable => "graph_model_unavailable",
            Self::GraphModelRejected => "graph_model_rejected",
            Self::GraphReadinessRejected => "graph_readiness_rejected",
            Self::GraphEnvironmentUnavailable => "graph_environment_unavailable",
            Self::GraphProfileRejected => "graph_profile_rejected",
            Self::ConversationStateUnavailable => "conversation_state_unavailable",
            Self::ConversationNotFound => "conversation_not_found",
            Self::GraphRouteStale => "graph_route_stale",
        }
    }

    pub fn from_wire(value: &str) -> Option<Self> {
        const VALUES: &[WorkflowDiagnosticCode] = &[
            WorkflowDiagnosticCode::WorkflowSyntaxInvalid,
            WorkflowDiagnosticCode::WorkflowShapeInvalid,
            WorkflowDiagnosticCode::WorkflowRequiredFieldMissing,
            WorkflowDiagnosticCode::WorkflowFieldTypeInvalid,
            WorkflowDiagnosticCode::WorkflowFieldValueInvalid,
            WorkflowDiagnosticCode::WorkflowUnknownField,
            WorkflowDiagnosticCode::WorkflowSchemaUnsupported,
            WorkflowDiagnosticCode::WorkflowMetadataIdInvalid,
            WorkflowDiagnosticCode::WorkflowMetadataNameInvalid,
            WorkflowDiagnosticCode::WorkflowMetadataVersionInvalid,
            WorkflowDiagnosticCode::WorkflowStateLimit,
            WorkflowDiagnosticCode::WorkflowTransitionLimit,
            WorkflowDiagnosticCode::WorkflowBindingLimit,
            WorkflowDiagnosticCode::WorkflowRuntimeLimit,
            WorkflowDiagnosticCode::WorkflowParallelismInvalid,
            WorkflowDiagnosticCode::WorkflowWorksetLimitInvalid,
            WorkflowDiagnosticCode::WorkflowRetryLimitInvalid,
            WorkflowDiagnosticCode::WorkflowBindingIdInvalid,
            WorkflowDiagnosticCode::WorkflowBindingDuplicate,
            WorkflowDiagnosticCode::WorkflowBindingLabelInvalid,
            WorkflowDiagnosticCode::WorkflowFallbackInvalid,
            WorkflowDiagnosticCode::WorkflowEntrySlotInvalid,
            WorkflowDiagnosticCode::WorkflowRuntimeIdInvalid,
            WorkflowDiagnosticCode::WorkflowRuntimeBindingInvalid,
            WorkflowDiagnosticCode::WorkflowWorksetIdInvalid,
            WorkflowDiagnosticCode::WorkflowWorksetItemBindingInvalid,
            WorkflowDiagnosticCode::WorkflowWorksetPredecessorFieldInvalid,
            WorkflowDiagnosticCode::WorkflowWorksetFieldConflict,
            WorkflowDiagnosticCode::WorkflowStateIdInvalid,
            WorkflowDiagnosticCode::WorkflowStateLabelInvalid,
            WorkflowDiagnosticCode::WorkflowStateInstructionInvalid,
            WorkflowDiagnosticCode::WorkflowStateDuplicate,
            WorkflowDiagnosticCode::WorkflowStateRetryInvalid,
            WorkflowDiagnosticCode::WorkflowActorBindingInvalid,
            WorkflowDiagnosticCode::WorkflowStateFieldInvalid,
            WorkflowDiagnosticCode::WorkflowScriptRuntimeInvalid,
            WorkflowDiagnosticCode::WorkflowScriptEntryMissing,
            WorkflowDiagnosticCode::WorkflowScriptEntryInvalid,
            WorkflowDiagnosticCode::WorkflowWorksetReferenceInvalid,
            WorkflowDiagnosticCode::WorkflowWorksetBindingInvalid,
            WorkflowDiagnosticCode::WorkflowInitialUnknown,
            WorkflowDiagnosticCode::WorkflowTransitionIdInvalid,
            WorkflowDiagnosticCode::WorkflowTransitionDuplicate,
            WorkflowDiagnosticCode::WorkflowTransitionStateUnknown,
            WorkflowDiagnosticCode::WorkflowGuardInvalid,
            WorkflowDiagnosticCode::WorkflowGuardAmbiguous,
            WorkflowDiagnosticCode::WorkflowRoutingInvalid,
            WorkflowDiagnosticCode::WorkflowTransitionModeInvalid,
            WorkflowDiagnosticCode::WorkflowFlowTargetIncomplete,
            WorkflowDiagnosticCode::WorkflowTopologyInvalid,
            WorkflowDiagnosticCode::WorkflowStateUnreachable,
            WorkflowDiagnosticCode::WorkflowTerminalUnreachable,
            WorkflowDiagnosticCode::WorkflowEffectCycle,
            WorkflowDiagnosticCode::WorkflowInvalid,
            WorkflowDiagnosticCode::GraphIdentityNotAssistantTemporary,
            WorkflowDiagnosticCode::GraphRuntimeAssetUnavailable,
            WorkflowDiagnosticCode::GraphAssistantMembershipRejected,
            WorkflowDiagnosticCode::GraphAssistantDesignationRejected,
            WorkflowDiagnosticCode::GraphAssistantSkillUnavailable,
            WorkflowDiagnosticCode::GraphAuthorityRejected,
            WorkflowDiagnosticCode::GraphBindingDuplicate,
            WorkflowDiagnosticCode::GraphBindingUnknown,
            WorkflowDiagnosticCode::GraphBindingKindRejected,
            WorkflowDiagnosticCode::GraphBindingIncomplete,
            WorkflowDiagnosticCode::GraphMembershipRejected,
            WorkflowDiagnosticCode::GraphModelUnavailable,
            WorkflowDiagnosticCode::GraphModelRejected,
            WorkflowDiagnosticCode::GraphReadinessRejected,
            WorkflowDiagnosticCode::GraphEnvironmentUnavailable,
            WorkflowDiagnosticCode::GraphProfileRejected,
            WorkflowDiagnosticCode::ConversationStateUnavailable,
            WorkflowDiagnosticCode::ConversationNotFound,
            WorkflowDiagnosticCode::GraphRouteStale,
        ];
        VALUES
            .iter()
            .copied()
            .find(|candidate| candidate.wire() == value)
    }
}

impl PartialEq<&str> for WorkflowDiagnosticCode {
    fn eq(&self, other: &&str) -> bool {
        self.wire() == *other
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
pub enum WorkflowDiagnosticStage {
    #[serde(rename = "workflow/parse")]
    WorkflowParse,
    #[serde(rename = "workflow/compile")]
    WorkflowCompile,
    #[serde(rename = "package/validate")]
    PackageValidate,
    #[serde(rename = "assistant-workflow/preflight")]
    AssistantWorkflowPreflight,
    #[serde(rename = "assistant-workflow/revalidate")]
    AssistantWorkflowRevalidate,
}

impl WorkflowDiagnosticStage {
    pub const fn wire(self) -> &'static str {
        match self {
            Self::WorkflowParse => "workflow/parse",
            Self::WorkflowCompile => "workflow/compile",
            Self::PackageValidate => "package/validate",
            Self::AssistantWorkflowPreflight => "assistant-workflow/preflight",
            Self::AssistantWorkflowRevalidate => "assistant-workflow/revalidate",
        }
    }

    pub fn from_wire(value: &str) -> Option<Self> {
        match value {
            "workflow/parse" => Some(Self::WorkflowParse),
            "workflow/compile" => Some(Self::WorkflowCompile),
            "package/validate" => Some(Self::PackageValidate),
            "assistant-workflow/preflight" => Some(Self::AssistantWorkflowPreflight),
            "assistant-workflow/revalidate" => Some(Self::AssistantWorkflowRevalidate),
            _ => None,
        }
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkflowDiagnosticExpected {
    Object,
    Array,
    String,
    Integer,
    Boolean,
    EnumValue,
    Identifier,
    NonEmptyText,
    UniqueId,
    ExistingReference,
    SupportedSchema,
    ValidRouting,
    ValidTopology,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkflowDiagnosticActualKind {
    Missing,
    Null,
    Object,
    Array,
    String,
    Number,
    Boolean,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkflowDiagnosticRecovery {
    AddRequiredField,
    CorrectField,
    RemoveUnknownField,
    RemoveDuplicate,
    CorrectReference,
    ReduceResource,
    CorrectRouting,
    CorrectTopology,
    UpdateAssistantProfile,
    UpdateBinding,
    RefreshConversationState,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PreflightDiagnostic {
    pub code: WorkflowDiagnosticCode,
    pub stage: WorkflowDiagnosticStage,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub related_paths: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub membership_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub actual: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expected: Option<WorkflowDiagnosticExpected>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub actual_kind: Option<WorkflowDiagnosticActualKind>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub recovery: Option<WorkflowDiagnosticRecovery>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub line: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub column: Option<u64>,
}
