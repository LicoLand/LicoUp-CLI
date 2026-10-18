//! Native ports behind the shared `licoup_application` facade.
//!
//! Every admitted interface surface decodes its own request into a
//! [`licoup_application`] command and hands it to [`ApplicationFacade`]. This
//! module is the backend the facade runs against: it holds the one translation
//! from a typed command to the domain owner's own invocation, and the one
//! projection from that owner's answer back to a [`CommandOutcome`].
//!
//! Three properties are load-bearing:
//!
//! - Durable work reaches the *already running* conversation host through
//!   [`crate::platform::conversation_host_client::execute_existing`]. With no
//!   host running the call fails closed with
//!   `persistent_conversation_transport_required`. This adapter never opens a
//!   process-local `ConversationService`, so a one-shot CLI process cannot
//!   silently serve a private store.
//! - Orchestration is not re-implemented here. Delegation, resume identity,
//!   cancellation and workflow control run through
//!   [`crate::domain::subagents::production_application`], which already owns
//!   claims, lineage and the durable transitions. The ports translate; they do
//!   not decide.
//! - The actor check here is the one this process can make about the caller: the
//!   in-process owner, or a provider the local mesh admits. Whether that caller
//!   is active in a conversation stays the domain owner's durable check, which
//!   it performs before any effect.

use crate::contracts::conversation_protocol::ConversationProtocolMethod;
use crate::domain::project::NativeProjectApplication;
use crate::domain::subagents::{
    CallerContext, SubagentCallContext, SubagentError, production_application,
};
use licoup_application::{
    ActorClaim, ActorPort, ApplicationCommand, ApplicationFacade, ApplicationFailure,
    ApplicationPorts, AssistantCommand, AssistantPort, CallbackDecision, CancelRequest,
    CommandOutcome, CommandResolution, ConversationCommand, ConversationPort, DispatchRequest,
    FailureNormalization, Operation, OperationReference, OperationState, RecoveryAction,
    SubagentCommand, SubagentPort, TaskType,
};
use serde_json::{Map, Value, json};
use std::sync::Arc;
use std::sync::atomic::AtomicBool;

/// The owner membership the in-process CLI acts as.
///
/// The business contract's own fixtures name their local admin this way
/// (`crates/licoup-application/tests/command_contract.rs`). This slice verifies
/// the claim by process ownership rather than by a store lookup, so naming it
/// needs no store access.
pub const LOCAL_ADMIN_OWNER_MEMBERSHIP_ID: &str = "membership:owner";

/// Where a durable-host transport failure happened.
const CONVERSATION_TRANSPORT_STAGE: &str = "conversation/transport";

/// Where a caller-scope refusal happened.
const CALLER_STAGE: &str = "caller/authorize";

/// The durable host method both reads travel over.
///
/// This is one of the frozen 29 RPC methods; the typed surface reuses it rather
/// than adding a wire method of its own.
const CONVERSATION_HOST_METHOD: &str =
    ConversationProtocolMethod::ClientConversationExecute.as_str();

/// Run one typed command as the local admin and report its resolution.
pub fn execute(command: &ApplicationCommand) -> CommandResolution {
    execute_as(&local_admin_claim(), command)
}

/// Run one typed command as one claimed caller and report its resolution.
pub fn execute_as(claim: &ActorClaim, command: &ApplicationCommand) -> CommandResolution {
    match application_facade().execute(claim, command) {
        Ok(outcome) => CommandResolution::Resolved(outcome),
        Err(failure) => CommandResolution::Failed(failure),
    }
}

/// The local-admin claim the in-process CLI acts as.
pub fn local_admin_claim() -> ActorClaim {
    ActorClaim::local_admin(LOCAL_ADMIN_OWNER_MEMBERSHIP_ID)
}

/// The published Subagents catalog: the callers the mesh admits, and the tool
/// schema the domain owner admits for them.
pub fn subagent_catalog() -> Result<Value, ApplicationFailure> {
    let application = production_application().map_err(|error| port_failure(&error))?;
    Ok(json!({
        "callers": application.caller_providers(),
        "tools": application.tool_catalog(),
    }))
}

/// The single facade the native application runs through.
fn application_facade() -> ApplicationFacade {
    ApplicationFacade::new(ApplicationPorts::new(
        Arc::new(NativeCallerAuthority),
        Arc::new(NativeAssistantApplication),
        Arc::new(NativeSubagentApplication),
        Arc::new(DurableConversationHost),
        Arc::new(NativeProjectApplication::portable()),
    ))
}

/// Verify the caller of one command.
struct NativeCallerAuthority;

impl ActorPort for NativeCallerAuthority {
    fn verify(&self, claim: &ActorClaim) -> Result<(), ApplicationFailure> {
        let ActorClaim::Membership { provider_id, .. } = claim else {
            // The in-process caller *is* the local owner, so there is nothing
            // further to look up about it.
            return Ok(());
        };
        // A provider claim is only as good as the mesh that admits it. The
        // conversation-level check stays with the domain owner, which performs
        // it against the durable store before any effect.
        if crate::platform::runtime_adapters::production_subagent_registry(
            crate::target_port::agent_target_port(),
        )
        .caller_providers()
        .any(|provider| provider.as_str() == provider_id)
        {
            return Ok(());
        }
        Err(ApplicationFailure::permanent(
            "subagents_caller_invalid",
            CALLER_STAGE,
        ))
    }
}

/// The delegated-work family, through the domain owner.
struct NativeSubagentApplication;

impl SubagentPort for NativeSubagentApplication {
    fn execute(
        &self,
        claim: &ActorClaim,
        command: &SubagentCommand,
    ) -> Result<CommandOutcome, ApplicationFailure> {
        let (name, arguments) = subagent_invocation(command);
        let answer = call_domain(claim, &name, &arguments)?;
        Ok(subagent_outcome(command, answer))
    }
}

/// The Assistant family, through the same domain owner.
struct NativeAssistantApplication;

impl AssistantPort for NativeAssistantApplication {
    fn execute(
        &self,
        claim: &ActorClaim,
        command: &AssistantCommand,
    ) -> Result<CommandOutcome, ApplicationFailure> {
        let (name, arguments) = assistant_invocation(command);
        let answer = call_domain(claim, &name, &arguments)?;
        Ok(assistant_outcome(command, answer))
    }
}

/// The two Canonical Conversation reads, over the durable host.
struct DurableConversationHost;

impl ConversationPort for DurableConversationHost {
    fn execute(
        &self,
        _claim: &ActorClaim,
        command: &ConversationCommand,
    ) -> Result<CommandOutcome, ApplicationFailure> {
        match command {
            ConversationCommand::List { include_archived } => {
                let payload = host_read(json!({
                    "action": "conversation.list",
                    "includeArchived": include_archived,
                }))?;
                Ok(read_outcome(Operation::ConversationList, payload))
            }
            ConversationCommand::Get { conversation_id } => {
                let payload = host_read(json!({
                    "action": "conversation.get",
                    "conversationId": conversation_id,
                }))?;
                Ok(read_outcome(Operation::ConversationGet, payload))
            }
            _ => Err(ApplicationFailure::permanent(
                "conversation_operation_unsupported",
                "conversation/execute",
            )),
        }
    }
}

/// One domain tool call as the claimed caller.
///
/// The domain owner checks the caller against the conversation store and refuses
/// before any effect, so `authenticated` here means exactly what the facade just
/// established: this process admitted the claim that carries it.
fn call_domain(
    claim: &ActorClaim,
    name: &str,
    arguments: &Map<String, Value>,
) -> Result<Value, ApplicationFailure> {
    let application = production_application().map_err(|error| port_failure(&error))?;
    let caller = caller_context(claim)?;
    application
        .call_tool(
            SubagentCallContext {
                caller: &caller,
                cancelled: Arc::new(AtomicBool::new(false)),
            },
            name,
            arguments,
        )
        .map_err(|error| port_failure(&error))
}

/// The domain's caller scope for one claim.
fn caller_context(claim: &ActorClaim) -> Result<CallerContext, ApplicationFailure> {
    let ActorClaim::Membership {
        provider_id,
        conversation_id,
        membership_id,
        parent_dispatch_id,
    } = claim
    else {
        return Err(missing_caller_scope());
    };
    let provider_id = licoup_agent_runtime::ProviderId::parse(provider_id.clone())
        .map_err(|_| ApplicationFailure::permanent("subagents_caller_invalid", CALLER_STAGE))?;
    Ok(CallerContext {
        provider_id,
        conversation_id: conversation_id.clone(),
        membership_id: membership_id.clone(),
        parent_dispatch_id: parent_dispatch_id.clone(),
        authenticated: true,
    })
}

/// The refusal for delegated work with no caller scope. Delegation is always
/// done on behalf of a membership, so an in-process owner has none.
fn missing_caller_scope() -> ApplicationFailure {
    ApplicationFailure::permanent("caller_membership_binding_required", CALLER_STAGE)
}

/// One Subagent command as the domain owner's tool invocation.
pub(crate) fn subagent_invocation(command: &SubagentCommand) -> (String, Map<String, Value>) {
    match command {
        SubagentCommand::List => ("lico_subagents_list".to_owned(), Map::new()),
        SubagentCommand::Probe { agent_id } => (
            "lico_subagent_probe".to_owned(),
            object(json!({"agentId": agent_id})),
        ),
        SubagentCommand::Delegate(request) => (
            "lico_subagent_delegate".to_owned(),
            dispatch_arguments(request),
        ),
        SubagentCommand::Continue(request) => (
            "lico_subagent_continue".to_owned(),
            dispatch_arguments(request),
        ),
        SubagentCommand::Cancel(request) => {
            ("lico_subagent_cancel".to_owned(), cancel_arguments(request))
        }
    }
}

/// One Assistant command as the domain owner's tool invocation.
pub(crate) fn assistant_invocation(command: &AssistantCommand) -> (String, Map<String, Value>) {
    match command {
        AssistantCommand::Profiles {
            conversation_id,
            filters,
        } => {
            let mut arguments = Map::new();
            arguments.insert("conversationId".to_owned(), json!(conversation_id));
            put_json(&mut arguments, "filters", filters.as_ref());
            ("lico_assistant_profiles".to_owned(), arguments)
        }
        AssistantCommand::WorkflowExecute {
            conversation_id,
            membership_id,
            workflow,
            bindings,
            filters,
            input,
            idempotency_key,
            decision,
        } => {
            let mut arguments = Map::new();
            arguments.insert("conversationId".to_owned(), json!(conversation_id));
            arguments.insert("membershipId".to_owned(), json!(membership_id));
            arguments.insert("workflow".to_owned(), workflow.clone());
            arguments.insert("idempotencyKey".to_owned(), json!(idempotency_key));
            put_json(&mut arguments, "bindings", Some(bindings));
            put_json(&mut arguments, "filters", Some(filters));
            put_json(&mut arguments, "input", input.as_ref());
            if let Some(decision) = decision {
                arguments.insert("decision".to_owned(), json!(decision.as_str()));
                if let CallbackDecision::Advance {
                    state_id,
                    state_visit,
                }
                | CallbackDecision::Return {
                    state_id,
                    state_visit,
                } = decision
                {
                    arguments.insert("callbackStateId".to_owned(), json!(state_id));
                    arguments.insert("callbackStateVisit".to_owned(), json!(state_visit));
                }
            }
            ("lico_assistant_workflow_execute".to_owned(), arguments)
        }
        AssistantCommand::WorkflowInspect { run_id } => (
            "lico_assistant_workflow_inspect".to_owned(),
            object(json!({"runId": run_id})),
        ),
        AssistantCommand::WorkflowCancel { run_id } => (
            "lico_assistant_workflow_cancel".to_owned(),
            object(json!({"runId": run_id})),
        ),
    }
}

/// The arguments of one dispatch, in the names the domain owner reads.
fn dispatch_arguments(request: &DispatchRequest) -> Map<String, Value> {
    let mut arguments = Map::new();
    put(
        &mut arguments,
        "conversationId",
        request.conversation_id.clone(),
    );
    put(
        &mut arguments,
        "membershipId",
        request.membership_id.clone(),
    );
    put(&mut arguments, "agent", request.agent_id.clone());
    arguments.insert("prompt".to_owned(), json!(request.prompt));
    put(&mut arguments, "model", request.model.clone());
    put(
        &mut arguments,
        "reasoningEffort",
        request.reasoning_effort.clone(),
    );
    put(
        &mut arguments,
        "workingDirectory",
        request.working_directory.clone(),
    );
    if let Some(task_type) = request.task_type {
        arguments.insert("taskType".to_owned(), json!(task_type_wire(task_type)));
    }
    put(&mut arguments, "timeoutMs", request.timeout_ms);
    if request.timeout_unbounded {
        // The tool schema accepts `unboundedTimeout` as the same request; the
        // owner reads either, so publishing one of them is not a second form of
        // the same decision.
        arguments.insert("timeoutUnbounded".to_owned(), json!(true));
    }
    put(&mut arguments, "maxStdoutBytes", request.max_stdout_bytes);
    put(&mut arguments, "maxStderrBytes", request.max_stderr_bytes);
    arguments
}

/// The arguments of one cancellation, in the names the domain owner reads.
fn cancel_arguments(request: &CancelRequest) -> Map<String, Value> {
    let mut arguments = Map::new();
    put(
        &mut arguments,
        "conversationId",
        request.conversation_id.clone(),
    );
    put(
        &mut arguments,
        "membershipId",
        request.membership_id.clone(),
    );
    put(&mut arguments, "agent", request.agent_id.clone());
    arguments
}

/// Insert one optional argument. An absent argument stays absent: the domain
/// owner defaults a missing one but refuses an explicit null, so inventing a
/// value here would turn a valid request into a refusal.
fn put<T: Into<Value>>(arguments: &mut Map<String, Value>, key: &str, value: Option<T>) {
    if let Some(value) = value {
        arguments.insert(key.to_owned(), value.into());
    }
}

/// Insert one free-form argument unless the caller left it out.
fn put_json(arguments: &mut Map<String, Value>, key: &str, value: Option<&Value>) {
    if let Some(value) = value.filter(|value| !value.is_null()) {
        arguments.insert(key.to_owned(), value.clone());
    }
}

/// The TaskType wire name the domain owner's tool schema publishes.
const fn task_type_wire(task_type: TaskType) -> &'static str {
    match task_type {
        TaskType::Frontend => "frontend",
        TaskType::Backend => "backend",
        TaskType::Retrieval => "retrieval",
        TaskType::Text => "text",
    }
}

fn object(value: Value) -> Map<String, Value> {
    value.as_object().cloned().unwrap_or_default()
}

/// One Subagent answer as the operation both interfaces refer to.
///
/// The identity is the domain owner's own: the dispatch id it committed and the
/// state it published for it. Nothing here invents a second identity.
fn subagent_outcome(command: &SubagentCommand, answer: Value) -> CommandOutcome {
    match command {
        SubagentCommand::List | SubagentCommand::Probe { .. } => {
            read_outcome(command.operation(), answer)
        }
        SubagentCommand::Delegate(_)
        | SubagentCommand::Continue(_)
        | SubagentCommand::Cancel(_) => {
            let mut reference = OperationReference::new(
                command.operation(),
                text(&answer, "dispatchId"),
                dispatch_state(&text(&answer, "state")),
            );
            reference.conversation_id = Some(text(&answer, "conversationId"));
            reference.membership_id = Some(text(&answer, "membershipId"));
            reference.depth = answer
                .get("depth")
                .and_then(Value::as_u64)
                .and_then(|depth| u8::try_from(depth).ok());
            CommandOutcome::new(reference).with_payload(answer)
        }
    }
}

/// One Assistant answer as the operation both interfaces refer to.
fn assistant_outcome(command: &AssistantCommand, answer: Value) -> CommandOutcome {
    let (id, conversation_id, membership_id, idempotency_key) = match command {
        AssistantCommand::Profiles { .. } => return read_outcome(command.operation(), answer),
        AssistantCommand::WorkflowExecute {
            conversation_id,
            membership_id,
            idempotency_key,
            ..
        } => (
            text(&answer, "runId"),
            Some(conversation_id.clone()),
            Some(membership_id.clone()),
            Some(idempotency_key.clone()),
        ),
        AssistantCommand::WorkflowInspect { run_id }
        | AssistantCommand::WorkflowCancel { run_id } => (
            run_id.clone(),
            optional_text(&answer, "conversationId"),
            optional_text(&answer, "assistantMembershipId"),
            None,
        ),
    };
    let mut reference = OperationReference::new(
        command.operation(),
        id,
        workflow_state(&text(&answer, "status")),
    );
    reference.conversation_id = conversation_id;
    reference.membership_id = membership_id;
    reference.idempotency_key = idempotency_key;
    CommandOutcome::new(reference).with_payload(answer)
}

fn text(value: &Value, field: &str) -> String {
    optional_text(value, field).unwrap_or_default()
}

fn optional_text(value: &Value, field: &str) -> Option<String> {
    value.get(field).and_then(Value::as_str).map(str::to_owned)
}

/// One dispatch state, in the reference vocabulary.
///
/// A state this build does not know is not treated as progress: the caller is
/// told to read the durable record, which is the only thing that knows.
fn dispatch_state(state: &str) -> OperationState {
    match state {
        "accepted" => OperationState::Accepted,
        "processing" => OperationState::Processing,
        "responding" => OperationState::Responding,
        "cancel-requested" => OperationState::CancelRequested,
        "cancelled" => OperationState::Cancelled,
        "completed" => OperationState::Completed,
        "failed" => OperationState::Failed,
        _ => OperationState::ReconciliationRequired,
    }
}

/// One workflow run state, in the reference vocabulary.
///
/// Every state that is not terminal holds a live slot, so an unrecognized one is
/// reported as still running rather than as stopped.
fn workflow_state(status: &str) -> OperationState {
    match status {
        "completed" => OperationState::Completed,
        "failed" | "blocked" => OperationState::Failed,
        "cancelled" => OperationState::Cancelled,
        "cancel-requested" => OperationState::CancelRequested,
        "cancel-in-doubt" => OperationState::ReconciliationRequired,
        _ => OperationState::Processing,
    }
}

/// A read outcome: no live identity to follow, just the operation's own body.
fn read_outcome(operation: Operation, payload: Value) -> CommandOutcome {
    CommandOutcome::new(OperationReference::new(
        operation,
        "",
        OperationState::Completed,
    ))
    .with_payload(payload)
}

/// A domain failure, in the neutral failure model.
///
/// The domain's recovery string *is* the product's retry/effect pairing, so it
/// decides which normalization applies. Both interfaces then project the same
/// failure onto their own vocabulary instead of restating the pairing.
fn port_failure(error: &SubagentError) -> ApplicationFailure {
    let normalization = if error.recovery == RecoveryAction::ReconcileBeforeRetry.mcp_wire() {
        FailureNormalization::UNCERTAIN
    } else if error.retryable {
        FailureNormalization::RETRYABLE
    } else {
        FailureNormalization::PERMANENT
    };
    normalization.into_failure(error.code, error.stage)
}

/// One durable read through the already-running conversation host.
///
/// The transport answers inside its own envelope, so the operation's own body
/// is the nested `result`. Publishing the envelope verbatim would leak the
/// transport into a versioned payload.
fn host_read(params: Value) -> Result<Value, ApplicationFailure> {
    let response = crate::platform::conversation_host_client::execute_existing(
        CONVERSATION_HOST_METHOD,
        &params,
    )
    .map_err(|error| host_failure(&error))?;
    operation_payload(response)
}

/// The operation's own body out of one host reply.
fn operation_payload(response: Value) -> Result<Value, ApplicationFailure> {
    response.get("result").cloned().ok_or_else(|| {
        ApplicationFailure::retryable(
            "conversation_state_unavailable",
            CONVERSATION_TRANSPORT_STAGE,
        )
    })
}

/// Map a host failure onto the normalized failure model.
///
/// A missing host is retryable *and* needs the runtime installed or restarted,
/// so its recovery is projected accordingly. A missing conversation and a
/// rejected request are permanent, so a caller is not told to retry them.
fn host_failure(error: &anyhow::Error) -> ApplicationFailure {
    let message = error.to_string();
    let code = message.split(':').next().unwrap_or_default().trim();
    match code {
        "persistent_conversation_transport_required" => {
            ApplicationFailure::retryable(code, CONVERSATION_TRANSPORT_STAGE)
                .with_recovery(RecoveryAction::InstallOrRetryRuntime)
        }
        "conversation_not_found" | "invalid_request" => {
            ApplicationFailure::permanent(code, CONVERSATION_TRANSPORT_STAGE)
        }
        "" => ApplicationFailure::retryable(
            "conversation_transport_failed",
            CONVERSATION_TRANSPORT_STAGE,
        ),
        other => ApplicationFailure::retryable(other, CONVERSATION_TRANSPORT_STAGE),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use licoup_application::EffectCertainty;

    #[test]
    fn a_read_publishes_the_operation_body_not_the_transport_envelope() {
        let payload = operation_payload(json!({
            "ok": true,
            "result": [{"id": "lico-group-default"}],
        }))
        .expect("a completed reply carries a body");
        assert_eq!(payload, json!([{"id": "lico-group-default"}]));
    }

    #[test]
    fn a_reply_without_a_body_is_retryable_rather_than_published() {
        let failure = operation_payload(json!({"ok": false})).expect_err("no body to publish");
        assert_eq!(failure.code, "conversation_state_unavailable");
        assert_eq!(failure.stage, CONVERSATION_TRANSPORT_STAGE);
        assert!(failure.retryable);
    }

    /// The domain's own recovery string decides the neutral pairing, and each
    /// surface projects the pairing back to that same string. A round trip that
    /// moved it would tell one interface to reconcile and the other to retry.
    #[test]
    fn a_domain_failure_keeps_its_code_stage_and_recovery_through_the_model() {
        for (retryable, recovery) in [
            (false, "correct_request_and_retry"),
            (true, "retry_after_recovery"),
            (true, "reconcile_before_retry"),
        ] {
            let failure = port_failure(&SubagentError {
                code: "subagent_adapter_unavailable",
                stage: "adapter/select",
                retryable,
                recovery,
            });
            assert_eq!(failure.code, "subagent_adapter_unavailable");
            assert_eq!(failure.stage, "adapter/select");
            assert_eq!(failure.retryable, retryable);
            assert_eq!(failure.recovery.mcp_wire(), recovery);
        }
        assert_eq!(
            port_failure(&SubagentError {
                code: "dispatch_reconciliation_required",
                stage: "dispatch/reconcile",
                retryable: true,
                recovery: "reconcile_before_retry",
            })
            .effect,
            EffectCertainty::Uncertain
        );
    }

    /// An absent optional argument stays absent. The domain owner defaults a
    /// missing filter set to the empty set but refuses an explicit null, so
    /// inventing one here would turn a valid dispatch into a refusal.
    #[test]
    fn optional_dispatch_arguments_are_omitted_rather_than_sent_as_null() {
        let (name, arguments) = subagent_invocation(&SubagentCommand::Delegate(DispatchRequest {
            conversation_id: Some("conversation:one".into()),
            agent_id: Some("codex".into()),
            prompt: "review the diff".into(),
            ..DispatchRequest::default()
        }));
        assert_eq!(name, "lico_subagent_delegate");
        assert_eq!(
            argument_names(&arguments),
            ["agent", "conversationId", "prompt"]
        );
    }

    #[test]
    fn a_dispatch_request_carries_every_field_the_tool_schema_admits() {
        let (name, arguments) = subagent_invocation(&SubagentCommand::Continue(DispatchRequest {
            conversation_id: Some("conversation:one".into()),
            membership_id: Some("membership:worker".into()),
            prompt: "continue".into(),
            model: Some("gpt-5.6-luna".into()),
            reasoning_effort: Some("max".into()),
            working_directory: Some("/synthetic/workspace".into()),
            task_type: Some(TaskType::Retrieval),
            timeout_ms: Some(60_000),
            timeout_unbounded: true,
            max_stdout_bytes: Some(65_536),
            max_stderr_bytes: Some(16_384),
            ..DispatchRequest::default()
        }));
        assert_eq!(name, "lico_subagent_continue");
        assert_eq!(arguments["conversationId"], json!("conversation:one"));
        assert_eq!(arguments["membershipId"], json!("membership:worker"));
        assert_eq!(arguments["prompt"], json!("continue"));
        assert_eq!(arguments["model"], json!("gpt-5.6-luna"));
        assert_eq!(arguments["reasoningEffort"], json!("max"));
        assert_eq!(arguments["workingDirectory"], json!("/synthetic/workspace"));
        assert_eq!(arguments["taskType"], json!("retrieval"));
        assert_eq!(arguments["timeoutMs"], json!(60_000));
        assert_eq!(arguments["timeoutUnbounded"], json!(true));
        assert_eq!(arguments["maxStdoutBytes"], json!(65_536));
        assert_eq!(arguments["maxStderrBytes"], json!(16_384));
    }

    #[test]
    fn a_cancellation_names_its_target_and_nothing_else() {
        let (name, arguments) = subagent_invocation(&SubagentCommand::Cancel(CancelRequest {
            conversation_id: Some("conversation:one".into()),
            agent_id: Some("cursor".into()),
            membership_id: None,
        }));
        assert_eq!(name, "lico_subagent_cancel");
        assert_eq!(argument_names(&arguments), ["agent", "conversationId"]);
    }

    /// A parked callback wait is answered by naming the state it answers; a
    /// terminal decision names none. Sending a state id with a terminate would
    /// answer a question the domain owner never asked.
    #[test]
    fn a_callback_decision_names_a_state_only_when_it_is_parked() {
        let execute = |decision| {
            assistant_invocation(&AssistantCommand::WorkflowExecute {
                conversation_id: "conversation:one".into(),
                membership_id: "membership:assistant".into(),
                workflow: json!({"steps": []}),
                bindings: json!([]),
                filters: json!({"membershipIds": ["membership:worker"]}),
                input: None,
                idempotency_key: "idem-1".into(),
                decision,
            })
            .1
        };
        let advanced = execute(Some(CallbackDecision::Advance {
            state_id: "state:one".into(),
            state_visit: 2,
        }));
        assert_eq!(advanced["decision"], json!("advance"));
        assert_eq!(advanced["callbackStateId"], json!("state:one"));
        assert_eq!(advanced["callbackStateVisit"], json!(2));

        let terminated = execute(Some(CallbackDecision::Terminate));
        assert_eq!(terminated["decision"], json!("terminate"));
        assert!(terminated.get("callbackStateId").is_none());
        assert!(terminated.get("callbackStateVisit").is_none());

        assert!(execute(None).get("decision").is_none());
    }

    /// The Assistant admission pass ranks candidate bindings against the
    /// caller's filters, so dropping them would run a different admission than
    /// the one that was asked for.
    #[test]
    fn a_workflow_execute_carries_the_filters_it_was_given() {
        let (name, arguments) = assistant_invocation(&AssistantCommand::WorkflowExecute {
            conversation_id: "conversation:one".into(),
            membership_id: "membership:assistant".into(),
            workflow: json!({"steps": []}),
            bindings: json!([{"valueId": "membership:worker"}]),
            filters: json!({"requiredAuthority": "delegate"}),
            input: Some(json!({"topic": "release"})),
            idempotency_key: "idem-1".into(),
            decision: None,
        });
        assert_eq!(name, "lico_assistant_workflow_execute");
        assert_eq!(
            arguments["filters"],
            json!({"requiredAuthority": "delegate"})
        );
        assert_eq!(arguments["input"], json!({"topic": "release"}));
    }

    #[test]
    fn a_profile_listing_without_filters_sends_none() {
        let (name, arguments) = assistant_invocation(&AssistantCommand::Profiles {
            conversation_id: "conversation:one".into(),
            filters: None,
        });
        assert_eq!(name, "lico_assistant_profiles");
        assert_eq!(argument_names(&arguments), ["conversationId"]);
    }

    #[test]
    fn a_dispatch_reference_is_the_identity_the_receipt_published() {
        let outcome = subagent_outcome(
            &SubagentCommand::Delegate(DispatchRequest {
                conversation_id: Some("conversation:one".into()),
                agent_id: Some("codex".into()),
                prompt: "review".into(),
                ..DispatchRequest::default()
            }),
            json!({
                "schemaVersion": "licoup.subagent.receipt.v3",
                "operation": "subagent.delegate",
                "conversationId": "conversation:one",
                "membershipId": "membership:worker",
                "dispatchId": "dispatch:one",
                "depth": 1,
                "state": "accepted",
            }),
        );
        assert_eq!(outcome.reference.operation, "subagent.delegate");
        assert_eq!(outcome.reference.id, "dispatch:one");
        assert_eq!(outcome.reference.state, OperationState::Accepted);
        assert_eq!(
            outcome.reference.conversation_id.as_deref(),
            Some("conversation:one")
        );
        assert_eq!(
            outcome.reference.membership_id.as_deref(),
            Some("membership:worker")
        );
        assert_eq!(outcome.reference.depth, Some(1));
        assert!(outcome.is_live());
    }

    /// An unknown dispatch state is neither progress nor a stop: the caller is
    /// told to read the durable record before acting.
    #[test]
    fn an_unknown_dispatch_state_asks_for_reconciliation() {
        assert_eq!(
            dispatch_state("something-new"),
            OperationState::ReconciliationRequired
        );
        assert_eq!(workflow_state("running"), OperationState::Processing);
        assert_eq!(
            workflow_state("cancel-in-doubt"),
            OperationState::ReconciliationRequired
        );
    }

    #[test]
    fn an_inventory_and_a_probe_read_without_claiming_a_live_operation() {
        for command in [
            SubagentCommand::List,
            SubagentCommand::Probe {
                agent_id: "codex".into(),
            },
        ] {
            let outcome = subagent_outcome(&command, json!({"count": 0}));
            assert!(!outcome.is_live(), "a read has no live identity to follow");
            assert_eq!(outcome.reference.state, OperationState::Completed);
        }
    }

    /// An in-process owner needs no caller scope; a provider the local mesh does
    /// not admit is refused before any family port runs.
    #[test]
    fn the_caller_authority_admits_the_owner_and_the_mesh_and_nothing_else() {
        let authority = NativeCallerAuthority;
        assert!(authority.verify(&local_admin_claim()).is_ok());
        let refusal = authority
            .verify(&ActorClaim::membership(
                "not-a-provider",
                "conversation:one",
                "membership:one",
            ))
            .expect_err("an unadmitted provider has no caller scope");
        assert_eq!(refusal.code, "subagents_caller_invalid");
        assert_eq!(refusal.stage, CALLER_STAGE);
    }

    #[test]
    fn a_call_without_a_caller_scope_is_refused_rather_than_served() {
        let refusal = caller_context(&local_admin_claim())
            .err()
            .expect("delegated work is always done as a membership");
        assert_eq!(refusal.code, "caller_membership_binding_required");
        assert_eq!(refusal.stage, CALLER_STAGE);
    }

    fn argument_names(arguments: &Map<String, Value>) -> Vec<&str> {
        arguments.keys().map(String::as_str).collect()
    }
}
