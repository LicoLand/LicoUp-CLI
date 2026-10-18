//! Assistant-temporary Graph admission.
//!
//! This Facade coordinates Conversation/Profile facts, Graph compilation and
//! exact Membership binding without owning any of those facts. Every
//! rejection here is pure and precedes durable admission or an effect permit.

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};
use std::fmt::{Display, Formatter};

use licoup_conversation::{
    LICOUP_GUIDE_SKILL_ID, MembershipProfileSnapshot, ProfileResponsibility,
};

use crate::ports::{CandidateFilters, ModelFactsPort, rank_candidates, route_receipt};
use licoup_workflow::{
    PreflightDiagnostic, WorkflowDiagnosticCode, WorkflowDiagnosticRecovery,
    WorkflowDiagnosticStage,
};

use licoup_workflow::{BindingKind, GraphStateKind, WorkflowDefinition, validate_workflow_value};

use licoup_workflow_store::BindingValue;

pub const ASSISTANT_TEMPORARY_DEFINITION_PREFIX: &str = "assistant-temporary";

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PreflightReceipt {
    pub conversation_id: String,
    pub assistant_membership_id: String,
    pub workflow_digest: String,
    pub membership_ids: Vec<String>,
    pub route_receipt: Value,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PreflightFailure {
    pub code: String,
    pub stage: String,
    pub retryable: bool,
    pub recovery: String,
    pub diagnostics: Vec<PreflightDiagnostic>,
}

impl Display for PreflightFailure {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.code)
    }
}

impl std::error::Error for PreflightFailure {}

impl PreflightFailure {
    pub(crate) fn new(code: &'static str, mut diagnostics: Vec<PreflightDiagnostic>) -> Self {
        diagnostics.sort_by(|left, right| {
            left.stage
                .cmp(&right.stage)
                .then_with(|| left.path.cmp(&right.path))
                .then_with(|| left.code.cmp(&right.code))
                .then_with(|| left.related_paths.cmp(&right.related_paths))
                .then_with(|| left.membership_id.cmp(&right.membership_id))
        });
        diagnostics.dedup();
        Self {
            code: code.to_owned(),
            stage: "assistant-workflow/preflight".to_owned(),
            retryable: true,
            recovery: "correct_graph_or_bindings_and_retry".to_owned(),
            diagnostics,
        }
    }
}

#[derive(Clone, Debug)]
pub struct AssistantPreflight {
    pub definition: WorkflowDefinition,
    pub bindings: Vec<BindingValue>,
    pub receipt: PreflightReceipt,
    /// Store-owned revisions revalidated immediately before durable admission.
    /// This is internal admission state, not a second public Profile projection.
    pub profile_revisions: BTreeMap<String, i64>,
}

pub fn preflight_assistant_graph(
    conversation_id: &str,
    assistant_membership_id: &str,
    workflow: &Value,
    bindings: &[BindingValue],
    snapshots: &[MembershipProfileSnapshot],
    filters: &CandidateFilters,
    model_facts: &dyn ModelFactsPort,
    selection_policy_revision: &str,
) -> Result<AssistantPreflight, PreflightFailure> {
    let validation = validate_workflow_value(workflow);
    let mut checks = validation.diagnostics;

    let by_membership = snapshots
        .iter()
        .map(|snapshot| (snapshot.membership_id.as_str(), snapshot))
        .collect::<BTreeMap<_, _>>();
    let assistant = by_membership.get(assistant_membership_id).copied();
    if assistant.is_none() {
        checks.push(check_at(
            "graph_assistant_membership_rejected",
            Some("/assistantMembershipId"),
            Some(assistant_membership_id),
        ));
    }
    if assistant
        .is_some_and(|assistant| assistant.responsibility != ProfileResponsibility::Assistant)
    {
        checks.push(check_at(
            "graph_assistant_designation_rejected",
            Some("/assistantMembershipId"),
            Some(assistant_membership_id),
        ));
    }
    if assistant.is_some_and(|assistant| {
        !assistant
            .skills
            .iter()
            .any(|skill| skill == LICOUP_GUIDE_SKILL_ID)
            || !conversation_driver_available(assistant)
    }) {
        checks.push(check_at(
            "graph_assistant_skill_unavailable",
            Some("/assistantMembershipId"),
            Some(assistant_membership_id),
        ));
    }
    if assistant.is_some_and(|assistant| {
        filters
            .required_authority
            .iter()
            .any(|required| !assistant.authority.contains(required))
    }) {
        checks.push(check_at(
            "graph_authority_rejected",
            Some("/filters/requiredAuthority"),
            Some(assistant_membership_id),
        ));
    }
    if assistant.is_some_and(|assistant| assistant.model.is_none()) {
        checks.push(check_at(
            "graph_model_unavailable",
            Some("/assistantMembershipId"),
            Some(assistant_membership_id),
        ));
    }
    if assistant.is_some_and(|assistant| assistant.readiness.as_deref() != Some("ready")) {
        checks.push(check_at(
            "graph_readiness_rejected",
            Some("/assistantMembershipId"),
            Some(assistant_membership_id),
        ));
    }
    if assistant.is_some_and(|assistant| assistant.environment.is_none()) {
        checks.push(check_at(
            "graph_environment_unavailable",
            Some("/assistantMembershipId"),
            Some(assistant_membership_id),
        ));
    }

    let Some(definition) = validation.definition else {
        return Err(PreflightFailure::new(failure_code(&checks), checks));
    };
    if !definition
        .metadata
        .id
        .starts_with(ASSISTANT_TEMPORARY_DEFINITION_PREFIX)
    {
        checks.push(diagnostic(
            "graph_identity_not_assistant_temporary",
            "assistant-workflow/preflight",
            Some("/metadata/id"),
            None,
            None,
            None,
        ));
    }
    if !definition.runtimes.is_empty()
        || definition
            .states
            .iter()
            .any(|state| state.kind == GraphStateKind::Script)
    {
        checks.push(check_at(
            "graph_runtime_asset_unavailable",
            Some("/runtimes"),
            None,
        ));
    }

    let actor_slots = definition
        .actor_slots
        .iter()
        .map(|slot| (slot.id.as_str(), slot))
        .collect::<BTreeMap<_, _>>();
    let required_slots = definition
        .actor_slots
        .iter()
        .filter(|slot| slot.required && slot.kind == BindingKind::Actor)
        .map(|slot| slot.id.as_str())
        .collect::<BTreeSet<_>>();
    let mut seen_bindings = BTreeSet::new();
    let mut bound_slots = BTreeSet::new();
    let mut bound_membership_ids = BTreeSet::new();
    let mut canonical_bindings = Vec::new();
    for (binding_index, binding) in bindings.iter().enumerate() {
        let binding_pointer = format!("/bindings/{binding_index}");
        if !seen_bindings.insert((binding.slot_id.as_str(), binding.ordinal)) {
            checks.push(check_at(
                "graph_binding_duplicate",
                Some(binding_pointer.as_str()),
                None,
            ));
            continue;
        }
        let Some(slot) = actor_slots.get(binding.slot_id.as_str()) else {
            checks.push(check_at(
                "graph_binding_unknown",
                Some(format!("{binding_pointer}/slotId").as_str()),
                None,
            ));
            continue;
        };
        if slot.kind != BindingKind::Actor || binding.ordinal >= 16 {
            checks.push(check_at(
                "graph_binding_kind_rejected",
                Some(binding_pointer.as_str()),
                None,
            ));
            continue;
        }
        if binding.ordinal == 0 {
            bound_slots.insert(binding.slot_id.as_str());
        }
        let Some(member) = by_membership.get(binding.value_id.as_str()).copied() else {
            checks.push(check_at(
                "graph_membership_rejected",
                Some(format!("{binding_pointer}/valueId").as_str()),
                Some(binding.value_id.as_str()),
            ));
            continue;
        };
        bound_membership_ids.insert(member.membership_id.clone());
        if member.membership_id != assistant_membership_id && member.model.is_none() {
            checks.push(check_at(
                "graph_model_unavailable",
                Some(format!("{binding_pointer}/model").as_str()),
                Some(member.membership_id.as_str()),
            ));
        } else if !binding.model.is_empty()
            && member.model.as_deref() != Some(binding.model.as_str())
        {
            checks.push(check_at(
                "graph_model_rejected",
                Some(format!("{binding_pointer}/model").as_str()),
                Some(member.membership_id.as_str()),
            ));
        }
        if member.membership_id != assistant_membership_id
            && member.readiness.as_deref() != Some("ready")
        {
            checks.push(check_at(
                "graph_readiness_rejected",
                Some(format!("{binding_pointer}/valueId").as_str()),
                Some(member.membership_id.as_str()),
            ));
        }
        if member.membership_id != assistant_membership_id && member.environment.is_none() {
            checks.push(check_at(
                "graph_environment_unavailable",
                Some(format!("{binding_pointer}/valueId").as_str()),
                Some(member.membership_id.as_str()),
            ));
        }
        canonical_bindings.push(BindingValue {
            slot_id: binding.slot_id.clone(),
            ordinal: binding.ordinal,
            value_id: member.membership_id.clone(),
            model: member.model.clone().unwrap_or_default(),
            reasoning_effort: binding.reasoning_effort.clone(),
            revision: 0,
        });
    }
    for slot in required_slots.difference(&bound_slots) {
        let slot_index = definition
            .actor_slots
            .iter()
            .position(|candidate| candidate.id == *slot)
            .unwrap_or_default();
        checks.push(check_at(
            "graph_binding_incomplete",
            Some(format!("/actorSlots/{slot_index}").as_str()),
            None,
        ));
    }

    let requested_memberships = filters
        .membership_ids
        .iter()
        .map(String::as_str)
        .collect::<BTreeSet<_>>();
    if !requested_memberships.is_empty()
        && bound_membership_ids
            .iter()
            .any(|membership_id| !requested_memberships.contains(membership_id.as_str()))
    {
        checks.push(check_at(
            "graph_membership_rejected",
            Some("/filters/membershipIds"),
            None,
        ));
    }
    let mut exact_filters = filters.clone();
    exact_filters.membership_ids = bound_membership_ids.iter().cloned().collect();
    let ranked = match rank_candidates(snapshots.to_vec(), &exact_filters) {
        Ok(ranked) => ranked,
        Err(_) => {
            checks.push(check_at("graph_profile_rejected", Some("/filters"), None));
            Vec::new()
        }
    };

    if !checks.is_empty() {
        return Err(PreflightFailure::new(failure_code(&checks), checks));
    }
    canonical_bindings.sort_by(|left, right| {
        left.slot_id
            .cmp(&right.slot_id)
            .then_with(|| left.ordinal.cmp(&right.ordinal))
            .then_with(|| left.value_id.cmp(&right.value_id))
    });
    // The receipt states the selection-policy revision the admitting caller
    // captured for this decision. Nothing is read and nothing is synthesized
    // here: a durable admission keeps the revision it was admitted under, so a
    // later adoption governs only the next task.
    let route_receipt = route_receipt(
        conversation_id,
        &ranked,
        model_facts,
        selection_policy_revision,
    );
    let digest_payload = json!({
        "workflow": definition,
        "bindings": canonical_bindings,
        "routeReceipt": route_receipt,
    });
    let workflow_digest = sha256_hex(&serde_json::to_vec(&digest_payload).unwrap_or_default());
    let membership_ids = bound_membership_ids.into_iter().collect::<Vec<_>>();
    let profile_revisions = snapshots
        .iter()
        .filter(|snapshot| {
            snapshot.membership_id == assistant_membership_id
                || membership_ids.contains(&snapshot.membership_id)
        })
        .map(|snapshot| (snapshot.membership_id.clone(), snapshot.intent_revision))
        .collect();
    Ok(AssistantPreflight {
        definition,
        bindings: canonical_bindings,
        receipt: PreflightReceipt {
            conversation_id: conversation_id.to_owned(),
            assistant_membership_id: assistant_membership_id.to_owned(),
            workflow_digest,
            membership_ids,
            route_receipt,
        },
        profile_revisions,
    })
}

fn failure_code(diagnostics: &[PreflightDiagnostic]) -> &'static str {
    if diagnostics.len() == 1
        && diagnostics[0].code == WorkflowDiagnosticCode::GraphIdentityNotAssistantTemporary
    {
        "graph_identity_rejected"
    } else if diagnostics.iter().any(|diagnostic| {
        matches!(
            diagnostic.stage,
            WorkflowDiagnosticStage::AssistantWorkflowPreflight
                | WorkflowDiagnosticStage::AssistantWorkflowRevalidate
        )
    }) {
        "graph_preflight_rejected"
    } else {
        "graph_invalid"
    }
}

fn conversation_driver_available(snapshot: &MembershipProfileSnapshot) -> bool {
    snapshot.capabilities.iter().any(|capability| {
        matches!(
            capability.as_str(),
            "conversationDriver:supported" | "conversationDriver:ready"
        )
    })
}

fn check_at(
    code: &str,
    json_pointer: Option<&str>,
    membership_id: Option<&str>,
) -> PreflightDiagnostic {
    diagnostic(
        code,
        "assistant-workflow/preflight",
        json_pointer,
        membership_id,
        None,
        None,
    )
}

fn diagnostic(
    code: &str,
    stage: &str,
    json_pointer: Option<&str>,
    membership_id: Option<&str>,
    actual: Option<u64>,
    limit: Option<u64>,
) -> PreflightDiagnostic {
    let code =
        WorkflowDiagnosticCode::from_wire(code).unwrap_or(WorkflowDiagnosticCode::WorkflowInvalid);
    let recovery = diagnostic_recovery(code);
    PreflightDiagnostic {
        code,
        stage: WorkflowDiagnosticStage::from_wire(stage)
            .unwrap_or(WorkflowDiagnosticStage::WorkflowCompile),
        path: json_pointer.map(str::to_owned),
        related_paths: Vec::new(),
        membership_id: membership_id
            .filter(|value| valid_diagnostic_identity(value))
            .map(str::to_owned),
        actual,
        limit,
        expected: None,
        actual_kind: None,
        recovery: Some(recovery),
        line: None,
        column: None,
    }
}

fn diagnostic_recovery(code: WorkflowDiagnosticCode) -> WorkflowDiagnosticRecovery {
    use WorkflowDiagnosticCode as Code;
    match code {
        Code::WorkflowRequiredFieldMissing => WorkflowDiagnosticRecovery::AddRequiredField,
        Code::WorkflowUnknownField => WorkflowDiagnosticRecovery::RemoveUnknownField,
        Code::WorkflowBindingDuplicate
        | Code::WorkflowStateDuplicate
        | Code::WorkflowTransitionDuplicate => WorkflowDiagnosticRecovery::RemoveDuplicate,
        Code::WorkflowRuntimeBindingInvalid
        | Code::WorkflowActorBindingInvalid
        | Code::WorkflowScriptRuntimeInvalid
        | Code::WorkflowWorksetReferenceInvalid
        | Code::WorkflowWorksetBindingInvalid
        | Code::WorkflowInitialUnknown
        | Code::WorkflowTransitionStateUnknown => WorkflowDiagnosticRecovery::CorrectReference,
        Code::WorkflowStateLimit
        | Code::WorkflowTransitionLimit
        | Code::WorkflowBindingLimit
        | Code::WorkflowRuntimeLimit
        | Code::WorkflowParallelismInvalid
        | Code::WorkflowWorksetLimitInvalid
        | Code::WorkflowRetryLimitInvalid => WorkflowDiagnosticRecovery::ReduceResource,
        Code::WorkflowGuardAmbiguous | Code::WorkflowRoutingInvalid => {
            WorkflowDiagnosticRecovery::CorrectRouting
        }
        Code::WorkflowTopologyInvalid
        | Code::WorkflowStateUnreachable
        | Code::WorkflowTerminalUnreachable
        | Code::WorkflowEffectCycle => WorkflowDiagnosticRecovery::CorrectTopology,
        Code::GraphAssistantMembershipRejected
        | Code::GraphAssistantDesignationRejected
        | Code::GraphAssistantSkillUnavailable
        | Code::GraphAuthorityRejected
        | Code::GraphModelUnavailable
        | Code::GraphModelRejected
        | Code::GraphReadinessRejected
        | Code::GraphEnvironmentUnavailable
        | Code::GraphProfileRejected => WorkflowDiagnosticRecovery::UpdateAssistantProfile,
        Code::GraphBindingDuplicate
        | Code::GraphBindingUnknown
        | Code::GraphBindingKindRejected
        | Code::GraphBindingIncomplete
        | Code::GraphMembershipRejected => WorkflowDiagnosticRecovery::UpdateBinding,
        Code::ConversationStateUnavailable | Code::ConversationNotFound | Code::GraphRouteStale => {
            WorkflowDiagnosticRecovery::RefreshConversationState
        }
        _ => WorkflowDiagnosticRecovery::CorrectField,
    }
}

fn valid_diagnostic_identity(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 256
        && value.chars().all(|character| {
            character.is_ascii_alphanumeric() || matches!(character, ':' | '.' | '_' | '-')
        })
}

pub(crate) fn sha256_hex(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    let digest = Sha256::digest(bytes);
    digest.iter().map(|byte| format!("{byte:02x}")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The selection-policy revision a test admits its fixture graph under.
    /// It is stated explicitly: the receipt must never carry a synthesized
    /// or current-reading revision.
    const FIXTURE_SELECTION_POLICY_REVISION: &str = "selection-policy-fixture-1";
    use licoup_workflow::{MAX_ACTIVE_EFFECTS, MAX_WORKSET_ITEMS};

    fn workflow() -> Value {
        json!({
            "schema": "licoup.adaptive-flywheel.workflow.v1",
            "metadata": {"id": "assistant-temporary", "name": "Temporary", "version": "1"},
            "limits": {"maxParallelism": 2, "maxWorksetItems": 16, "maxAttempts": 2},
            "actorSlots": [{"id": "actor", "kind": "actor", "label": "Actor", "required": true, "entry": true}],
            "runtimes": [],
            "worksets": [],
            "initial": "run",
            "states": [
                {"id": "run", "kind": "actor", "label": "Run", "binding": "actor"},
                {"id": "done", "kind": "succeed", "label": "Done"},
                {"id": "failed", "kind": "fail", "label": "Failed"}
            ],
            "transitions": [
                {"id": "done", "from": "run", "to": "done", "event": "success"},
                {"id": "failed", "from": "run", "to": "failed", "event": "failure"}
            ],
        })
    }

    fn snapshot(id: &str, responsibility: ProfileResponsibility) -> MembershipProfileSnapshot {
        MembershipProfileSnapshot {
            conversation_id: "conversation:g".to_owned(),
            membership_id: id.to_owned(),
            agent_id: "agent:a".to_owned(),
            intent_revision: 1,
            responsibility,
            required_capabilities: vec!["conversationDriver:supported".to_owned()],
            preferred_capabilities: Vec::new(),
            skill_references: if responsibility == ProfileResponsibility::Assistant {
                vec![LICOUP_GUIDE_SKILL_ID.to_owned()]
            } else {
                Vec::new()
            },
            preferred_model: None,
            preferred_reasoning_effort: None,
            preferred_environment: None,
            preferred_permission_mode: None,
            model: Some("model-a".to_owned()),
            capabilities: vec!["conversationDriver:supported".to_owned()],
            skills: if responsibility == ProfileResponsibility::Assistant {
                vec![LICOUP_GUIDE_SKILL_ID.to_owned()]
            } else {
                Vec::new()
            },
            environment: Some("local".to_owned()),
            readiness: Some("ready".to_owned()),
            price_input_usd_per_million_tokens: Some(1.0),
            price_output_usd_per_million_tokens: Some(2.0),
            intelligence_score: Some(8),
            task_tags: Vec::new(),
            reliability_class: Some("verified".to_owned()),
            latency_class: Some(1),
            authority: vec![
                "conversation.act".to_owned(),
                "conversation.read".to_owned(),
            ],
        }
    }

    #[test]
    fn exact_memberships_and_owner_derived_model_are_frozen() {
        let snapshots = vec![
            snapshot("membership:assistant", ProfileResponsibility::Assistant),
            snapshot("membership:actor", ProfileResponsibility::Member),
        ];
        let admitted = preflight_assistant_graph(
            "conversation:g",
            "membership:assistant",
            &workflow(),
            &[BindingValue {
                slot_id: "actor".to_owned(),
                ordinal: 0,
                value_id: "membership:actor".to_owned(),
                model: String::new(),
                reasoning_effort: String::new(),
                revision: 0,
            }],
            &snapshots,
            &CandidateFilters::default(),
            crate::ports::host_ports().model.as_ref(),
            FIXTURE_SELECTION_POLICY_REVISION,
        )
        .unwrap();
        assert_eq!(admitted.bindings[0].value_id, "membership:actor");
        assert_eq!(admitted.bindings[0].model, "model-a");
        assert_eq!(admitted.receipt.membership_ids, vec!["membership:actor"]);
    }

    #[test]
    fn missing_skill_and_runtime_assets_fail_before_admission() {
        let mut assistant = snapshot("membership:assistant", ProfileResponsibility::Assistant);
        assistant.skills.clear();
        let mut script = workflow();
        script["actorSlots"].as_array_mut().unwrap().push(json!({
            "id": "node",
            "kind": "runtime",
            "label": "Node",
            "required": true
        }));
        script["runtimes"] = json!([{"id": "node", "kind": "node"}]);
        let failure = preflight_assistant_graph(
            "conversation:g",
            "membership:assistant",
            &script,
            &[],
            &[assistant],
            &CandidateFilters::default(),
            crate::ports::host_ports().model.as_ref(),
            FIXTURE_SELECTION_POLICY_REVISION,
        )
        .unwrap_err();
        assert!(
            failure
                .diagnostics
                .iter()
                .any(|check| check.code == "graph_assistant_skill_unavailable")
        );
        assert!(
            failure
                .diagnostics
                .iter()
                .any(|check| check.code == "graph_runtime_asset_unavailable")
        );
    }

    #[test]
    fn resource_and_identity_diagnostics_preserve_safe_repair_locations() {
        let mut invalid = workflow();
        invalid["metadata"]["id"] = json!("imported-workflow");
        invalid["limits"]["maxParallelism"] = json!(0);
        invalid["limits"]["maxWorksetItems"] = json!(MAX_WORKSET_ITEMS + 1);
        let failure = preflight_assistant_graph(
            "conversation:g",
            "membership:assistant",
            &invalid,
            &[],
            &[],
            &CandidateFilters::default(),
            crate::ports::host_ports().model.as_ref(),
            FIXTURE_SELECTION_POLICY_REVISION,
        )
        .unwrap_err();
        assert_eq!(failure.code, "graph_preflight_rejected");
        assert!(failure.diagnostics.iter().any(|diagnostic| {
            diagnostic.code == "graph_identity_not_assistant_temporary"
                && diagnostic.path.as_deref() == Some("/metadata/id")
        }));
        assert!(failure.diagnostics.iter().any(|diagnostic| {
            diagnostic.code == "workflow_parallelism_invalid"
                && diagnostic.path.as_deref() == Some("/limits/maxParallelism")
                && diagnostic.actual == Some(0)
                && diagnostic.limit == Some(MAX_ACTIVE_EFFECTS as u64)
        }));
        assert!(failure.diagnostics.iter().any(|diagnostic| {
            diagnostic.code == "workflow_workset_limit_invalid"
                && diagnostic.path.as_deref() == Some("/limits/maxWorksetItems")
                && diagnostic.actual == Some((MAX_WORKSET_ITEMS + 1) as u64)
                && diagnostic.limit == Some(MAX_WORKSET_ITEMS as u64)
        }));
    }

    #[test]
    fn binding_diagnostics_identify_the_exact_request_item() {
        let snapshots = vec![snapshot(
            "membership:assistant",
            ProfileResponsibility::Assistant,
        )];
        let failure = preflight_assistant_graph(
            "conversation:g",
            "membership:assistant",
            &workflow(),
            &[BindingValue {
                slot_id: "actor".to_owned(),
                ordinal: 0,
                value_id: "membership:missing".to_owned(),
                model: String::new(),
                reasoning_effort: String::new(),
                revision: 0,
            }],
            &snapshots,
            &CandidateFilters::default(),
            crate::ports::host_ports().model.as_ref(),
            FIXTURE_SELECTION_POLICY_REVISION,
        )
        .unwrap_err();
        assert!(failure.diagnostics.iter().any(|diagnostic| {
            diagnostic.code == "graph_membership_rejected"
                && diagnostic.path.as_deref() == Some("/bindings/0/valueId")
                && diagnostic.membership_id.as_deref() == Some("membership:missing")
        }));
    }
}
