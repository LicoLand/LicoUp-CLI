use super::*;
use licoup_agent_runtime::{
    AdapterFailure, CallerRegistrationPlan, CallerRegistrationReceipt, DurableNativeBinding,
    ExecutionAdmissionEvidence, McpCallerIntegration, NativeResumeIdentity, ReadinessEvidence,
    RegistrationApproval, RuntimeDispatchReceipt, RuntimeObservation, RuntimeTransition,
    SubagentCapabilities, SubagentContinueRequest, SubagentDispatchRequest, SubagentRuntimeAdapter,
};
use licoup_conversation::{
    ConversationRuntimeScope, ConversationStore, DispatchState, MembershipAccess, Principal,
    PrincipalKind, SubagentDispatchClaimState,
};
use std::collections::BTreeMap;
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex};

#[test]
fn native_operation_catalog_is_closed_and_ordered() {
    let catalog = tool_catalog();
    assert_eq!(
        catalog
            .iter()
            .filter_map(|tool| tool.get("name").and_then(Value::as_str))
            .collect::<Vec<_>>(),
        TOOL_NAMES
    );
    assert!(catalog.iter().all(|tool| {
        tool.pointer("/inputSchema/additionalProperties") == Some(&Value::Bool(false))
    }));
}

#[test]
fn validation_is_closed_and_bounds_effect_arguments() {
    let valid = json!({
        "conversationId": "conversation:fixture",
        "membershipId": "membership:fixture",
        "prompt": "bounded task",
        "timeoutMs": 0,
    });
    assert!(validate_tool_arguments(
        "lico_subagent_delegate",
        valid.as_object().unwrap()
    ));
    let mut unknown = valid.as_object().unwrap().clone();
    unknown.insert("provider".into(), json!("codex"));
    assert!(!validate_tool_arguments("lico_subagent_delegate", &unknown));
    let oversized = json!({
        "conversationId": "conversation:fixture",
        "membershipId": "membership:fixture",
        "prompt": "x".repeat(MAX_PROMPT_BYTES + 1),
    });
    assert!(!validate_tool_arguments(
        "lico_subagent_delegate",
        oversized.as_object().unwrap()
    ));
    let whitespace_prompt = json!({
        "conversationId": "conversation:fixture",
        "membershipId": "membership:fixture",
        "prompt": "   ",
    });
    assert!(!validate_tool_arguments(
        "lico_subagent_delegate",
        whitespace_prompt.as_object().unwrap()
    ));
    assert!(validate_tool_arguments(
        "lico_subagent_delegate",
        json!({
            "agent": "cursor",
            "prompt": "bounded task",
        })
        .as_object()
        .unwrap()
    ));
    assert!(validate_tool_arguments(
        "lico_subagent_delegate",
        json!({
            "prompt": "bounded task",
        })
        .as_object()
        .unwrap()
    ));
    assert!(validate_tool_arguments(
        "lico_subagent_cancel",
        json!({
            "agent": "cursor",
        })
        .as_object()
        .unwrap()
    ));
    assert!(validate_tool_arguments(
        "lico_subagent_delegate",
        json!({
            "prompt": "bounded task",
            "timeoutMs": 0,
            "timeoutUnbounded": true,
            "taskType": "frontend",
        })
        .as_object()
        .unwrap()
    ));
}

#[test]
fn workflow_execute_accepts_optional_callback_decision_fields() {
    let valid = json!({
        "conversationId": "conversation:fixture",
        "membershipId": "membership:fixture",
        "workflow": {"schema": "licoup.adaptive-flywheel.workflow.v1"},
        "bindings": [],
        "idempotencyKey": "key-1",
        "decision": "advance",
        "callbackStateId": "work",
        "callbackStateVisit": 1,
    });
    assert!(validate_tool_arguments(
        "lico_assistant_workflow_execute",
        valid.as_object().unwrap()
    ));
    // The schema surface declares the decision vocabulary for discovery...
    let catalog = tool_catalog();
    let execute = catalog
        .iter()
        .find(|tool| {
            tool.get("name").and_then(Value::as_str) == Some("lico_assistant_workflow_execute")
        })
        .unwrap();
    let properties = execute.pointer("/inputSchema/properties").unwrap();
    assert_eq!(
        properties.pointer("/decision/enum"),
        Some(&json!(["advance", "return", "terminate"]))
    );
    assert!(properties.get("callbackStateId").is_some());
    assert!(properties.get("callbackStateVisit").is_some());
    // ...while the required list and the closed shape stay untouched.
    assert_eq!(
        execute.pointer("/inputSchema/required"),
        Some(&json!([
            "conversationId",
            "membershipId",
            "workflow",
            "bindings",
            "idempotencyKey"
        ]))
    );
    assert_eq!(
        execute.pointer("/inputSchema/additionalProperties"),
        Some(&Value::Bool(false))
    );
    // Value enforcement stays in the domain: the generic validator only
    // enforces type and bounds.
    let mut out_of_vocabulary = valid.as_object().unwrap().clone();
    out_of_vocabulary.insert("decision".into(), json!("warp"));
    assert!(validate_tool_arguments(
        "lico_assistant_workflow_execute",
        &out_of_vocabulary
    ));
    let mut zero_visit = valid.as_object().unwrap().clone();
    zero_visit.insert("callbackStateVisit".into(), json!(0));
    assert!(!validate_tool_arguments(
        "lico_assistant_workflow_execute",
        &zero_visit
    ));
    let mut typed_wrong = valid.as_object().unwrap().clone();
    typed_wrong.insert("callbackStateVisit".into(), json!("1"));
    assert!(!validate_tool_arguments(
        "lico_assistant_workflow_execute",
        &typed_wrong
    ));
    let mut unknown = valid.as_object().unwrap().clone();
    unknown.insert("verdict".into(), json!("advance"));
    assert!(!validate_tool_arguments(
        "lico_assistant_workflow_execute",
        &unknown
    ));
}

#[test]
fn caller_effect_scope_is_exact_membership_and_conversation_bound() {
    let caller = CallerContext {
        provider_id: ProviderId::parse("codex").unwrap(),
        conversation_id: Some("conversation:one".into()),
        membership_id: Some("membership:caller".into()),
        parent_dispatch_id: None,
        authenticated: true,
    };
    assert_eq!(
        caller.effect_scope("conversation:one").unwrap(),
        "membership:caller"
    );
    assert_eq!(
        caller.effect_scope("conversation:two").unwrap_err().code,
        "subagent_cross_conversation_rejected"
    );
}

struct FixtureCaller {
    provider: ProviderId,
}

impl McpCallerIntegration for FixtureCaller {
    fn provider_id(&self) -> &ProviderId {
        &self.provider
    }
    fn plan_registration(&self) -> Result<CallerRegistrationPlan, AdapterFailure> {
        unreachable!()
    }
    fn readiness(&self) -> ReadinessEvidence {
        ready(&self.provider)
    }
    fn apply_registration(
        &self,
        _: &CallerRegistrationPlan,
        _: &mut RegistrationApproval,
    ) -> Result<CallerRegistrationReceipt, AdapterFailure> {
        unreachable!()
    }
    fn remove_registration(
        &self,
        _: &CallerRegistrationPlan,
        _: &mut RegistrationApproval,
    ) -> Result<(), AdapterFailure> {
        unreachable!()
    }
}

struct FixtureRuntime {
    provider: ProviderId,
    calls: Arc<Mutex<Vec<String>>>,
    persistent_store: Option<ConversationStore>,
    execution_admitted: bool,
    capability_available: bool,
    readiness: ReadinessEvidence,
    send_failure: Option<AdapterFailure>,
}

impl FixtureRuntime {
    fn with_send_failure(mut self, failure: Option<AdapterFailure>) -> Self {
        self.send_failure = failure;
        self
    }
}

impl SubagentRuntimeAdapter for FixtureRuntime {
    fn provider_id(&self) -> &ProviderId {
        &self.provider
    }
    fn capabilities(&self) -> SubagentCapabilities {
        SubagentCapabilities {
            create: self.capability_available,
            exact_resume: true,
            observe: true,
            continue_turn: true,
            active_cancel: true,
            native_steer: self.provider.as_str() == "codex",
            instruction_policy: if self.provider.as_str() == "codex" {
                licoup_agent_runtime::InstructionPolicy::NativeDeveloperInstructions
            } else {
                licoup_agent_runtime::InstructionPolicy::OrdinaryWirePrefix
            },
        }
    }
    fn execution_admission(&self) -> ExecutionAdmissionEvidence {
        ExecutionAdmissionEvidence {
            provider_id: self.provider.as_str().into(),
            installed: self.execution_admitted,
            executable_message_send_route: self.execution_admitted,
        }
    }
    fn readiness(&self) -> ReadinessEvidence {
        self.readiness.clone()
    }
    fn resolve_resume_identity(
        &self,
        durable: &DurableNativeBinding,
    ) -> Result<NativeResumeIdentity, AdapterFailure> {
        NativeResumeIdentity::from_durable(durable, durable.native_session_id())
    }
    fn send(
        &self,
        request: &SubagentDispatchRequest,
    ) -> Result<RuntimeDispatchReceipt, AdapterFailure> {
        if let Some(error) = &self.send_failure {
            self.calls.lock().unwrap().push(request.dispatch_id.clone());
            return Err(error.clone());
        }
        if let Some(store) = &self.persistent_store {
            store
                .prepare_runtime_dispatch(
                    self.provider.as_str(),
                    "",
                    &request.prompt,
                    Some(&request.conversation_id),
                    Some(&request.target_membership_id),
                    None,
                    Some(&request.dispatch_id),
                )
                .map_err(|_| AdapterFailure::permanent("fixture_dispatch_failed", "fixture"))?;
        }
        self.calls.lock().unwrap().push(request.dispatch_id.clone());
        Ok(RuntimeDispatchReceipt {
            dispatch_id: request.dispatch_id.clone(),
            transition: RuntimeTransition::Accepted,
            identity: None,
        })
    }
    fn continue_turn(
        &self,
        request: &SubagentContinueRequest,
    ) -> Result<RuntimeDispatchReceipt, AdapterFailure> {
        self.calls
            .lock()
            .unwrap()
            .push(request.dispatch.dispatch_id.clone());
        Ok(RuntimeDispatchReceipt {
            dispatch_id: request.dispatch.dispatch_id.clone(),
            transition: RuntimeTransition::Accepted,
            identity: None,
        })
    }
    fn observe(&self, dispatch_id: &str) -> Result<RuntimeObservation, AdapterFailure> {
        Ok(RuntimeObservation {
            dispatch_id: dispatch_id.into(),
            transition: RuntimeTransition::Processing,
            parts: vec![],
        })
    }
    fn cancel_active(
        &self,
        dispatch_id: &str,
        _: &NativeResumeIdentity,
    ) -> Result<RuntimeDispatchReceipt, AdapterFailure> {
        Ok(RuntimeDispatchReceipt {
            dispatch_id: dispatch_id.into(),
            transition: RuntimeTransition::CancelRequested,
            identity: None,
        })
    }
    fn cleanup(&self, _: &NativeResumeIdentity) -> Result<(), AdapterFailure> {
        Ok(())
    }
}

fn ready(provider: &ProviderId) -> ReadinessEvidence {
    ReadinessEvidence {
        provider_id: provider.as_str().into(),
        installed: true,
        identity_verified: true,
        transport_ready: true,
        permission_ready: true,
        capability_revision: format!("fixture:{}:1", provider),
        blocker_code: None,
    }
}

fn unverified(provider: &ProviderId) -> ReadinessEvidence {
    ReadinessEvidence {
        provider_id: provider.as_str().into(),
        installed: true,
        identity_verified: false,
        transport_ready: false,
        permission_ready: false,
        capability_revision: format!("fixture:{}:unverified", provider),
        blocker_code: Some("provider_readiness_evidence_incomplete".into()),
    }
}

fn fixture_runtime(
    provider: ProviderId,
    calls: Arc<Mutex<Vec<String>>>,
    persistent_store: Option<ConversationStore>,
) -> FixtureRuntime {
    FixtureRuntime {
        readiness: ready(&provider),
        provider,
        calls,
        persistent_store,
        execution_admitted: true,
        capability_available: true,
        send_failure: None,
    }
}

struct FixtureHost {
    store: ConversationStore,
    providers: BTreeMap<String, ProviderId>,
    dynamic_providers: Mutex<BTreeMap<String, ProviderId>>,
    working_directories: Mutex<BTreeMap<String, String>>,
    runtime_session_ids: Mutex<BTreeMap<String, String>>,
    profiles: Mutex<BTreeMap<String, (Option<String>, Option<String>, Option<String>)>>,
    simulate_profile_failure: AtomicBool,
    simulate_cwd_failure: AtomicBool,
}

impl FixtureHost {
    fn new(store: ConversationStore, providers: BTreeMap<String, ProviderId>) -> Self {
        Self {
            store,
            providers,
            dynamic_providers: Mutex::new(BTreeMap::new()),
            working_directories: Mutex::new(BTreeMap::new()),
            runtime_session_ids: Mutex::new(BTreeMap::new()),
            profiles: Mutex::new(BTreeMap::new()),
            simulate_profile_failure: AtomicBool::new(false),
            simulate_cwd_failure: AtomicBool::new(false),
        }
    }
}

impl ConversationHostPort for FixtureHost {
    fn verify_caller(
        &self,
        caller: &CallerContext,
        conversation_id: &str,
    ) -> Result<(), SubagentError> {
        let membership = caller.effect_scope(conversation_id)?;
        let valid = self.providers.get(membership) == Some(&caller.provider_id)
            || self.dynamic_providers.lock().unwrap().get(membership) == Some(&caller.provider_id);
        if valid {
            Ok(())
        } else {
            Err(permanent("caller_identity_mismatch", "fixture"))
        }
    }
    fn assistant_profiles(
        &self,
        _: &CallerContext,
        _: &Map<String, Value>,
    ) -> Result<Value, SubagentError> {
        Ok(json!({}))
    }
    fn assistant_workflow(
        &self,
        _: &CallerContext,
        _: &str,
        _: &Map<String, Value>,
    ) -> Result<Value, SubagentError> {
        Ok(json!({}))
    }
    fn target_membership(
        &self,
        conversation_id: &str,
        membership_id: &str,
    ) -> Result<TargetMembership, SubagentError> {
        let provider_id = self
            .providers
            .get(membership_id)
            .cloned()
            .or_else(|| {
                self.dynamic_providers
                    .lock()
                    .unwrap()
                    .get(membership_id)
                    .cloned()
            })
            .ok_or_else(|| permanent("membership_not_found", "fixture"))?;
        let (preferred_model, preferred_reasoning_effort, preferred_permission_mode) = self
            .profiles
            .lock()
            .unwrap()
            .get(membership_id)
            .cloned()
            .unwrap_or((None, None, None));
        let working_directory = self
            .working_directories
            .lock()
            .unwrap()
            .get(membership_id)
            .cloned();
        Ok(TargetMembership {
            conversation_id: conversation_id.into(),
            membership_id: membership_id.into(),
            provider_id,
            preferred_model,
            preferred_reasoning_effort,
            preferred_permission_mode,
            working_directory,
        })
    }
    fn target_membership_by_agent(
        &self,
        conversation_id: &str,
        agent: &str,
    ) -> Result<TargetMembership, SubagentError> {
        let mut matches = self
            .providers
            .iter()
            .filter(|(_, provider)| provider.as_str() == agent)
            .map(|(membership_id, _)| membership_id.clone())
            .collect::<Vec<_>>();
        let dynamic_matches = self
            .dynamic_providers
            .lock()
            .unwrap()
            .iter()
            .filter(|(_, provider)| provider.as_str() == agent)
            .map(|(membership_id, _)| membership_id.clone())
            .collect::<Vec<_>>();
        matches.extend(dynamic_matches);
        match matches.as_slice() {
            [membership_id] => self.target_membership(conversation_id, membership_id),
            _ => Err(SubagentError::retryable(
                "subagent_target_seat_missing",
                "conversation/authorize",
            )),
        }
    }
    fn admit_subagent(
        &self,
        request: &SubagentAdmissionRequest,
    ) -> Result<TargetMembership, SubagentError> {
        request.validate()?;
        let parsed_provider = ProviderId::parse(request.agent_id.clone())
            .map_err(|_| permanent("subagent_target_invalid", "conversation/authorize"))?;

        let principal = Principal {
            id: format!("agent:{}", request.agent_id),
            kind: PrincipalKind::Agent,
            display_name: request.agent_id.clone(),
            agent_id: Some(request.agent_id.clone()),
            created_at_unix_ms: 1,
        };
        let member = self
            .store
            .add_member(
                &request.conversation_id,
                principal,
                MembershipAccess::Member,
            )
            .map_err(|_| permanent("subagent_admission_failed", "store/admit"))?;
        let membership_id = member.id.clone();

        // Step 1: Provisional member seat admission
        self.dynamic_providers
            .lock()
            .unwrap()
            .insert(membership_id.clone(), parsed_provider);

        let rollback = |id: &str| {
            let _ = self.store.leave_member(&request.conversation_id, id);
            self.dynamic_providers.lock().unwrap().remove(id);
            self.profiles.lock().unwrap().remove(id);
            self.working_directories.lock().unwrap().remove(id);
        };

        // Step 2: Profile binding
        if self
            .simulate_profile_failure
            .load(std::sync::atomic::Ordering::SeqCst)
        {
            rollback(&membership_id);
            return Err(permanent(
                "subagent_profile_binding_failed",
                "admission/profile",
            ));
        }
        self.profiles.lock().unwrap().insert(
            membership_id.clone(),
            (
                request.preferred_model.clone(),
                request.preferred_reasoning_effort.clone(),
                request.preferred_permission_mode.clone(),
            ),
        );

        // Step 3: Cwd reservation
        if self
            .simulate_cwd_failure
            .load(std::sync::atomic::Ordering::SeqCst)
        {
            rollback(&membership_id);
            return Err(permanent(
                "subagent_cwd_binding_failed",
                "admission/workspace",
            ));
        }
        if let Some(cwd) = &request.working_directory {
            self.working_directories
                .lock()
                .unwrap()
                .insert(membership_id.clone(), cwd.clone());
        }

        self.target_membership(&request.conversation_id, &membership_id)
    }
    fn claim_dispatch(
        &self,
        conversation_id: &str,
        caller: &str,
        target: &str,
        parent: Option<&str>,
    ) -> Result<SubagentDispatchClaim, SubagentError> {
        self.store
            .claim_subagent_dispatch(conversation_id, caller, target, parent)
            .map_err(project_fixture_store)
    }
    fn update_claim(
        &self,
        dispatch_id: &str,
        state: SubagentDispatchClaimState,
    ) -> Result<(), SubagentError> {
        self.store
            .update_subagent_claim_state(dispatch_id, state)
            .map_err(|_| permanent("transition_failed", "fixture"))
    }
    fn active_claim(
        &self,
        conversation_id: &str,
        caller: &str,
        target: &str,
    ) -> Result<Option<SubagentDispatchClaim>, SubagentError> {
        self.store
            .active_subagent_claim(conversation_id, caller, target)
            .map_err(|_| permanent("claim_failed", "fixture"))
    }
    fn record_inbound(
        &self,
        conversation_id: &str,
        caller: Option<&str>,
        target: Option<&str>,
        tool: &str,
        outcome: &str,
    ) -> Result<(), SubagentError> {
        self.store
            .record_subagent_mcp_inbound(conversation_id, caller, target, tool, outcome)
            .map_err(|_| permanent("inbound_failed", "fixture"))
    }
    fn bind_resume_session(
        &self,
        _conversation_id: &str,
        membership_id: &str,
        runtime_session_id: Option<&str>,
        working_directory: Option<&str>,
    ) -> Result<(), SubagentError> {
        if let Some(session_id) = runtime_session_id {
            self.runtime_session_ids
                .lock()
                .unwrap()
                .insert(membership_id.to_owned(), session_id.to_owned());
        }
        if let Some(cwd) = working_directory {
            self.working_directories
                .lock()
                .unwrap()
                .insert(membership_id.to_owned(), cwd.to_owned());
        }
        Ok(())
    }
    fn latest_resume_binding(
        &self,
        _: &str,
        membership_id: &str,
    ) -> Result<DurableNativeBinding, SubagentError> {
        let provider = self
            .providers
            .get(membership_id)
            .cloned()
            .or_else(|| {
                self.dynamic_providers
                    .lock()
                    .unwrap()
                    .get(membership_id)
                    .cloned()
            })
            .ok_or_else(|| permanent("membership_not_found", "fixture"))?;
        let cwd = self
            .working_directories
            .lock()
            .unwrap()
            .get(membership_id)
            .cloned();
        let session_id = self
            .runtime_session_ids
            .lock()
            .unwrap()
            .get(membership_id)
            .cloned()
            .unwrap_or_else(|| format!("native-{membership_id}"));
        DurableNativeBinding::new(provider, session_id, None, cwd).map_err(project_adapter_failure)
    }
}

struct FixtureTargets;
impl ReadOnlyTargetPort for FixtureTargets {
    fn list(&self) -> Result<Value, SubagentError> {
        Ok(json!({"count":3}))
    }
    fn probe(&self, provider: &ProviderId) -> Result<Value, SubagentError> {
        if provider.as_str() == "unknown" {
            return Err(permanent("subagent_unavailable", "target/probe"));
        }
        Ok(json!({"agentId":provider.as_str()}))
    }
}

/// A Conversation host whose Assistant workflow lane is the production
/// strategy service over one shared portable root, so an MCP
/// `lico_assistant_workflow_execute` call drives a real Assistant-temporary
/// run end to end.
struct StrategyFixtureHost {
    inner: FixtureHost,
    strategy: crate::domain::workflow_runtime::StrategyService,
}

impl ConversationHostPort for StrategyFixtureHost {
    fn verify_caller(
        &self,
        caller: &CallerContext,
        conversation_id: &str,
    ) -> Result<(), SubagentError> {
        self.inner.verify_caller(caller, conversation_id)
    }
    fn assistant_profiles(
        &self,
        caller: &CallerContext,
        arguments: &Map<String, Value>,
    ) -> Result<Value, SubagentError> {
        self.inner.assistant_profiles(caller, arguments)
    }
    fn assistant_workflow(
        &self,
        _: &CallerContext,
        action: &str,
        arguments: &Map<String, Value>,
    ) -> Result<Value, SubagentError> {
        let mut request = arguments.clone();
        request.insert("action".into(), json!(action));
        // Admitting an execution captures the selection-policy revision the
        // decision is admitted under, exactly as `production::assistant_workflow`
        // captures it for this action alone: the runtime requires it and never
        // defaults it, because a receipt that cannot name its policy revision
        // would let provenance degrade silently. An inspection carries no
        // decision and admits no such field.
        if action == "strategy.assistant.workflow.execute" {
            request.insert(
                "selectionPolicyRevision".into(),
                json!(
                    crate::domain::client_conversation::selection_policy::current_binding()
                        .revision_name()
                ),
            );
        }
        self.strategy
            .execute(Value::Object(request))
            .map_err(|_| permanent("assistant_workflow_unavailable", "workflow/execute"))
    }
    fn target_membership(
        &self,
        conversation_id: &str,
        membership_id: &str,
    ) -> Result<TargetMembership, SubagentError> {
        self.inner.target_membership(conversation_id, membership_id)
    }
    fn target_membership_by_agent(
        &self,
        conversation_id: &str,
        agent: &str,
    ) -> Result<TargetMembership, SubagentError> {
        self.inner
            .target_membership_by_agent(conversation_id, agent)
    }
    fn admit_subagent(
        &self,
        request: &SubagentAdmissionRequest,
    ) -> Result<TargetMembership, SubagentError> {
        self.inner.admit_subagent(request)
    }
    fn claim_dispatch(
        &self,
        conversation_id: &str,
        caller_membership_id: &str,
        target_membership_id: &str,
        parent_dispatch_id: Option<&str>,
    ) -> Result<SubagentDispatchClaim, SubagentError> {
        self.inner.claim_dispatch(
            conversation_id,
            caller_membership_id,
            target_membership_id,
            parent_dispatch_id,
        )
    }
    fn update_claim(
        &self,
        dispatch_id: &str,
        state: SubagentDispatchClaimState,
    ) -> Result<(), SubagentError> {
        self.inner.update_claim(dispatch_id, state)
    }
    fn active_claim(
        &self,
        conversation_id: &str,
        caller_membership_id: &str,
        target_membership_id: &str,
    ) -> Result<Option<SubagentDispatchClaim>, SubagentError> {
        self.inner
            .active_claim(conversation_id, caller_membership_id, target_membership_id)
    }
    fn bind_resume_session(
        &self,
        conversation_id: &str,
        membership_id: &str,
        runtime_session_id: Option<&str>,
        working_directory: Option<&str>,
    ) -> Result<(), SubagentError> {
        self.inner.bind_resume_session(
            conversation_id,
            membership_id,
            runtime_session_id,
            working_directory,
        )
    }
    fn latest_resume_binding(
        &self,
        conversation_id: &str,
        membership_id: &str,
    ) -> Result<DurableNativeBinding, SubagentError> {
        self.inner
            .latest_resume_binding(conversation_id, membership_id)
    }
    fn record_inbound(
        &self,
        conversation_id: &str,
        caller_membership_id: Option<&str>,
        target_membership_id: Option<&str>,
        tool: &str,
        outcome: &str,
    ) -> Result<(), SubagentError> {
        self.inner.record_inbound(
            conversation_id,
            caller_membership_id,
            target_membership_id,
            tool,
            outcome,
        )
    }
}

struct ReadyProfiles;

impl crate::domain::client_conversation::ProfileSnapshotAuthority for ReadyProfiles {
    fn target_facts(
        &mut self,
        _agent_id: &str,
    ) -> Option<crate::domain::client_conversation::TargetFacts> {
        Some(crate::domain::client_conversation::TargetFacts {
            status: Some("available".to_owned()),
            model: Some("model-a".to_owned()),
            environment: Some("local".to_owned()),
            capabilities: vec!["conversationDriver:supported".to_owned()],
            readiness: Some("ready".to_owned()),
            reliability_class: Some("verified".to_owned()),
            latency_class: Some(1),
        })
    }
    fn model_price_usd_per_million_tokens(
        &mut self,
        _model: &str,
    ) -> Option<crate::domain::client_conversation::PriceFacts> {
        Some(crate::domain::client_conversation::PriceFacts {
            input: 1.0,
            output: 2.0,
        })
    }
    fn coding_score(&mut self, _agent_id: &str, _model: &str) -> Option<i64> {
        Some(3)
    }
    fn skill_names(&mut self, _agent_id: &str) -> Vec<String> {
        vec![crate::domain::client_conversation::LICOUP_GUIDE_SKILL_ID.to_owned()]
    }
}

fn project_fixture_store(error: anyhow::Error) -> SubagentError {
    match error.to_string().split(':').next().unwrap_or("") {
        "subagent_self_call_rejected" => permanent("subagent_self_call_rejected", "lineage/admit"),
        "subagent_duplicate_active_edge" => {
            permanent("subagent_duplicate_active_edge", "lineage/admit")
        }
        "subagent_caller_membership_inactive" => permanent(
            "subagent_caller_membership_inactive",
            "conversation/authorize",
        ),
        "subagent_target_membership_inactive" => permanent(
            "subagent_target_membership_inactive",
            "conversation/authorize",
        ),
        "subagent_parent_dispatch_unavailable" => {
            permanent("subagent_parent_dispatch_unavailable", "lineage/admit")
        }
        "subagent_cross_conversation_rejected" => {
            permanent("subagent_cross_conversation_rejected", "lineage/admit")
        }
        "subagent_lineage_caller_mismatch" => {
            permanent("subagent_lineage_caller_mismatch", "lineage/admit")
        }
        "subagent_repeated_ancestor" | "subagent_lineage_cycle" => {
            permanent("subagent_lineage_cycle", "lineage/admit")
        }
        "subagent_depth_exceeded" => permanent("subagent_depth_exceeded", "lineage/admit"),
        "subagent_dispatch_transition_invalid" => permanent(
            "subagent_dispatch_transition_invalid",
            "dispatch/transition",
        ),
        _ => permanent("claim_failed", "fixture"),
    }
}

fn invoke(
    app: &SubagentApplication,
    caller: &CallerContext,
    name: &str,
    arguments: Value,
) -> Result<Value, SubagentError> {
    app.call_tool(
        SubagentCallContext {
            caller,
            cancelled: Arc::new(AtomicBool::new(false)),
        },
        name,
        arguments.as_object().unwrap(),
    )
}

#[test]
fn dispatch_request_inherits_target_profile_and_preserves_explicit_overrides() {
    let target = TargetMembership {
        conversation_id: "conversation:fixture".into(),
        membership_id: "membership:target".into(),
        provider_id: ProviderId::parse("cursor").unwrap(),
        preferred_model: Some("profile-model".into()),
        preferred_reasoning_effort: Some("profile-effort".into()),
        preferred_permission_mode: Some("profile-permission".into()),
        working_directory: None,
    };
    let claim = SubagentDispatchClaim {
        id: "subagent:fixture".into(),
        conversation_id: target.conversation_id.clone(),
        caller_membership_id: "membership:caller".into(),
        target_membership_id: target.membership_id.clone(),
        parent_dispatch_id: None,
        depth: 1,
        state: SubagentDispatchClaimState::Claimed,
        created_at_unix_ms: 1,
        updated_at_unix_ms: 1,
    };

    let inherited = dispatch_request(
        json!({"prompt":"bounded task"}).as_object().unwrap(),
        "membership:caller",
        &target,
        &claim,
        None,
    )
    .unwrap();
    assert_eq!(inherited.model.as_deref(), Some("profile-model"));
    assert_eq!(
        inherited.reasoning_effort.as_deref(),
        Some("profile-effort")
    );
    assert_eq!(
        inherited.permission_mode.as_deref(),
        Some("profile-permission")
    );
    assert!(
        inherited
            .generated_guidance
            .as_deref()
            .is_some_and(|guidance| guidance.contains("report the outcome back"))
    );
    assert!(!inherited.timeout_unbounded);
    assert!(inherited.task_type.is_none());

    let overridden = dispatch_request(
        json!({
            "prompt":"bounded task",
            "model":"request-model",
            "reasoningEffort":"request-effort",
            "permissionMode":"request-permission"
        })
        .as_object()
        .unwrap(),
        "membership:caller",
        &target,
        &claim,
        None,
    )
    .unwrap();
    assert_eq!(overridden.model.as_deref(), Some("request-model"));
    assert_eq!(
        overridden.reasoning_effort.as_deref(),
        Some("request-effort")
    );
    assert_eq!(
        overridden.permission_mode.as_deref(),
        Some("request-permission")
    );

    let unbounded = dispatch_request(
        json!({
            "prompt":"bounded task",
            "timeoutMs": 0,
            "timeoutUnbounded": true,
            "taskType": "frontend"
        })
        .as_object()
        .unwrap(),
        "membership:caller",
        &target,
        &claim,
        None,
    )
    .unwrap();
    assert!(unbounded.timeout_unbounded);
    assert_eq!(unbounded.timeout_ms, Some(0));
    assert_eq!(unbounded.task_type.as_deref(), Some("frontend"));
}

#[test]
fn impossible_runtime_admission_stops_before_target_effect() {
    let calls = Arc::new(Mutex::new(Vec::new()));
    let provider = ProviderId::parse("cursor").unwrap();
    let target = TargetMembership {
        conversation_id: "conversation:fixture".into(),
        membership_id: "membership:target".into(),
        provider_id: provider.clone(),
        preferred_model: None,
        preferred_reasoning_effort: None,
        preferred_permission_mode: None,
        working_directory: None,
    };
    let app_with = |runtime: FixtureRuntime| {
        let mut registry = AdapterRegistry::empty();
        registry
            .register_pair(
                Arc::new(FixtureCaller {
                    provider: provider.clone(),
                }),
                Arc::new(runtime),
            )
            .unwrap();
        SubagentApplication::new(
            Arc::new(FixtureHost::new(
                ConversationStore::open_in_memory().unwrap(),
                BTreeMap::new(),
            )),
            registry,
            Arc::new(FixtureTargets),
        )
    };

    let mut missing_route = fixture_runtime(provider.clone(), Arc::clone(&calls), None);
    missing_route.execution_admitted = false;
    assert_eq!(
        app_with(missing_route)
            .runtime(&target, Operation::Delegate)
            .err()
            .unwrap()
            .code,
        "provider_not_installed"
    );

    let mut missing_capability = fixture_runtime(provider.clone(), Arc::clone(&calls), None);
    missing_capability.capability_available = false;
    assert_eq!(
        app_with(missing_capability)
            .runtime(&target, Operation::Delegate)
            .err()
            .unwrap()
            .code,
        "subagent_capability_unavailable"
    );

    let empty = SubagentApplication::new(
        Arc::new(FixtureHost::new(
            ConversationStore::open_in_memory().unwrap(),
            BTreeMap::new(),
        )),
        AdapterRegistry::empty(),
        Arc::new(FixtureTargets),
    );
    assert_eq!(
        empty
            .runtime(&target, Operation::Delegate)
            .err()
            .unwrap()
            .code,
        "subagent_adapter_unavailable"
    );

    let admitted = app_with(fixture_runtime(provider.clone(), Arc::clone(&calls), None));
    let unauthenticated = CallerContext {
        provider_id: provider,
        conversation_id: Some(target.conversation_id.clone()),
        membership_id: Some("membership:caller".into()),
        parent_dispatch_id: None,
        authenticated: false,
    };
    assert_eq!(
        invoke(
            &admitted,
            &unauthenticated,
            "lico_subagent_delegate",
            json!({
                "conversationId": target.conversation_id,
                "membershipId": target.membership_id,
                "prompt": "must not execute"
            }),
        )
        .unwrap_err()
        .code,
        "caller_authentication_required"
    );
    assert!(calls.lock().unwrap().is_empty());
}

#[test]
fn unverified_direct_dispatch_records_inbound_claim_and_preserves_native_failure() {
    let store = ConversationStore::open_in_memory().unwrap();
    let owner = Principal {
        id: "human:owner".into(),
        kind: PrincipalKind::Human,
        display_name: "Owner".into(),
        agent_id: None,
        created_at_unix_ms: 1,
    };
    let members = ["codex", "cursor", "antigravity"].map(|provider| {
        (
            Principal {
                id: format!("agent:{provider}"),
                kind: PrincipalKind::Agent,
                display_name: provider.into(),
                agent_id: Some(provider.into()),
                created_at_unix_ms: 1,
            },
            MembershipAccess::Member,
        )
    });
    let conversation = store
        .create_conversation_with_members("Direct", owner, &members)
        .unwrap();
    let by_provider = conversation
        .memberships
        .iter()
        .filter_map(|membership| {
            membership
                .principal
                .agent_id
                .as_deref()
                .map(|provider| (provider.to_owned(), membership.id.clone()))
        })
        .collect::<BTreeMap<_, _>>();
    let calls = Arc::new(Mutex::new(Vec::new()));
    let host = Arc::new(FixtureHost::new(
        store.clone(),
        by_provider
            .iter()
            .map(|(provider, membership)| {
                (
                    membership.clone(),
                    ProviderId::parse(provider.clone()).unwrap(),
                )
            })
            .collect(),
    ));
    let mut registry = AdapterRegistry::empty();
    for provider in ["codex", "cursor", "antigravity"] {
        let provider_id = ProviderId::parse(provider).unwrap();
        registry
            .register_pair(
                Arc::new(FixtureCaller {
                    provider: provider_id.clone(),
                }),
                Arc::new(FixtureRuntime {
                    readiness: if provider == "cursor" {
                        unverified(&provider_id)
                    } else {
                        ready(&provider_id)
                    },
                    ..fixture_runtime(
                        provider_id,
                        Arc::clone(&calls),
                        (provider == "cursor").then(|| store.clone()),
                    )
                    .with_send_failure((provider == "antigravity").then_some(
                        AdapterFailure::retryable(
                            "provider_protocol_unavailable",
                            "protocol/initialize",
                        ),
                    ))
                }),
            )
            .unwrap();
    }
    let app = SubagentApplication::new(host, registry, Arc::new(FixtureTargets));
    let caller = CallerContext {
        provider_id: ProviderId::parse("codex").unwrap(),
        conversation_id: Some(conversation.id.clone()),
        membership_id: Some(by_provider["codex"].clone()),
        parent_dispatch_id: None,
        authenticated: true,
    };
    let receipt = invoke(
        &app,
        &caller,
        "lico_subagent_delegate",
        json!({
            "conversationId": conversation.id,
            "membershipId": by_provider["cursor"],
            "prompt": "synthetic bounded work",
            "model": "composer-2.5"
        }),
    )
    .unwrap();
    assert_eq!(receipt["accepted"], true);
    assert_eq!(receipt["agentId"], "cursor");
    assert_eq!(
        calls.lock().unwrap().len(),
        1,
        "only the selected target runs"
    );
    assert_eq!(
        calls.lock().unwrap()[0],
        receipt["dispatchId"].as_str().unwrap()
    );

    let edge = store
        .subagent_mesh_edge(
            &conversation.id,
            &by_provider["codex"],
            &by_provider["cursor"],
        )
        .unwrap();
    assert!(edge.inbound_delegate);
    assert_eq!(edge.delegate_outcome.as_deref(), Some("accepted"));
    assert_eq!(edge.claim_state.as_deref(), Some("running"));
    assert_eq!(edge.dispatch_state.as_deref(), Some("accepted"));

    // The fixture turn never settles on its own, so the claim stays running
    // until the canonical dispatch does. Once the delegated PersistentTurn
    // reaches a terminal state, `finish_runtime_dispatch` moves the claim to
    // the matching terminal state in the same transaction; `subagent_claim`
    // is a direct row read that never reconciles, so this assertion proves
    // the eager writeback rather than the lazy reconciler.
    let dispatch_id = receipt["dispatchId"].as_str().unwrap();
    let delegated_event = store
        .page_events(&conversation.id, None, 50)
        .unwrap()
        .events
        .into_iter()
        .find(|event| {
            event.correlation_id.as_deref() == Some(dispatch_id)
                && event.author_membership_id.as_deref() == Some(by_provider["cursor"].as_str())
        })
        .unwrap();
    let scope = ConversationRuntimeScope {
        dispatch_id: dispatch_id.to_owned(),
        conversation_id: conversation.id.clone(),
        membership_id: by_provider["cursor"].clone(),
        event_id: delegated_event.id.clone(),
    };
    store
        .finish_runtime_dispatch(
            &scope,
            &json!({"output": "fixture delegated output"}),
            DispatchState::Completed,
            None,
        )
        .unwrap();
    assert_eq!(
        store.subagent_claim(dispatch_id).unwrap().unwrap().state,
        SubagentDispatchClaimState::Completed
    );

    let calls_before_invalid_request = calls.lock().unwrap().len();
    assert_eq!(
        invoke(
            &app,
            &caller,
            "lico_subagent_delegate",
            json!({
                "conversationId": conversation.id,
                "membershipId": by_provider["cursor"],
                "prompt": "   "
            }),
        )
        .unwrap_err()
        .code,
        "invalid_request"
    );
    assert_eq!(calls.lock().unwrap().len(), calls_before_invalid_request);

    let failure = invoke(
        &app,
        &caller,
        "lico_subagent_delegate",
        json!({
            "conversationId": conversation.id,
            "membershipId": by_provider["antigravity"],
            "prompt": "synthetic typed failure"
        }),
    )
    .unwrap_err();
    assert_eq!(failure.code, "provider_protocol_unavailable");
    assert_eq!(failure.stage, "protocol/initialize");
    assert!(failure.retryable);
    assert_eq!(failure.recovery, "retry_after_recovery");
    assert_eq!(calls.lock().unwrap().len(), 2);
}

#[test]
fn assistant_execute_replays_settle_a_callback_wait_with_the_master_decision() {
    use crate::domain::workflow_runtime::{
        ActorTurnPort, StrategyPackageImporter, StrategyService, StrategyStore,
        TurnCancelDisposition,
    };

    let root =
        std::env::temp_dir().join(format!("licoup-subagent-callback-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&root).unwrap();
    let store = ConversationStore::open(&root).unwrap();
    let conversation = store
        .create_conversation(
            "Group",
            Principal {
                id: "human:owner".into(),
                kind: PrincipalKind::Human,
                display_name: "Owner".into(),
                agent_id: None,
                created_at_unix_ms: 1,
            },
        )
        .unwrap();
    let membership = store
        .add_member(
            &conversation.id,
            Principal {
                id: "agent:codex".into(),
                kind: PrincipalKind::Agent,
                display_name: "Codex".into(),
                agent_id: Some("codex".into()),
                created_at_unix_ms: 1,
            },
            MembershipAccess::Member,
        )
        .unwrap();
    let owner_membership_id = conversation
        .memberships
        .iter()
        .find(|candidate| candidate.principal.kind == PrincipalKind::Human)
        .unwrap()
        .id
        .clone();
    let revision = store.get(&conversation.id).unwrap().revision;
    store
        .set_conversation_assistant(
            &conversation.id,
            &owner_membership_id,
            revision,
            Some(&membership.id),
        )
        .unwrap();

    let strategy = StrategyService::from_parts(
        root.clone(),
        StrategyStore::open(&root).unwrap(),
        StrategyPackageImporter::open(&root).unwrap(),
    )
    .with_actor_turn_port(ActorTurnPort {
        open: Arc::new(|_| Ok("dispatch:entry-1".to_owned())),
        run: Arc::new(|_, _| {
            Ok(json!({"ok": true, "output": "done", "nativeSessionId": "session-1"}))
        }),
        cancel: Arc::new(|_| TurnCancelDisposition::NoActiveTurn),
        abandon: Arc::new(|_| {}),
    })
    .with_profile_snapshot_authority(std::sync::Arc::new(Mutex::new(Box::new(ReadyProfiles))));

    let host = Arc::new(StrategyFixtureHost {
        inner: FixtureHost::new(
            store.clone(),
            BTreeMap::from([(membership.id.clone(), ProviderId::parse("codex").unwrap())]),
        ),
        strategy,
    });
    let mut registry = AdapterRegistry::empty();
    let provider_id = ProviderId::parse("codex").unwrap();
    registry
        .register_pair(
            Arc::new(FixtureCaller {
                provider: provider_id.clone(),
            }),
            Arc::new(fixture_runtime(
                provider_id.clone(),
                Arc::new(Mutex::new(Vec::new())),
                None,
            )),
        )
        .unwrap();
    let app = SubagentApplication::new(host, registry, Arc::new(FixtureTargets));
    let caller = CallerContext {
        provider_id,
        conversation_id: Some(conversation.id.clone()),
        membership_id: Some(membership.id.clone()),
        parent_dispatch_id: None,
        authenticated: true,
    };

    let workflow = json!({
        "schema": "licoup.adaptive-flywheel.workflow.v1",
        "metadata": {"id": "assistant-temporary", "name": "Assistant Callback Graph", "version": "1"},
        "limits": {"maxParallelism": 2, "maxWorksetItems": 16, "maxAttempts": 2},
        "actorSlots": [{"id": "subagent-a", "kind": "actor", "label": "Subagent A", "required": true, "entry": true}],
        "runtimes": [],
        "worksets": [],
        "initial": "running",
        "states": [
            {"id": "running", "kind": "actor", "label": "Running", "binding": "subagent-a"},
            {"id": "done", "kind": "succeed", "label": "Done"},
            {"id": "failed", "kind": "fail", "label": "Failed"}
        ],
        "transitions": [
            {"id": "succeeded", "from": "running", "event": "success", "to": "done", "mode": "callback"},
            {"id": "failed", "from": "running", "event": "failure", "to": "failed"}
        ]
    });
    let request = |idempotency_key: &str| {
        json!({
            "conversationId": conversation.id,
            "membershipId": membership.id,
            "workflow": workflow,
            "bindings": [{
                "slotId": "subagent-a",
                "ordinal": 0,
                "valueId": membership.id,
                "model": "model-a",
                "reasoningEffort": "",
                "revision": 1
            }],
            "input": {"message": "hi"},
            "idempotencyKey": idempotency_key
        })
    };

    // The entry effect completes, the callback edge parks the run, and the
    // execute call returns the pending decision to the MCP master.
    let parked = invoke(
        &app,
        &caller,
        "lico_assistant_workflow_execute",
        request("mcp-callback-1"),
    )
    .unwrap();
    assert_eq!(parked["ok"], true, "{parked}");
    assert_eq!(parked["result"]["status"], "waiting");
    assert_eq!(
        parked["result"]["terminal"]["code"],
        "callback_decision_required"
    );
    assert_eq!(
        parked["result"]["pendingCallbacks"][0]["stateId"],
        json!("running")
    );
    assert_eq!(
        parked["result"]["pendingCallbacks"][0]["stateVisit"],
        json!(1)
    );
    let run_id = parked["result"]["runId"].as_str().unwrap().to_owned();

    // The conversation surface names the MCP answer channel for the master.
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    let report = loop {
        let found = store
            .page_events(&conversation.id, None, 64)
            .unwrap()
            .events
            .iter()
            .filter_map(|event| {
                event
                    .parts
                    .iter()
                    .filter(|part| part.kind == licoup_conversation::EventPartKind::Metadata)
                    .filter_map(|part| serde_json::from_str::<Value>(&part.content).ok())
                    .find(|content| content["kind"] == json!("strategy-callback-request"))
            })
            .next();
        if let Some(report) = found {
            break report;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "the callback request never reached the conversation"
        );
        std::thread::sleep(std::time::Duration::from_millis(20));
    };
    assert_eq!(report["runId"], json!(run_id));
    assert_eq!(report["stateId"], json!("running"));
    assert_eq!(
        report["answerChannel"],
        json!("lico_assistant_workflow_execute")
    );
    assert_eq!(
        report["answerFields"],
        json!(["decision", "callbackStateId", "callbackStateVisit"])
    );

    // Inspect projects the pending wait...
    let inspected = invoke(
        &app,
        &caller,
        "lico_assistant_workflow_inspect",
        json!({"runId": run_id}),
    )
    .unwrap();
    assert_eq!(
        inspected["result"]["pendingCallbacks"]
            .as_array()
            .map(Vec::len),
        Some(1)
    );

    // ...and the same idempotent execute carries the master decision back in.
    let mut advance = request("mcp-callback-1");
    advance["decision"] = json!("advance");
    advance["callbackStateId"] = json!("running");
    advance["callbackStateVisit"] = json!(1);
    let advanced = invoke(&app, &caller, "lico_assistant_workflow_execute", advance).unwrap();
    assert_eq!(advanced["ok"], true, "{advanced}");
    assert_eq!(advanced["result"]["terminal"]["status"], "completed");
    assert_eq!(advanced["result"]["runId"], json!(run_id));
    let inspected = invoke(
        &app,
        &caller,
        "lico_assistant_workflow_inspect",
        json!({"runId": run_id}),
    )
    .unwrap();
    assert_eq!(
        inspected["result"]["pendingCallbacks"]
            .as_array()
            .map(Vec::len),
        Some(0),
        "the decided wait is cleared"
    );

    // A replayed decision is stale and never double-enters the target.
    let mut stale = request("mcp-callback-1");
    stale["decision"] = json!("advance");
    stale["callbackStateId"] = json!("running");
    stale["callbackStateVisit"] = json!(1);
    let stale = invoke(&app, &caller, "lico_assistant_workflow_execute", stale).unwrap();
    assert_eq!(stale["ok"], false, "{stale}");
    assert_eq!(stale["error"]["code"], "callback_stale");

    // Terminate delegates to the existing cancel semantics.
    let parked = invoke(
        &app,
        &caller,
        "lico_assistant_workflow_execute",
        request("mcp-callback-2"),
    )
    .unwrap();
    assert_eq!(parked["result"]["status"], "waiting");
    let mut terminate = request("mcp-callback-2");
    terminate["decision"] = json!("terminate");
    terminate["callbackStateId"] = json!("running");
    terminate["callbackStateVisit"] = json!(1);
    let terminated = invoke(&app, &caller, "lico_assistant_workflow_execute", terminate).unwrap();
    assert_eq!(terminated["ok"], true, "{terminated}");
    assert_eq!(terminated["result"]["terminal"]["status"], "cancelled");

    drop(app);
    for _ in 0..100 {
        if std::fs::remove_dir_all(&root).is_ok() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
}

#[test]
fn self_call_records_rejected_inbound_without_a_claim() {
    let store = ConversationStore::open_in_memory().unwrap();
    let owner = Principal {
        id: "human:owner".into(),
        kind: PrincipalKind::Human,
        display_name: "Owner".into(),
        agent_id: None,
        created_at_unix_ms: 1,
    };
    let conversation = store
        .create_conversation_with_members(
            "Self",
            owner,
            &[(
                Principal {
                    id: "agent:codex".into(),
                    kind: PrincipalKind::Agent,
                    display_name: "Codex".into(),
                    agent_id: Some("codex".into()),
                    created_at_unix_ms: 1,
                },
                MembershipAccess::Member,
            )],
        )
        .unwrap();
    let membership = conversation
        .memberships
        .iter()
        .find(|membership| membership.principal.agent_id.as_deref() == Some("codex"))
        .unwrap()
        .id
        .clone();
    let host = Arc::new(FixtureHost::new(
        store.clone(),
        BTreeMap::from([(membership.clone(), ProviderId::parse("codex").unwrap())]),
    ));
    let mut registry = AdapterRegistry::empty();
    let provider_id = ProviderId::parse("codex").unwrap();
    registry
        .register_pair(
            Arc::new(FixtureCaller {
                provider: provider_id.clone(),
            }),
            Arc::new(fixture_runtime(
                provider_id.clone(),
                Arc::new(Mutex::new(Vec::new())),
                None,
            )),
        )
        .unwrap();
    let app = SubagentApplication::new(host, registry, Arc::new(FixtureTargets));
    let caller = CallerContext {
        provider_id,
        conversation_id: Some(conversation.id.clone()),
        membership_id: Some(membership.clone()),
        parent_dispatch_id: None,
        authenticated: true,
    };
    assert_eq!(
        invoke(
            &app,
            &caller,
            "lico_subagent_delegate",
            json!({
                "conversationId": conversation.id,
                "membershipId": membership,
                "prompt": "synthetic self call"
            }),
        )
        .unwrap_err()
        .code,
        "subagent_self_call_rejected"
    );
    let edge = store
        .subagent_mesh_edge(&conversation.id, &membership, &membership)
        .unwrap();
    assert_eq!(edge.inbound_delegate, true);
    assert_eq!(
        edge.delegate_outcome.as_deref(),
        Some("subagent_self_call_rejected")
    );
    assert_eq!(edge.claim_state, None);
}

#[test]
fn session_bound_delegate_accepts_agent_and_prompt() {
    let store = ConversationStore::open_in_memory().unwrap();
    let owner = Principal {
        id: "human:owner".into(),
        kind: PrincipalKind::Human,
        display_name: "Owner".into(),
        agent_id: None,
        created_at_unix_ms: 1,
    };
    let members = ["codex", "cursor"].map(|provider| {
        (
            Principal {
                id: format!("agent:{provider}"),
                kind: PrincipalKind::Agent,
                display_name: provider.into(),
                agent_id: Some(provider.into()),
                created_at_unix_ms: 1,
            },
            MembershipAccess::Member,
        )
    });
    let conversation = store
        .create_conversation_with_members("Session", owner, &members)
        .unwrap();
    let by_provider = conversation
        .memberships
        .iter()
        .filter_map(|membership| {
            membership
                .principal
                .agent_id
                .as_deref()
                .map(|provider| (provider.to_owned(), membership.id.clone()))
        })
        .collect::<BTreeMap<_, _>>();
    let calls = Arc::new(Mutex::new(Vec::new()));
    let host = Arc::new(FixtureHost::new(
        store.clone(),
        by_provider
            .iter()
            .map(|(provider, membership)| {
                (
                    membership.clone(),
                    ProviderId::parse(provider.clone()).unwrap(),
                )
            })
            .collect(),
    ));
    let mut registry = AdapterRegistry::empty();
    for provider in ["codex", "cursor"] {
        let provider_id = ProviderId::parse(provider).unwrap();
        registry
            .register_pair(
                Arc::new(FixtureCaller {
                    provider: provider_id.clone(),
                }),
                Arc::new(fixture_runtime(provider_id, Arc::clone(&calls), None)),
            )
            .unwrap();
    }
    let app = SubagentApplication::new(host, registry, Arc::new(FixtureTargets));
    let caller = CallerContext {
        provider_id: ProviderId::parse("codex").unwrap(),
        conversation_id: Some(conversation.id.clone()),
        membership_id: Some(by_provider["codex"].clone()),
        parent_dispatch_id: None,
        authenticated: true,
    };
    let receipt = invoke(
        &app,
        &caller,
        "lico_subagent_delegate",
        json!({
            "agent": "cursor",
            "prompt": "bounded task from session"
        }),
    )
    .unwrap();
    assert_eq!(receipt["accepted"], true);
    assert_eq!(receipt["agentId"], "cursor");
    assert_eq!(receipt["membershipId"], by_provider["cursor"]);
    assert_eq!(calls.lock().unwrap().len(), 1);

    let missing = invoke(
        &app,
        &caller,
        "lico_subagent_delegate",
        json!({
            "agent": "unknown",
            "prompt": "no seat"
        }),
    )
    .unwrap_err();
    assert_eq!(missing.code, "subagent_target_seat_missing");
    assert!(missing.retryable);
    assert_eq!(missing.stage, "conversation/authorize");

    let no_target = invoke(
        &app,
        &caller,
        "lico_subagent_delegate",
        json!({
            "prompt": "no seat selector"
        }),
    )
    .unwrap_err();
    assert_eq!(no_target.code, "subagent_target_seat_missing");
    assert!(no_target.retryable);
}

#[test]
fn test_subagent_workspace_isolation_facts() {
    let isolation = SubagentWorkspaceIsolation::new("/abs/workspace/path").unwrap();
    assert_eq!(isolation.working_directory, "/abs/workspace/path");
    // Explicit assertion: cwd isolation is NOT isolation of quota, database, generated dirs, or tool effects
    assert!(!isolation.isolates_quota);
    assert!(!isolation.isolates_database);
    assert!(!isolation.isolates_generated_dirs);
    assert!(!isolation.isolates_tool_effects);
    isolation.assert_unisolated_invariants();

    let json_repr = serde_json::to_value(&isolation).unwrap();
    assert_eq!(json_repr["isolatesQuota"], false);
    assert_eq!(json_repr["isolatesDatabase"], false);
    assert_eq!(json_repr["isolatesGeneratedDirs"], false);
    assert_eq!(json_repr["isolatesToolEffects"], false);
}

#[test]
fn test_subagent_continuity_modes_rebuild_not_exact_restore() {
    // Only ExactResume represents exact restore; Fork, Rehydrate, and Rebuild do not.
    assert!(SubagentContinuityMode::ExactResume.is_exact_restore());
    assert!(!SubagentContinuityMode::Fork.is_exact_restore());
    assert!(!SubagentContinuityMode::Rehydrate.is_exact_restore());
    assert!(!SubagentContinuityMode::Rebuild.is_exact_restore());

    let capabilities = SubagentExtendedCapabilities {
        exact_resume: true,
        fork: false,
        rehydrate: false,
        rebuild: true,
    };
    assert!(
        capabilities
            .validate_mode(SubagentContinuityMode::ExactResume)
            .is_ok()
    );
    assert_eq!(
        capabilities
            .validate_mode(SubagentContinuityMode::Fork)
            .unwrap_err()
            .code,
        "subagent_fork_unavailable"
    );
    assert_eq!(
        capabilities
            .validate_mode(SubagentContinuityMode::Rehydrate)
            .unwrap_err()
            .code,
        "subagent_rehydrate_unavailable"
    );
    // Rebuild is always supported as a fallback, but is NOT exact restore
    assert!(
        capabilities
            .validate_mode(SubagentContinuityMode::Rebuild)
            .is_ok()
    );
}

#[test]
fn test_resume_working_directory_verification_rules() {
    // 1. None requested, none recorded => Ok(None)
    assert_eq!(verify_resume_working_directory(None, None).unwrap(), None);

    // 2. None requested, recorded present => Ok(Some(recorded)) [inherits recorded cwd]
    assert_eq!(
        verify_resume_working_directory(None, Some("/repo/base")).unwrap(),
        Some("/repo/base".to_string())
    );

    // 3. Requested matches recorded => Ok(Some(recorded))
    assert_eq!(
        verify_resume_working_directory(Some("/repo/base"), Some("/repo/base")).unwrap(),
        Some("/repo/base".to_string())
    );

    // 4. Requested differs from recorded => Err(conversation_working_directory_mismatch)
    let mismatch =
        verify_resume_working_directory(Some("/repo/other"), Some("/repo/base")).unwrap_err();
    assert_eq!(mismatch.code, "conversation_working_directory_mismatch");

    // 5. Requested present, but none was recorded => Err(conversation_working_directory_mismatch)
    let new_binding = verify_resume_working_directory(Some("/repo/other"), None).unwrap_err();
    assert_eq!(new_binding.code, "conversation_working_directory_mismatch");
}

fn create_test_conversation(
    store: &ConversationStore,
    providers: &[&str],
) -> (licoup_conversation::Conversation, BTreeMap<String, String>) {
    let owner = Principal {
        id: "human:owner".into(),
        kind: PrincipalKind::Human,
        display_name: "Owner".into(),
        agent_id: None,
        created_at_unix_ms: 1,
    };
    let members = providers
        .iter()
        .map(|provider| {
            (
                Principal {
                    id: format!("agent:{provider}"),
                    kind: PrincipalKind::Agent,
                    display_name: (*provider).into(),
                    agent_id: Some((*provider).into()),
                    created_at_unix_ms: 1,
                },
                MembershipAccess::Member,
            )
        })
        .collect::<Vec<_>>();
    let conversation = store
        .create_conversation_with_members("TestConversation", owner, &members)
        .unwrap();
    let by_provider = conversation
        .memberships
        .iter()
        .filter_map(|membership| {
            membership
                .principal
                .agent_id
                .as_deref()
                .map(|provider| (provider.to_owned(), membership.id.clone()))
        })
        .collect::<BTreeMap<_, _>>();
    (conversation, by_provider)
}

#[test]
fn test_atomic_admission_binds_member_profile_and_cwd_together() {
    let store = ConversationStore::open_in_memory().unwrap();
    let (conversation, by_provider) = create_test_conversation(&store, &["codex"]);
    let caller_member_id = by_provider["codex"].clone();

    let host = Arc::new(FixtureHost::new(
        store.clone(),
        BTreeMap::from([(
            caller_member_id.clone(),
            ProviderId::parse("codex").unwrap(),
        )]),
    ));

    let mut registry = AdapterRegistry::empty();
    let calls = Arc::new(Mutex::new(Vec::new()));
    for provider in ["codex", "antigravity"] {
        let provider_id = ProviderId::parse(provider).unwrap();
        registry
            .register_pair(
                Arc::new(FixtureCaller {
                    provider: provider_id.clone(),
                }),
                Arc::new(fixture_runtime(provider_id, Arc::clone(&calls), None)),
            )
            .unwrap();
    }

    let app = SubagentApplication::new(host.clone(), registry, Arc::new(FixtureTargets));
    let caller = CallerContext {
        provider_id: ProviderId::parse("codex").unwrap(),
        conversation_id: Some(conversation.id.clone()),
        membership_id: Some(caller_member_id),
        parent_dispatch_id: None,
        authenticated: true,
    };

    // Before delegation: "antigravity" has no seat in the conversation
    assert!(
        host.target_membership_by_agent(&conversation.id, "antigravity")
            .is_err()
    );

    // Delegate to unseated agent "antigravity" with explicit cwd, model, and reasoning effort
    let receipt = invoke(
        &app,
        &caller,
        "lico_subagent_delegate",
        json!({
            "agent": "antigravity",
            "prompt": "run task with cwd and model",
            "workingDirectory": "/var/tmp/project",
            "model": "gemini-ultra",
            "reasoningEffort": "high"
        }),
    )
    .unwrap();
    assert_eq!(receipt["accepted"], true);
    assert_eq!(receipt["agentId"], "antigravity");

    // Verify target membership was atomically admitted into host with all attributes
    let target = host
        .target_membership_by_agent(&conversation.id, "antigravity")
        .unwrap();
    assert_eq!(target.provider_id.as_str(), "antigravity");
    assert_eq!(target.preferred_model.as_deref(), Some("gemini-ultra"));
    assert_eq!(target.preferred_reasoning_effort.as_deref(), Some("high"));
    assert_eq!(
        target.working_directory.as_deref(),
        Some("/var/tmp/project")
    );
}

#[test]
fn test_atomic_admission_failure_rolls_back_and_leaves_no_half_member() {
    let store = ConversationStore::open_in_memory().unwrap();
    let (conversation, by_provider) = create_test_conversation(&store, &["codex"]);
    let caller_member_id = by_provider["codex"].clone();

    let host = Arc::new(FixtureHost::new(
        store.clone(),
        BTreeMap::from([(
            caller_member_id.clone(),
            ProviderId::parse("codex").unwrap(),
        )]),
    ));

    let mut registry = AdapterRegistry::empty();
    let calls = Arc::new(Mutex::new(Vec::new()));
    for provider in ["codex", "antigravity"] {
        let provider_id = ProviderId::parse(provider).unwrap();
        registry
            .register_pair(
                Arc::new(FixtureCaller {
                    provider: provider_id.clone(),
                }),
                Arc::new(fixture_runtime(provider_id, Arc::clone(&calls), None)),
            )
            .unwrap();
    }

    let app = SubagentApplication::new(host.clone(), registry, Arc::new(FixtureTargets));
    let caller = CallerContext {
        provider_id: ProviderId::parse("codex").unwrap(),
        conversation_id: Some(conversation.id.clone()),
        membership_id: Some(caller_member_id),
        parent_dispatch_id: None,
        authenticated: true,
    };

    // Case 1: Profile binding failure causes complete atomic rollback
    host.simulate_profile_failure
        .store(true, std::sync::atomic::Ordering::SeqCst);
    let err = invoke(
        &app,
        &caller,
        "lico_subagent_delegate",
        json!({
            "agent": "antigravity",
            "prompt": "run task",
            "workingDirectory": "/var/tmp/project",
            "model": "gemini-ultra"
        }),
    )
    .unwrap_err();
    assert_eq!(err.code, "subagent_profile_binding_failed");

    // Assert NO half-member remains in host
    assert!(host.dynamic_providers.lock().unwrap().is_empty());
    assert!(host.profiles.lock().unwrap().is_empty());
    assert!(host.working_directories.lock().unwrap().is_empty());
    assert_eq!(
        host.target_membership_by_agent(&conversation.id, "antigravity")
            .unwrap_err()
            .code,
        "subagent_target_seat_missing"
    );

    // Case 2: CWD binding failure causes complete atomic rollback
    host.simulate_profile_failure
        .store(false, std::sync::atomic::Ordering::SeqCst);
    host.simulate_cwd_failure
        .store(true, std::sync::atomic::Ordering::SeqCst);
    let err = invoke(
        &app,
        &caller,
        "lico_subagent_delegate",
        json!({
            "agent": "antigravity",
            "prompt": "run task",
            "workingDirectory": "/var/tmp/project",
            "model": "gemini-ultra"
        }),
    )
    .unwrap_err();
    assert_eq!(err.code, "subagent_cwd_binding_failed");

    // Assert NO half-member, orphan profile, or leaked cwd remains
    assert!(host.dynamic_providers.lock().unwrap().is_empty());
    assert!(host.profiles.lock().unwrap().is_empty());
    assert!(host.working_directories.lock().unwrap().is_empty());
    assert_eq!(
        host.target_membership_by_agent(&conversation.id, "antigravity")
            .unwrap_err()
            .code,
        "subagent_target_seat_missing"
    );
}

#[test]
fn test_resume_rejects_conflicting_working_directory_end_to_end() {
    let store = ConversationStore::open_in_memory().unwrap();
    let (conversation, by_provider) = create_test_conversation(&store, &["codex", "cursor"]);
    let caller_member_id = by_provider["codex"].clone();
    let target_member_id = by_provider["cursor"].clone();

    let host = Arc::new(FixtureHost::new(
        store.clone(),
        BTreeMap::from([
            (
                caller_member_id.clone(),
                ProviderId::parse("codex").unwrap(),
            ),
            (
                target_member_id.clone(),
                ProviderId::parse("cursor").unwrap(),
            ),
        ]),
    ));
    // Pre-record working directory for the target member
    host.working_directories.lock().unwrap().insert(
        target_member_id.clone(),
        "/var/tmp/workspace-original".into(),
    );

    let mut registry = AdapterRegistry::empty();
    let calls = Arc::new(Mutex::new(Vec::new()));
    for provider in ["codex", "cursor"] {
        let provider_id = ProviderId::parse(provider).unwrap();
        registry
            .register_pair(
                Arc::new(FixtureCaller {
                    provider: provider_id.clone(),
                }),
                Arc::new(fixture_runtime(provider_id, Arc::clone(&calls), None)),
            )
            .unwrap();
    }

    let app = SubagentApplication::new(host.clone(), registry, Arc::new(FixtureTargets));
    let caller = CallerContext {
        provider_id: ProviderId::parse("codex").unwrap(),
        conversation_id: Some(conversation.id.clone()),
        membership_id: Some(caller_member_id),
        parent_dispatch_id: None,
        authenticated: true,
    };

    // Attempting to continue with a DIFFERENT working directory must be rejected
    let mismatch_err = invoke(
        &app,
        &caller,
        "lico_subagent_continue",
        json!({
            "agent": "cursor",
            "prompt": "continue with different cwd",
            "workingDirectory": "/var/tmp/workspace-conflicting"
        }),
    )
    .unwrap_err();
    assert_eq!(mismatch_err.code, "conversation_working_directory_mismatch");
    // No dispatch calls were made to runtime
    assert_eq!(calls.lock().unwrap().len(), 0);
}

#[test]
fn test_resume_inherits_recorded_working_directory_end_to_end() {
    let store = ConversationStore::open_in_memory().unwrap();
    let (conversation, by_provider) = create_test_conversation(&store, &["codex", "cursor"]);
    let caller_member_id = by_provider["codex"].clone();
    let target_member_id = by_provider["cursor"].clone();

    let host = Arc::new(FixtureHost::new(
        store.clone(),
        BTreeMap::from([
            (
                caller_member_id.clone(),
                ProviderId::parse("codex").unwrap(),
            ),
            (
                target_member_id.clone(),
                ProviderId::parse("cursor").unwrap(),
            ),
        ]),
    ));
    host.working_directories.lock().unwrap().insert(
        target_member_id.clone(),
        "/var/tmp/workspace-original".into(),
    );

    let mut registry = AdapterRegistry::empty();
    let calls = Arc::new(Mutex::new(Vec::new()));
    for provider in ["codex", "cursor"] {
        let provider_id = ProviderId::parse(provider).unwrap();
        registry
            .register_pair(
                Arc::new(FixtureCaller {
                    provider: provider_id.clone(),
                }),
                Arc::new(fixture_runtime(provider_id, Arc::clone(&calls), None)),
            )
            .unwrap();
    }

    let app = SubagentApplication::new(host.clone(), registry, Arc::new(FixtureTargets));
    let caller = CallerContext {
        provider_id: ProviderId::parse("codex").unwrap(),
        conversation_id: Some(conversation.id.clone()),
        membership_id: Some(caller_member_id),
        parent_dispatch_id: None,
        authenticated: true,
    };

    // Continuing with NO workingDirectory inherits the recorded working directory
    let receipt = invoke(
        &app,
        &caller,
        "lico_subagent_continue",
        json!({
            "agent": "cursor",
            "prompt": "continue with inherited cwd"
        }),
    )
    .unwrap();
    assert_eq!(receipt["accepted"], true);
    assert_eq!(calls.lock().unwrap().len(), 1);

    // Verify latest resume binding in host holds the inherited working directory
    let binding = host
        .latest_resume_binding(&conversation.id, &target_member_id)
        .unwrap();
    assert_eq!(
        binding.working_directory(),
        Some("/var/tmp/workspace-original")
    );
}

#[test]
fn test_relative_working_directory_rejected() {
    let store = ConversationStore::open_in_memory().unwrap();
    let (conversation, by_provider) = create_test_conversation(&store, &["codex"]);
    let caller_member_id = by_provider["codex"].clone();

    let host = Arc::new(FixtureHost::new(
        store.clone(),
        BTreeMap::from([(
            caller_member_id.clone(),
            ProviderId::parse("codex").unwrap(),
        )]),
    ));
    let registry = AdapterRegistry::empty();
    let app = SubagentApplication::new(host, registry, Arc::new(FixtureTargets));
    let caller = CallerContext {
        provider_id: ProviderId::parse("codex").unwrap(),
        conversation_id: Some(conversation.id.clone()),
        membership_id: Some(caller_member_id),
        parent_dispatch_id: None,
        authenticated: true,
    };

    let err = invoke(
        &app,
        &caller,
        "lico_subagent_delegate",
        json!({
            "agent": "cursor",
            "prompt": "task",
            "workingDirectory": "relative/path"
        }),
    )
    .unwrap_err();
    assert_eq!(err.code, "invalid_working_directory");
}

#[test]
fn test_continue_to_unseated_agent_does_not_auto_admit() {
    let store = ConversationStore::open_in_memory().unwrap();
    let (conversation, by_provider) = create_test_conversation(&store, &["codex"]);
    let caller_member_id = by_provider["codex"].clone();

    let host = Arc::new(FixtureHost::new(
        store.clone(),
        BTreeMap::from([(
            caller_member_id.clone(),
            ProviderId::parse("codex").unwrap(),
        )]),
    ));

    let mut registry = AdapterRegistry::empty();
    let calls = Arc::new(Mutex::new(Vec::new()));
    for provider in ["codex", "antigravity"] {
        let provider_id = ProviderId::parse(provider).unwrap();
        registry
            .register_pair(
                Arc::new(FixtureCaller {
                    provider: provider_id.clone(),
                }),
                Arc::new(fixture_runtime(provider_id, Arc::clone(&calls), None)),
            )
            .unwrap();
    }

    let app = SubagentApplication::new(host.clone(), registry, Arc::new(FixtureTargets));
    let caller = CallerContext {
        provider_id: ProviderId::parse("codex").unwrap(),
        conversation_id: Some(conversation.id.clone()),
        membership_id: Some(caller_member_id),
        parent_dispatch_id: None,
        authenticated: true,
    };

    // "antigravity" is probe-able inventory but holds no seat: a continue has
    // no native session to resume and must not admit one as a side effect.
    let err = invoke(
        &app,
        &caller,
        "lico_subagent_continue",
        json!({
            "agent": "antigravity",
            "prompt": "continue task"
        }),
    )
    .unwrap_err();
    assert_eq!(err.code, "subagent_target_seat_missing");
    assert!(
        host.target_membership_by_agent(&conversation.id, "antigravity")
            .is_err()
    );
    assert!(host.dynamic_providers.lock().unwrap().is_empty());
    assert_eq!(calls.lock().unwrap().len(), 0);
}

#[test]
fn test_delegate_records_cwd_without_replacing_native_session_identity() {
    let store = ConversationStore::open_in_memory().unwrap();
    let (conversation, by_provider) = create_test_conversation(&store, &["codex", "cursor"]);
    let caller_member_id = by_provider["codex"].clone();
    let target_member_id = by_provider["cursor"].clone();

    let host = Arc::new(FixtureHost::new(
        store.clone(),
        BTreeMap::from([
            (
                caller_member_id.clone(),
                ProviderId::parse("codex").unwrap(),
            ),
            (
                target_member_id.clone(),
                ProviderId::parse("cursor").unwrap(),
            ),
        ]),
    ));
    // The member already holds a durable native session identity.
    host.runtime_session_ids
        .lock()
        .unwrap()
        .insert(target_member_id.clone(), "native-session-cursor".to_owned());

    let mut registry = AdapterRegistry::empty();
    let calls = Arc::new(Mutex::new(Vec::new()));
    for provider in ["codex", "cursor"] {
        let provider_id = ProviderId::parse(provider).unwrap();
        registry
            .register_pair(
                Arc::new(FixtureCaller {
                    provider: provider_id.clone(),
                }),
                Arc::new(fixture_runtime(provider_id, Arc::clone(&calls), None)),
            )
            .unwrap();
    }

    let app = SubagentApplication::new(host.clone(), registry, Arc::new(FixtureTargets));
    let caller = CallerContext {
        provider_id: ProviderId::parse("codex").unwrap(),
        conversation_id: Some(conversation.id.clone()),
        membership_id: Some(caller_member_id),
        parent_dispatch_id: None,
        authenticated: true,
    };

    let receipt = invoke(
        &app,
        &caller,
        "lico_subagent_delegate",
        json!({
            "agent": "cursor",
            "prompt": "run task in workspace",
            "workingDirectory": "/var/tmp/workspace-delegated"
        }),
    )
    .unwrap();
    assert_eq!(receipt["accepted"], true);
    let dispatch_id = receipt["dispatchId"].as_str().unwrap().to_owned();

    // The effective cwd is recorded, but the dispatch claim id never replaces
    // the native session identity used for exact resume.
    let binding = host
        .latest_resume_binding(&conversation.id, &target_member_id)
        .unwrap();
    assert_eq!(
        binding.working_directory(),
        Some("/var/tmp/workspace-delegated")
    );
    assert_eq!(binding.native_session_id(), "native-session-cursor");
    assert_ne!(binding.native_session_id(), dispatch_id);
}
