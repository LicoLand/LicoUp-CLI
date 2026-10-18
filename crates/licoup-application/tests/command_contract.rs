//! Command encode/decode and failure mapping.
//!
//! Two interfaces decode into these types, so a round trip has to be exact and a
//! malformed command has to be refused before any port is reached. The failure
//! cases here are the ones the product's existing rules depend on: an uncertain
//! effect must reconcile rather than retry, and a permanent failure must not
//! claim the caller can fix it by retrying.

use licoup_application::{
    ActorClaim, ApplicationCommand, ApplicationFacade, ApplicationFailure, ApplicationPorts,
    ArtifactInputRequest, AssistantCommand, CallbackDecision, CommandOutcome, ConversationCommand,
    DependencyDeclarationRequest, DispatchRequest, EffectCertainty, ExportRequest, Operation,
    OperationReference, OperationState, ProjectCommand, ProjectRegistrationRequest, RecoveryAction,
    SearchRequest, SubagentCommand, TaskType,
};
use serde_json::{Value, json};
use std::sync::{Arc, Mutex};

// ---------------------------------------------------------------------------
// Round trips
// ---------------------------------------------------------------------------

#[test]
fn every_command_family_round_trips_through_json_unchanged() {
    let commands = vec![
        ApplicationCommand::Assistant(AssistantCommand::Profiles {
            conversation_id: "conversation:one".into(),
            filters: Some(json!({"role": "worker"})),
        }),
        ApplicationCommand::Assistant(AssistantCommand::WorkflowExecute {
            conversation_id: "conversation:one".into(),
            membership_id: "membership:assistant".into(),
            workflow: json!({"steps": []}),
            bindings: json!([{"valueId": "membership:worker"}]),
            filters: json!({"requiredAuthority": "delegate"}),
            input: None,
            idempotency_key: "idem-1".into(),
            decision: Some(CallbackDecision::Advance {
                state_id: "state:one".into(),
                state_visit: 2,
            }),
        }),
        ApplicationCommand::Assistant(AssistantCommand::WorkflowInspect {
            run_id: "run:one".into(),
        }),
        ApplicationCommand::Assistant(AssistantCommand::WorkflowCancel {
            run_id: "run:one".into(),
        }),
        ApplicationCommand::Subagent(SubagentCommand::List),
        ApplicationCommand::Subagent(SubagentCommand::Probe {
            agent_id: "codex".into(),
        }),
        ApplicationCommand::Subagent(SubagentCommand::Delegate(DispatchRequest {
            conversation_id: Some("conversation:one".into()),
            membership_id: Some("membership:worker".into()),
            agent_id: None,
            prompt: "review the diff".into(),
            model: Some("gpt-5.6-luna".into()),
            reasoning_effort: Some("max".into()),
            working_directory: Some("/synthetic/workspace".into()),
            task_type: Some(TaskType::Backend),
            timeout_ms: Some(60_000),
            timeout_unbounded: false,
            max_stdout_bytes: None,
            max_stderr_bytes: None,
        })),
        ApplicationCommand::Subagent(SubagentCommand::Continue(DispatchRequest {
            agent_id: Some("cursor".into()),
            prompt: "continue".into(),
            ..DispatchRequest::default()
        })),
        ApplicationCommand::Subagent(SubagentCommand::Cancel(licoup_application::CancelRequest {
            conversation_id: Some("conversation:one".into()),
            agent_id: Some("cursor".into()),
            membership_id: None,
        })),
        ApplicationCommand::Conversation(ConversationCommand::List {
            include_archived: true,
        }),
        ApplicationCommand::Conversation(ConversationCommand::Get {
            conversation_id: "conversation:one".into(),
        }),
        ApplicationCommand::Conversation(ConversationCommand::Search(SearchRequest {
            query: "luna reserve".into(),
            limit: 20,
        })),
        ApplicationCommand::Conversation(ConversationCommand::Export(ExportRequest {
            path: "/synthetic/export".into(),
            conversation_ids: vec!["conversation:one".into()],
        })),
        ApplicationCommand::Conversation(ConversationCommand::Import(
            licoup_application::ImportRequest {
                path: "/synthetic/import".into(),
            },
        )),
        ApplicationCommand::Project(ProjectCommand::Register(ProjectRegistrationRequest {
            project_id: "alpha-project".into(),
            display_name: "Synthetic project alpha-project".into(),
            authorized_root: "/synthetic/authorized/root".into(),
            authority_kind: "membership".into(),
            authority_reference: "membership:owner".into(),
            workspace_id: "workspace:shared".into(),
            plan_id: "plan:alpha".into(),
        })),
        ApplicationCommand::Project(ProjectCommand::Read {
            project_id: "alpha-project".into(),
        }),
        ApplicationCommand::Project(ProjectCommand::List),
        ApplicationCommand::Project(ProjectCommand::DeclareDependency(
            DependencyDeclarationRequest {
                project_id: "alpha-project".into(),
                work_item_id: "test".into(),
                artifact: ArtifactInputRequest::Local {
                    producer_work_item_id: "build".into(),
                    path: "dist/out.bin".into(),
                },
            },
        )),
        ApplicationCommand::Project(ProjectCommand::DeclareDependency(
            DependencyDeclarationRequest {
                project_id: "bravo-project".into(),
                work_item_id: "integrate".into(),
                artifact: ArtifactInputRequest::CrossProject {
                    project_id: "alpha-project".into(),
                    work_item_id: "build".into(),
                },
            },
        )),
        ApplicationCommand::Project(ProjectCommand::Dependencies {
            project_id: "alpha-project".into(),
        }),
        ApplicationCommand::Project(ProjectCommand::UnresolvedArtifacts {
            project_id: "alpha-project".into(),
        }),
        ApplicationCommand::Project(ProjectCommand::BlockedConsumers {
            project_id: "alpha-project".into(),
            work_item_id: "build".into(),
        }),
    ];

    for command in commands {
        assert!(command.validate().is_ok(), "{command:?} should validate");
        let encoded = command.encode().expect("encode");
        let decoded = ApplicationCommand::decode(&encoded).expect("decode");
        assert_eq!(decoded, command, "round trip changed {command:?}");
        assert_eq!(decoded.operation(), command.operation());
        assert_eq!(decoded.family(), command.family());
    }
}

#[test]
fn operation_names_are_stable_and_effect_producing_ones_are_marked() {
    // The neutral names are what receipts and references publish, so they are
    // part of the contract rather than a display choice.
    assert_eq!(Operation::SubagentDelegate.as_str(), "subagent.delegate");
    assert_eq!(Operation::WorkflowExecute.as_str(), "workflow.execute");
    assert_eq!(
        Operation::ConversationImport.as_str(),
        "conversation.import"
    );
    assert_eq!(Operation::ProjectRegister.as_str(), "project.register");
    assert_eq!(Operation::ProjectRead.as_str(), "project.read");
    assert_eq!(Operation::ProjectList.as_str(), "project.list");
    assert_eq!(
        Operation::ProjectImportPreview.as_str(),
        "project.import.preview"
    );
    assert_eq!(
        Operation::ProjectImportApply.as_str(),
        "project.import.apply"
    );
    assert_eq!(
        Operation::ProjectDeclareDependency.as_str(),
        "project.declare-dependency"
    );
    assert_eq!(
        Operation::ProjectDependencies.as_str(),
        "project.dependencies"
    );
    assert_eq!(
        Operation::ProjectUnresolvedArtifacts.as_str(),
        "project.unresolved-artifacts"
    );
    assert_eq!(
        Operation::ProjectBlockedConsumers.as_str(),
        "project.blocked-consumers"
    );

    for operation in [
        Operation::WorkflowExecute,
        Operation::WorkflowCancel,
        Operation::SubagentDelegate,
        Operation::SubagentContinue,
        Operation::SubagentCancel,
        Operation::ConversationImport,
    ] {
        assert!(
            operation.produces_effect(),
            "{} changes provider state, so its failures must reason about effect certainty",
            operation.as_str()
        );
    }
    for operation in [
        Operation::AssistantProfiles,
        Operation::SubagentsList,
        Operation::SubagentProbe,
        Operation::WorkflowInspect,
        Operation::ConversationList,
        Operation::ConversationGet,
        Operation::ConversationSearch,
        Operation::ConversationExport,
        Operation::ProjectRead,
        Operation::ProjectList,
        Operation::ProjectImportPreview,
        Operation::ProjectImportApply,
        Operation::ProjectDependencies,
        Operation::ProjectUnresolvedArtifacts,
        Operation::ProjectBlockedConsumers,
    ] {
        assert!(
            !operation.produces_effect(),
            "{} is a read and must not claim an effect",
            operation.as_str()
        );
    }
    // Registration writes this product's own durable record and never touches a
    // provider, and a repeated identity is refused rather than replayed, so it
    // claims neither a provider effect nor the reconciliation that follows one.
    // Declaring an input is the same kind of write.
    assert!(!Operation::ProjectRegister.produces_effect());
    assert!(!Operation::ProjectDeclareDependency.produces_effect());
    assert_eq!(
        ApplicationCommand::Project(ProjectCommand::Register(ProjectRegistrationRequest {
            project_id: "alpha-project".into(),
            display_name: "Synthetic project alpha-project".into(),
            authorized_root: "/synthetic/authorized/root".into(),
            authority_kind: "membership".into(),
            authority_reference: "membership:owner".into(),
            workspace_id: "workspace:shared".into(),
            plan_id: "plan:alpha".into(),
        }))
        .family(),
        licoup_application::CommandFamily::Project
    );
}

// ---------------------------------------------------------------------------
// Structural validation
// ---------------------------------------------------------------------------

#[test]
fn malformed_commands_are_refused_before_any_port_runs() {
    let cases: Vec<(ApplicationCommand, &str)> = vec![
        (
            ApplicationCommand::Conversation(ConversationCommand::Get {
                conversation_id: "  ".into(),
            }),
            "conversation_id",
        ),
        (
            ApplicationCommand::Subagent(SubagentCommand::Delegate(DispatchRequest {
                membership_id: None,
                agent_id: None,
                prompt: "work".into(),
                ..DispatchRequest::default()
            })),
            "target",
        ),
        (
            ApplicationCommand::Subagent(SubagentCommand::Delegate(DispatchRequest {
                agent_id: Some("codex".into()),
                prompt: "  ".into(),
                ..DispatchRequest::default()
            })),
            "prompt",
        ),
        (
            ApplicationCommand::Subagent(SubagentCommand::Delegate(DispatchRequest {
                agent_id: Some("codex".into()),
                prompt: "work".into(),
                working_directory: Some("relative/path".into()),
                ..DispatchRequest::default()
            })),
            "working_directory",
        ),
        (
            ApplicationCommand::Subagent(SubagentCommand::Probe {
                agent_id: "Not A Provider".into(),
            }),
            "agent_id",
        ),
        (
            ApplicationCommand::Conversation(ConversationCommand::Search(SearchRequest {
                query: "ok".into(),
                limit: 0,
            })),
            "limit",
        ),
        (
            ApplicationCommand::Conversation(ConversationCommand::Export(ExportRequest {
                path: "/synthetic/export".into(),
                conversation_ids: vec![],
            })),
            "conversation_ids",
        ),
        (
            ApplicationCommand::Assistant(AssistantCommand::WorkflowExecute {
                conversation_id: "conversation:one".into(),
                membership_id: "membership:assistant".into(),
                workflow: json!("not an object"),
                bindings: json!([]),
                filters: Value::Null,
                input: None,
                idempotency_key: "idem".into(),
                decision: None,
            }),
            "workflow",
        ),
        (
            ApplicationCommand::Assistant(AssistantCommand::WorkflowExecute {
                conversation_id: "conversation:one".into(),
                membership_id: "membership:assistant".into(),
                workflow: json!({}),
                bindings: json!([]),
                filters: Value::Null,
                input: None,
                idempotency_key: "idem".into(),
                decision: Some(CallbackDecision::Return {
                    state_id: "state:one".into(),
                    state_visit: 0,
                }),
            }),
            "callback_state_visit",
        ),
        (
            ApplicationCommand::Project(ProjectCommand::ImportPreview {
                document: json!("not a document"),
            }),
            "document",
        ),
        (
            ApplicationCommand::Project(ProjectCommand::ImportApply {
                document: Value::Null,
                expected_revision: 0,
            }),
            "document",
        ),
        (
            ApplicationCommand::Project(ProjectCommand::DeclareDependency(
                DependencyDeclarationRequest {
                    project_id: "alpha-project".into(),
                    work_item_id: "  ".into(),
                    artifact: ArtifactInputRequest::Local {
                        producer_work_item_id: "build".into(),
                        path: "dist/out.bin".into(),
                    },
                },
            )),
            "work_item_id",
        ),
        (
            ApplicationCommand::Project(ProjectCommand::DeclareDependency(
                DependencyDeclarationRequest {
                    project_id: "alpha-project".into(),
                    work_item_id: "test".into(),
                    artifact: ArtifactInputRequest::Local {
                        producer_work_item_id: "build".into(),
                        path: "  ".into(),
                    },
                },
            )),
            "path",
        ),
        (
            ApplicationCommand::Project(ProjectCommand::DeclareDependency(
                DependencyDeclarationRequest {
                    project_id: "alpha-project".into(),
                    work_item_id: "test".into(),
                    artifact: ArtifactInputRequest::CrossProject {
                        project_id: String::new(),
                        work_item_id: "build".into(),
                    },
                },
            )),
            "project_id",
        ),
        (
            ApplicationCommand::Project(ProjectCommand::UnresolvedArtifacts {
                project_id: String::new(),
            }),
            "project_id",
        ),
        (
            ApplicationCommand::Project(ProjectCommand::BlockedConsumers {
                project_id: "alpha-project".into(),
                work_item_id: String::new(),
            }),
            "work_item_id",
        ),
    ];

    for (command, field) in cases {
        let failure = command
            .validate()
            .expect_err(&format!("{command:?} must be refused"));
        assert_eq!(failure.code, "invalid_request");
        assert_eq!(failure.stage, "schema/validate");
        assert_eq!(failure.field.as_deref(), Some(field));
        assert_eq!(failure.effect, EffectCertainty::NotAttempted);
    }
}

#[test]
fn an_undecodable_command_is_refused_rather_than_partially_accepted() {
    for value in [
        json!({"family": "subagent", "command": "delegate"}),
        json!({"family": "assistant", "command": "workflow-execute"}),
        json!({"family": "unknown", "command": "list"}),
        json!("not an object"),
    ] {
        let failure = ApplicationCommand::decode(&value).expect_err("must not decode");
        assert_eq!(failure.code, "invalid_request");
        assert_eq!(failure.effect, EffectCertainty::NotAttempted);
    }
}

/// A declared artifact reference has exactly two shapes at the surface too. A
/// payload that names a third shape, omits the producer, or carries a field
/// this product did not declare is refused rather than interpreted.
#[test]
fn a_declaration_payload_must_name_one_of_the_two_declared_artifact_shapes() {
    for artifact in [
        json!({"kind": "discovered", "path": "dist/out.bin"}),
        json!({"kind": "local", "path": "dist/out.bin"}),
        json!({
            "kind": "local",
            "producerWorkItemId": "build",
            "path": "dist/out.bin",
            "credential": "synthetic-secret-material",
        }),
        json!({"kind": "cross-project", "projectId": "alpha-project"}),
    ] {
        let value = json!({
            "family": "project",
            "command": "declare-dependency",
            "projectId": "alpha-project",
            "workItemId": "test",
            "artifact": artifact,
        });
        let failure = ApplicationCommand::decode(&value).expect_err("must not decode");
        assert_eq!(failure.code, "invalid_request");
    }

    let decoded = ApplicationCommand::decode(&json!({
        "family": "project",
        "command": "declare-dependency",
        "projectId": "alpha-project",
        "workItemId": "test",
        "artifact": {
            "kind": "local",
            "producerWorkItemId": "build",
            "path": "dist/out.bin",
        },
    }))
    .expect("one declared shape decodes");
    assert_eq!(decoded.operation(), Operation::ProjectDeclareDependency);
    assert_eq!(decoded.family(), licoup_application::CommandFamily::Project);
    assert_eq!(decoded.validate(), Ok(()));
}

// ---------------------------------------------------------------------------
// Failure mapping
// ---------------------------------------------------------------------------

#[test]
fn an_uncertain_effect_always_requires_reconciliation() {
    let failure = ApplicationFailure::uncertain("dispatch_uncertain", "dispatch/reconcile");
    assert_eq!(failure.effect, EffectCertainty::Uncertain);
    assert_eq!(failure.recovery, RecoveryAction::ReconcileBeforeRetry);
    assert!(
        failure.retryable,
        "uncertain work may still be retried, after reconciling"
    );
    assert!(failure.requires_reconciliation());
    assert!(
        !failure.safe_to_retry(),
        "an uncertain effect must never look safe to retry on its own"
    );
}

#[test]
fn declaring_an_uncertain_effect_cannot_skip_reconciliation() {
    // A caller that knows only "something may have happened" cannot also claim
    // the retry is safe: the durable record decides that.
    let failure = ApplicationFailure::permanent("dispatch_failed", "dispatch/execute")
        .with_effect(EffectCertainty::Uncertain);
    assert_eq!(failure.recovery, RecoveryAction::ReconcileBeforeRetry);
    assert!(failure.retryable);
    assert!(failure.requires_reconciliation());
}

#[test]
fn permanent_and_retryable_failures_keep_their_own_recovery() {
    let permanent = ApplicationFailure::permanent("subagent_adapter_unavailable", "adapter/select");
    assert!(!permanent.retryable);
    assert_eq!(permanent.effect, EffectCertainty::NotAttempted);
    assert!(!permanent.requires_reconciliation());
    assert_eq!(permanent.recovery, RecoveryAction::CorrectRequest);

    let retryable =
        ApplicationFailure::retryable("conversation_state_unavailable", "conversation/store");
    assert!(retryable.safe_to_retry());
    assert_eq!(retryable.recovery, RecoveryAction::RetryAfterRecovery);
}

#[test]
fn each_interface_projects_recovery_onto_its_own_vocabulary() {
    // The CLI vocabulary has no reconcile value: an uncertain outcome surfaces
    // as a result to review, which is what that surface already does.
    assert_eq!(
        RecoveryAction::ReconcileBeforeRetry.cli_wire(),
        "review_terminal_result"
    );
    assert_eq!(
        RecoveryAction::ReconcileBeforeRetry.mcp_wire(),
        "reconcile_before_retry"
    );
    assert_eq!(
        RecoveryAction::CorrectArguments.cli_wire(),
        "correct_command_arguments"
    );
    assert_eq!(
        RecoveryAction::CorrectArguments.mcp_wire(),
        "correct_request_and_retry"
    );
    assert_eq!(
        RecoveryAction::ReduceArguments.cli_wire(),
        "reduce_command_arguments"
    );
}

#[test]
fn normalization_applies_the_products_retry_and_recovery_pairing() {
    assert_eq!(
        licoup_application::FailureNormalization::UNCERTAIN
            .into_failure("x", "y")
            .recovery,
        RecoveryAction::ReconcileBeforeRetry
    );
    assert_eq!(
        licoup_application::FailureNormalization::RETRYABLE
            .into_failure("x", "y")
            .recovery,
        RecoveryAction::RetryAfterRecovery
    );
    assert_eq!(
        licoup_application::FailureNormalization::PERMANENT
            .into_failure("x", "y")
            .recovery,
        RecoveryAction::CorrectRequest
    );
}

// ---------------------------------------------------------------------------
// Operation references
// ---------------------------------------------------------------------------

#[test]
fn a_reference_carries_the_identity_both_interfaces_must_agree_on() {
    let reference = OperationReference::new(
        Operation::SubagentDelegate,
        "dispatch:one",
        OperationState::Accepted,
    )
    .with_conversation("conversation:one")
    .with_membership("membership:worker")
    .with_depth(1)
    .with_idempotency_key("idem-1");

    assert_eq!(reference.operation, "subagent.delegate");
    assert_eq!(reference.state.as_str(), "accepted");
    assert!(reference.state.is_live());
    assert!(!reference.state.is_terminal());

    // The same operation observed through the other interface compares equal,
    // even though the states differ: identity is not the state.
    let observed = OperationReference::new(
        Operation::SubagentDelegate,
        "dispatch:one",
        OperationState::Processing,
    )
    .with_conversation("conversation:one")
    .with_membership("membership:worker");
    assert!(
        observed.same_operation(&reference),
        "one operation reported twice must compare equal across state changes"
    );

    // Different operations, or the same id in a different conversation, are not
    // the same thing and must not compare equal.
    let other_operation = OperationReference::new(
        Operation::SubagentContinue,
        "dispatch:one",
        OperationState::Processing,
    )
    .with_conversation("conversation:one")
    .with_membership("membership:worker");
    assert!(!other_operation.same_operation(&reference));

    let other_conversation = OperationReference::new(
        Operation::SubagentDelegate,
        "dispatch:one",
        OperationState::Processing,
    )
    .with_conversation("conversation:two")
    .with_membership("membership:worker");
    assert!(!other_conversation.same_operation(&reference));
}

#[test]
fn states_are_live_or_terminal_but_never_both() {
    for state in [
        OperationState::Accepted,
        OperationState::Processing,
        OperationState::Responding,
        OperationState::CancelRequested,
        OperationState::Cancelled,
        OperationState::Completed,
        OperationState::Failed,
        OperationState::ReconciliationRequired,
    ] {
        assert!(
            state.is_live() ^ state.is_terminal(),
            "{state:?} must be exactly one of live or terminal"
        );
    }
    assert_eq!(
        OperationState::ReconciliationRequired.as_str(),
        "reconciliation-required"
    );
}

// ---------------------------------------------------------------------------
// Facade ordering
// ---------------------------------------------------------------------------

/// Records what each port was asked, so ordering can be asserted rather than
/// assumed.
#[derive(Default)]
struct Trace {
    calls: Mutex<Vec<String>>,
}

#[derive(Clone)]
struct FixturePorts {
    trace: Arc<Trace>,
    verify_fails: bool,
}

impl FixturePorts {
    fn record(&self, entry: impl Into<String>) {
        self.trace.calls.lock().unwrap().push(entry.into());
    }
    fn calls(&self) -> Vec<String> {
        self.trace.calls.lock().unwrap().clone()
    }
}

impl licoup_application::ActorPort for FixturePorts {
    fn verify(&self, _claim: &ActorClaim) -> Result<(), ApplicationFailure> {
        self.record("verify");
        if self.verify_fails {
            return Err(ApplicationFailure::permanent(
                "caller_membership_not_authorized",
                "conversation/authorize",
            ));
        }
        Ok(())
    }
}

impl licoup_application::AssistantPort for FixturePorts {
    fn execute(
        &self,
        _claim: &ActorClaim,
        command: &AssistantCommand,
    ) -> Result<CommandOutcome, ApplicationFailure> {
        self.record("assistant");
        Ok(CommandOutcome::new(OperationReference::new(
            command.operation(),
            "run:one",
            OperationState::Processing,
        )))
    }
}

impl licoup_application::SubagentPort for FixturePorts {
    fn execute(
        &self,
        _claim: &ActorClaim,
        command: &SubagentCommand,
    ) -> Result<CommandOutcome, ApplicationFailure> {
        self.record("subagent");
        Ok(CommandOutcome::new(OperationReference::new(
            command.operation(),
            "dispatch:one",
            OperationState::Accepted,
        )))
    }
}

impl licoup_application::ProjectPort for FixturePorts {
    fn execute(
        &self,
        _claim: &ActorClaim,
        command: &ProjectCommand,
    ) -> Result<CommandOutcome, ApplicationFailure> {
        self.record("project");
        Ok(CommandOutcome::new(OperationReference::new(
            command.operation(),
            "alpha-project",
            OperationState::Completed,
        )))
    }
}

impl licoup_application::ConversationPort for FixturePorts {
    fn execute(
        &self,
        _claim: &ActorClaim,
        command: &ConversationCommand,
    ) -> Result<CommandOutcome, ApplicationFailure> {
        self.record("conversation");
        Ok(CommandOutcome::read(
            command.operation(),
            json!({"command": format!("{:?}", command.operation())}),
        ))
    }
}

fn facade(verify_fails: bool) -> (ApplicationFacade, FixturePorts) {
    let ports = FixturePorts {
        trace: Arc::new(Trace::default()),
        verify_fails,
    };
    let application = ApplicationPorts::new(
        Arc::new(ports.clone()),
        Arc::new(ports.clone()),
        Arc::new(ports.clone()),
        Arc::new(ports.clone()),
        Arc::new(ports.clone()),
    );
    (ApplicationFacade::new(application), ports)
}

#[test]
fn the_facade_verifies_once_and_only_then_reaches_the_family_port() {
    let (facade, ports) = facade(false);
    let claim = ActorClaim::membership("codex", "conversation:one", "membership:codex");
    let command = ApplicationCommand::Subagent(SubagentCommand::List);

    let outcome = facade.execute(&claim, &command).expect("dispatch");
    assert_eq!(outcome.reference.operation, "subagent.list");
    assert_eq!(
        ports.calls(),
        vec!["verify".to_owned(), "subagent".to_owned()],
        "verification must happen exactly once, before the family port"
    );
}

#[test]
fn a_refused_claim_never_reaches_a_family_port() {
    let (facade, ports) = facade(true);
    let claim = ActorClaim::membership("codex", "conversation:one", "membership:codex");
    let command = ApplicationCommand::Conversation(ConversationCommand::List {
        include_archived: false,
    });

    let failure = facade
        .execute(&claim, &command)
        .expect_err("must be refused");
    assert_eq!(failure.code, "caller_membership_not_authorized");
    assert_eq!(
        ports.calls(),
        vec!["verify".to_owned()],
        "no family port may run for an unverified claim"
    );
}

#[test]
fn a_malformed_command_is_refused_without_verifying_or_executing() {
    let (facade, ports) = facade(false);
    let claim = ActorClaim::membership("codex", "conversation:one", "membership:codex");
    let command = ApplicationCommand::Conversation(ConversationCommand::Get {
        conversation_id: "   ".into(),
    });

    let failure = facade
        .execute(&claim, &command)
        .expect_err("must be refused");
    assert_eq!(failure.code, "invalid_request");
    assert!(
        ports.calls().is_empty(),
        "a malformed command must cost nothing: {:?}",
        ports.calls()
    );
}

#[test]
fn a_claim_bound_to_another_conversation_is_refused_before_verification() {
    let (facade, ports) = facade(false);
    let claim = ActorClaim::membership("codex", "conversation:one", "membership:codex");
    let command = ApplicationCommand::Conversation(ConversationCommand::Get {
        conversation_id: "conversation:two".into(),
    });

    let failure = facade
        .execute(&claim, &command)
        .expect_err("must be refused");
    assert_eq!(failure.code, "actor_conversation_mismatch");
    assert!(
        ports.calls().is_empty(),
        "binding is decided before verification"
    );
}

#[test]
fn an_invalid_claim_shape_is_refused_before_anything_else() {
    let (facade, ports) = facade(false);
    let claim = ActorClaim::membership("Not A Provider", "conversation:one", "membership:codex");
    let command = ApplicationCommand::Conversation(ConversationCommand::List {
        include_archived: false,
    });

    let failure = facade
        .execute(&claim, &command)
        .expect_err("must be refused");
    assert_eq!(failure.code, "actor_provider_invalid");
    assert!(ports.calls().is_empty());
}

#[test]
fn a_local_admin_claim_is_not_conversation_scoped() {
    // The in-process caller owns the whole state root, so it can address any
    // conversation; that is the deliberate asymmetry the product already has.
    let (facade, ports) = facade(false);
    let claim = ActorClaim::local_admin("membership:owner");
    let command = ApplicationCommand::Conversation(ConversationCommand::Get {
        conversation_id: "conversation:whatever".into(),
    });

    assert!(facade.execute(&claim, &command).is_ok());
    assert_eq!(
        ports.calls(),
        vec!["verify".to_owned(), "conversation".to_owned()]
    );
}

#[test]
fn a_caller_scope_that_names_no_conversation_is_not_bound_to_one() {
    // An inventory and a readiness probe are not made in a conversation, so the
    // caller scope that reaches them carries none. The claim is still a
    // membership claim: it names the provider the work is done as, which is what
    // the native caller authority has to admit.
    let (facade, ports) = facade(false);
    let claim = ActorClaim::Membership {
        provider_id: "codex".into(),
        conversation_id: None,
        membership_id: None,
        parent_dispatch_id: None,
    };
    assert_eq!(claim.conversation_id(), None);
    assert_eq!(claim.membership_id(), None);
    assert!(!claim.is_local_admin());

    let command = ApplicationCommand::Subagent(SubagentCommand::List);
    assert!(facade.execute(&claim, &command).is_ok());
    assert_eq!(
        ports.calls(),
        vec!["verify".to_owned(), "subagent".to_owned()]
    );
}

#[test]
fn a_caller_scope_bound_elsewhere_still_binds_before_verification() {
    // A claim that names no conversation is admitted here and refused by the
    // domain owner — the only place that can see whether such a caller has a
    // binding. A claim that does name one still binds first, as before.
    let (facade, ports) = facade(false);
    let claim = ActorClaim::Membership {
        provider_id: "codex".into(),
        conversation_id: Some("conversation:one".into()),
        membership_id: Some("membership:codex".into()),
        parent_dispatch_id: None,
    };
    let command = ApplicationCommand::Conversation(ConversationCommand::Get {
        conversation_id: "conversation:two".into(),
    });

    let failure = facade
        .execute(&claim, &command)
        .expect_err("a bound claim may not leave its conversation");
    assert_eq!(failure.code, "actor_conversation_mismatch");
    assert!(ports.calls().is_empty());
}

#[test]
fn a_local_admin_dispatch_names_its_conversation() {
    // The in-process caller has no conversation of its own, so the dispatch
    // itself has to name one — without this the CLI could not delegate at all.
    let (facade, ports) = facade(false);
    let claim = ActorClaim::local_admin("membership:owner");
    let command = ApplicationCommand::Subagent(SubagentCommand::Delegate(DispatchRequest {
        conversation_id: Some("conversation:one".into()),
        agent_id: Some("codex".into()),
        prompt: "review the diff".into(),
        ..DispatchRequest::default()
    }));

    assert!(command.validate().is_ok());
    assert_eq!(command.conversation_id(), Some("conversation:one"));
    assert!(facade.execute(&claim, &command).is_ok());
    assert_eq!(
        ports.calls(),
        vec!["verify".to_owned(), "subagent".to_owned()]
    );
}

#[test]
fn a_membership_cannot_dispatch_into_another_conversation() {
    // The native tool accepts an optional conversationId and rejects a mismatch
    // with subagent_cross_conversation_rejected. The facade must refuse the same
    // thing before verification, so neither interface can reach a port with a
    // cross-conversation dispatch.
    let (facade, ports) = facade(false);
    let claim = ActorClaim::membership("codex", "conversation:one", "membership:codex");
    let command = ApplicationCommand::Subagent(SubagentCommand::Delegate(DispatchRequest {
        conversation_id: Some("conversation:two".into()),
        agent_id: Some("cursor".into()),
        prompt: "work elsewhere".into(),
        ..DispatchRequest::default()
    }));

    let failure = facade
        .execute(&claim, &command)
        .expect_err("must be refused");
    assert_eq!(failure.code, "actor_conversation_mismatch");
    assert!(
        ports.calls().is_empty(),
        "binding is decided before verification"
    );
}

#[test]
fn a_membership_dispatch_into_its_own_conversation_is_allowed() {
    let (facade, _) = facade(false);
    let claim = ActorClaim::membership("codex", "conversation:one", "membership:codex");
    let command = ApplicationCommand::Subagent(SubagentCommand::Delegate(DispatchRequest {
        conversation_id: Some("conversation:one".into()),
        agent_id: Some("cursor".into()),
        prompt: "review the diff".into(),
        ..DispatchRequest::default()
    }));

    assert!(facade.execute(&claim, &command).is_ok());
}

#[test]
fn a_working_directory_is_judged_by_the_platforms_own_rule() {
    // A Unix-shaped `starts_with('/')` check would reject every Windows path;
    // the native dispatch path uses the platform rule, so this must too.
    let absolute = std::env::temp_dir()
        .join("licoup-contract-workspace")
        .to_string_lossy()
        .into_owned();
    assert!(
        ApplicationCommand::Subagent(SubagentCommand::Delegate(DispatchRequest {
            agent_id: Some("codex".into()),
            prompt: "work".into(),
            working_directory: Some(absolute),
            ..DispatchRequest::default()
        }))
        .validate()
        .is_ok(),
        "an absolute platform-native path must be accepted"
    );

    let relative = ApplicationCommand::Subagent(SubagentCommand::Delegate(DispatchRequest {
        agent_id: Some("codex".into()),
        prompt: "work".into(),
        working_directory: Some("workspace".into()),
        ..DispatchRequest::default()
    }));
    assert_eq!(
        relative
            .validate()
            .expect_err("relative must be refused")
            .field
            .as_deref(),
        Some("working_directory")
    );
}

#[test]
fn a_read_returns_a_payload_without_claiming_a_live_operation() {
    let (facade, _) = facade(false);
    let claim = ActorClaim::local_admin("membership:owner");
    let command = ApplicationCommand::Conversation(ConversationCommand::List {
        include_archived: false,
    });

    let outcome = facade.execute(&claim, &command).expect("read");
    assert!(!outcome.is_live(), "a read has no live identity to follow");
    assert!(!outcome.payload.is_null(), "a read carries its payload");
}
