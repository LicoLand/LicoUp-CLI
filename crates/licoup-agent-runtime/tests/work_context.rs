use licoup_agent_runtime::work_context::{
    CapabilityProfile, ChildBinding, ContinuityEffectClass, ContinuityFailureCode, CoordinatorKind,
    ForkInheritance, GoalInference, HermeticProtocol, IsolationVerdict, LateResult,
    NativeAttemptRef, NativeCapabilitySupport, NativeControlRequest, NativeWorkContextKey,
    NativeWorkContextPort, OperationKind, ProtocolOutcome, SafeReason, SessionPresence, TurnExit,
    WorkContextConfig, WorkContextRuntime, unavailable_work_context_port,
};

fn child() -> ChildBinding {
    ChildBinding {
        child_conversation_id: "conversation:child".into(),
        membership_id: "membership:child-assistant".into(),
        source_task_id: "goal:source-task".into(),
        parent_conversation_id: "conversation:parent".into(),
    }
}

fn config(knowledge: bool) -> WorkContextConfig {
    WorkContextConfig::child(child()).with_knowledge_injected(knowledge)
}

fn key(matter: &str, generation: i64) -> NativeWorkContextKey {
    NativeWorkContextKey {
        conversation_id: child().child_conversation_id,
        membership_id: child().membership_id,
        matter_id: matter.into(),
        generation,
    }
}

fn lost_runtime() -> WorkContextRuntime {
    WorkContextRuntime::from_hermetic(
        HermeticProtocol::codex(CapabilityProfile::High).with_presence(SessionPresence::Lost),
        config(false),
    )
}

#[test]
fn ac_02_005_lost_resume_then_explicit_rehydrate_keeps_distinct_operations() {
    let runtime = lost_runtime();
    let source = key("matter:docs", 1);
    let failure = runtime.exact_resume(&source).unwrap_err();
    assert_eq!(failure.code, ContinuityFailureCode::NativeBindingLost);
    assert_eq!(failure.effect_class, ContinuityEffectClass::None);
    let resume_ops = runtime.operations().unwrap();
    assert_eq!(resume_ops.len(), 1);
    assert_eq!(resume_ops[0].kind, OperationKind::ExactResume);
    assert!(!resume_ops[0].succeeded);
    assert_eq!(resume_ops[0].protocol_method, "thread/resume");
    assert_eq!(resume_ops[0].binding_generation, 1);

    let rebuilt = runtime.rehydrate(&source).unwrap();
    assert_eq!(rebuilt, 2);
    let operations = runtime.operations().unwrap();
    assert_eq!(operations.len(), 2);
    assert_ne!(operations[0].operation_id, operations[1].operation_id);
    assert_ne!(
        operations[0].binding_generation,
        operations[1].binding_generation
    );
    assert_eq!(operations[1].kind, OperationKind::Rehydrate);
    assert_eq!(operations[1].protocol_method, "thread/start");
    assert_ne!(operations[1].protocol_method, operations[0].protocol_method);
    let checkpoint = operations[1].source_checkpoint.as_ref().unwrap();
    assert_eq!(
        checkpoint.failed_operation_id.as_deref(),
        Some(operations[0].operation_id.as_str())
    );
    let handoff = &runtime.handoffs().unwrap()[0];
    assert_eq!(handoff.from_operation_id, operations[0].operation_id);
    assert_eq!(handoff.to_operation_id, operations[1].operation_id);
    assert_eq!(handoff.from_generation, 1);
    assert_eq!(handoff.to_generation, 2);
    assert!(
        runtime.bindings().unwrap().iter().any(
            |binding| binding.binding_generation == 2 && binding.key.matter_id == "matter:docs"
        )
    );
}

#[test]
fn ac_02_006_single_session_knowledge_queues_second_matter_without_session_ids() {
    let runtime = WorkContextRuntime::hermetic_codex(CapabilityProfile::Low, config(true));
    let first = key("matter:a", 1);
    let second = key("matter:b", 1);
    runtime.claim_writer(&first).unwrap();
    let conflict = runtime.claim_writer(&second).unwrap_err();
    assert_eq!(conflict.code, ContinuityFailureCode::WriterBusy);
    assert_eq!(
        runtime.queued_matters().unwrap(),
        vec!["matter:b".to_string()]
    );
    let reasons = runtime.safe_log().unwrap();
    assert!(reasons.contains(&SafeReason::WriterBusy));
    assert!(reasons.contains(&SafeReason::Queued));
    assert!(reasons.contains(&SafeReason::NativeIsolationUnverified));
    for reason in &reasons {
        let token = reason.as_str();
        assert!(!token.contains("session"));
        assert!(!token.contains("thread"));
        assert!(!token.contains('/'));
    }
    assert!(!runtime.isolation_review().claims_clean());
}

#[test]
fn exact_resume_never_starts_a_new_thread() {
    let runtime = lost_runtime();
    let source = key("matter:docs", 1);
    let _ = runtime.exact_resume(&source);
    let operations = runtime.operations().unwrap();
    assert!(
        operations
            .iter()
            .all(|op| op.protocol_method != "thread/start")
    );
}

#[test]
fn cache_miss_does_not_change_identity() {
    let runtime = WorkContextRuntime::from_hermetic(
        HermeticProtocol::codex(CapabilityProfile::Low)
            .with_knowledge_injected(false)
            .with_presence(SessionPresence::Present),
        config(false),
    );
    let source = key("matter:docs", 4);
    runtime.exact_resume(&source).unwrap();
    let binding = runtime.bindings().unwrap().pop().unwrap();
    assert_eq!(binding.binding_generation, 4);
    assert_eq!(binding.key.matter_id, "matter:docs");
    assert_eq!(binding.fidelity.author.membership_id, child().membership_id);
}

#[test]
fn fidelity_survives_rehydrate_and_keeps_author() {
    let runtime = lost_runtime();
    let source = key("matter:docs", 1);
    let _ = runtime.exact_resume(&source);
    let _ = runtime.rehydrate(&source).unwrap();
    let fidelity = runtime.fidelity();
    assert_eq!(fidelity.tools.name, "codex.tools");
    assert_eq!(fidelity.config.name, "codex.config");
    assert_eq!(fidelity.skills.name, "codex.skills");
    assert_eq!(fidelity.hooks.name, "codex.hooks");
    assert_eq!(fidelity.model.name, "codex.model");
    assert_eq!(fidelity.approval.name, "codex.approval");
    assert_eq!(fidelity.environment.name, "codex.environment");
    assert_eq!(fidelity.author.membership_id, child().membership_id);
    assert_eq!(
        fidelity.author.conversation_id,
        child().child_conversation_id
    );
    assert_eq!(runtime.source_task_id(), "goal:source-task");
}

#[test]
fn parent_or_sibling_keys_are_identity_conflicts() {
    let runtime = lost_runtime();
    let parent = NativeWorkContextKey {
        conversation_id: "conversation:parent".into(),
        membership_id: child().membership_id,
        matter_id: "matter:docs".into(),
        generation: 1,
    };
    assert_eq!(
        runtime.negotiate(&parent).unwrap_err().code,
        ContinuityFailureCode::IdentityConflict
    );
    let other_child = NativeWorkContextKey {
        conversation_id: "conversation:child-b".into(),
        membership_id: child().membership_id,
        matter_id: "matter:docs".into(),
        generation: 1,
    };
    assert_eq!(
        runtime.exact_resume(&other_child).unwrap_err().code,
        ContinuityFailureCode::IdentityConflict
    );
}

#[test]
fn late_result_from_a_cannot_land_in_b_or_parent() {
    let runtime = WorkContextRuntime::hermetic_codex(CapabilityProfile::High, config(false));
    let attempt = NativeAttemptRef {
        conversation_id: "conversation:child-b".into(),
        membership_id: child().membership_id,
        matter_id: "matter:b".into(),
        generation: 1,
        attempt: 1,
    };
    let leaked = LateResult {
        source_matter_id: "matter:a".into(),
        source_conversation_id: "conversation:parent".into(),
        source_generation: 1,
        attempt,
    };
    assert_eq!(
        runtime.admit_late_result(leaked).unwrap_err().code,
        ContinuityFailureCode::IdentityConflict
    );
    let accepted = LateResult {
        source_matter_id: "matter:a".into(),
        source_conversation_id: child().child_conversation_id.clone(),
        source_generation: 1,
        attempt: NativeAttemptRef {
            conversation_id: child().child_conversation_id,
            membership_id: child().membership_id,
            matter_id: "matter:a".into(),
            generation: 1,
            attempt: 2,
        },
    };
    runtime.admit_late_result(accepted).unwrap();
    assert_eq!(runtime.admitted_late_results().unwrap().len(), 1);
}

#[test]
fn attempts_reuse_one_task_identity_and_do_not_change_author() {
    let runtime = WorkContextRuntime::from_hermetic(
        HermeticProtocol::codex(CapabilityProfile::High),
        config(false).with_coordinator(CoordinatorKind::DesignatedAssistant),
    );
    let first = runtime.pin_attempt(&key("matter:docs", 1)).unwrap();
    let second = runtime.pin_attempt(&key("matter:docs", 1)).unwrap();
    assert_eq!(first.attempt, 1);
    assert_eq!(second.attempt, 2);
    assert_eq!(first.matter_id, second.matter_id);
    assert_eq!(runtime.source_task_id(), "goal:source-task");
    assert_eq!(runtime.coordinator(), CoordinatorKind::DesignatedAssistant);
    assert_eq!(
        runtime.fidelity().author.membership_id,
        child().membership_id
    );
}

#[test]
fn delegated_coordinator_must_already_be_admitted() {
    let runtime = WorkContextRuntime::from_hermetic(
        HermeticProtocol::codex(CapabilityProfile::High),
        config(false)
            .with_coordinator(CoordinatorKind::DelegatedCoordinator)
            .with_admitted(vec![child().membership_id, "membership:delegated".into()]),
    );
    let foreign = NativeWorkContextKey {
        conversation_id: child().child_conversation_id,
        membership_id: "membership:new-role".into(),
        matter_id: "matter:docs".into(),
        generation: 1,
    };
    assert_eq!(
        runtime.claim_writer(&foreign).unwrap_err().code,
        ContinuityFailureCode::IdentityConflict
    );
}

#[test]
fn unknown_cancel_and_eof_are_not_terminal_success() {
    let runtime = WorkContextRuntime::from_hermetic(
        HermeticProtocol::codex(CapabilityProfile::High).with_scripted_cancel(
            ProtocolOutcome::unknown("turn/interrupt", {
                use licoup_agent_runtime::work_context::reconciliation_required;
                reconciliation_required()
            }),
        ),
        config(false),
    );
    let failure = runtime
        .cancel(&NativeControlRequest::cancel(
            key("matter:docs", 1),
            "turn:host",
            "turn:native",
        ))
        .unwrap_err();
    assert_eq!(failure.code, ContinuityFailureCode::ReconciliationRequired);
    assert_eq!(failure.effect_class, ContinuityEffectClass::Unknown);
    assert_eq!(
        runtime.turn_exit_goal_inference(TurnExit::Eof),
        GoalInference::NotInferred
    );
    assert_eq!(
        runtime.turn_exit_goal_inference(TurnExit::Completed),
        GoalInference::NotInferred
    );
}

#[test]
fn disconnect_and_unknown_session_fail_typed() {
    let disconnected = WorkContextRuntime::from_hermetic(
        HermeticProtocol::pi(CapabilityProfile::High).with_scripted_resume(
            ProtocolOutcome::unknown(
                "session/resume",
                licoup_agent_runtime::work_context::reconciliation_required(),
            ),
        ),
        config(false),
    );
    let failure = disconnected
        .exact_resume(&key("matter:docs", 1))
        .unwrap_err();
    assert_eq!(failure.code, ContinuityFailureCode::ReconciliationRequired);
    let unknown = WorkContextRuntime::from_hermetic(
        HermeticProtocol::pi(CapabilityProfile::High).with_presence(SessionPresence::Unknown),
        config(false),
    );
    assert_eq!(
        unknown
            .exact_resume(&key("matter:docs", 1))
            .unwrap_err()
            .code,
        ContinuityFailureCode::NativeBindingLost
    );
}

#[test]
fn double_write_on_same_binding_is_writer_busy() {
    let runtime = WorkContextRuntime::hermetic_codex(CapabilityProfile::High, config(false));
    let source = key("matter:docs", 1);
    runtime.claim_writer(&source).unwrap();
    assert_eq!(
        runtime.claim_writer(&source).unwrap_err().code,
        ContinuityFailureCode::WriterBusy
    );
}

#[test]
fn high_capability_can_run_parallel_matters() {
    let runtime = WorkContextRuntime::hermetic_codex(CapabilityProfile::High, config(false));
    runtime.claim_writer(&key("matter:a", 1)).unwrap();
    runtime.claim_writer(&key("matter:b", 1)).unwrap();
    assert!(runtime.queued_matters().unwrap().is_empty());
}

#[test]
fn recovery_generation_shares_conflict_key_and_is_writer_busy() {
    let runtime = WorkContextRuntime::hermetic_codex(CapabilityProfile::High, config(false));
    let gen1 = key("matter:docs", 1);
    let gen2 = key("matter:docs", 2);
    runtime.claim_writer(&gen1).unwrap();
    assert_eq!(
        runtime.claim_writer(&gen2).unwrap_err().code,
        ContinuityFailureCode::WriterBusy
    );
}

#[test]
fn honest_queue_double_write_on_same_binding_is_writer_busy() {
    let runtime = WorkContextRuntime::hermetic_codex(CapabilityProfile::Low, config(false));
    let source = key("matter:docs", 1);
    runtime.claim_writer(&source).unwrap();
    assert_eq!(
        runtime.claim_writer(&source).unwrap_err().code,
        ContinuityFailureCode::WriterBusy
    );
}

#[test]
fn shared_native_session_conflict_key_prevents_concurrent_writes() {
    let protocol = HermeticProtocol::codex(CapabilityProfile::High)
        .with_session_conflict_key("native-thread:shared");
    let runtime = WorkContextRuntime::from_hermetic(protocol, config(false));
    runtime.claim_writer(&key("matter:a", 1)).unwrap();
    let second = runtime.claim_writer(&key("matter:b", 1)).unwrap_err();
    assert_eq!(second.code, ContinuityFailureCode::WriterBusy);
}

#[test]
fn fork_requires_explicit_inheritance_and_does_not_imply_isolation() {
    let runtime = WorkContextRuntime::from_hermetic(
        HermeticProtocol::codex(CapabilityProfile::High)
            .with_fork(NativeCapabilitySupport::Supported),
        config(false),
    );
    let implicit = runtime.fork(&key("matter:docs", 1)).unwrap_err();
    assert_eq!(
        implicit.code,
        ContinuityFailureCode::NativeIsolationUnverified
    );
    let forked = runtime
        .fork_with_inheritance(
            &key("matter:docs", 1),
            &ForkInheritance::explicit(true, true, true, true, IsolationVerdict::Inherited),
        )
        .unwrap();
    assert_eq!(forked, 2);
    assert!(
        !ForkInheritance::explicit(true, true, true, true, IsolationVerdict::Inherited)
            .is_isolation()
    );
}

#[test]
fn knowledge_injection_cannot_claim_clean_new_binding() {
    let runtime = WorkContextRuntime::hermetic_codex(CapabilityProfile::Low, config(true));
    assert_eq!(
        runtime.start_new(&key("matter:fresh", 1)).unwrap_err().code,
        ContinuityFailureCode::NativeIsolationUnverified
    );
}

#[test]
fn compact_unverified_is_not_a_text_summary_success() {
    let runtime = WorkContextRuntime::hermetic_codex(CapabilityProfile::High, config(false));
    let snapshot = runtime.negotiate(&key("matter:docs", 1)).unwrap();
    assert_eq!(snapshot.compact, NativeCapabilitySupport::Unverified);
    assert_eq!(
        runtime.compact(&key("matter:docs", 1)).unwrap_err().code,
        ContinuityFailureCode::ReconciliationRequired
    );
}

#[test]
fn unavailable_port_stays_unsupported() {
    let port = unavailable_work_context_port();
    assert_eq!(
        port.claim_writer(&key("matter:docs", 1)).unwrap_err().code,
        ContinuityFailureCode::UnsupportedCapability
    );
}

#[test]
fn live_control_is_exact_generation_and_empty_native_is_reconciliation() {
    let runtime = WorkContextRuntime::hermetic_codex(CapabilityProfile::High, config(false));
    let current = key("matter:docs", 2);
    let previous = key("matter:docs", 1);
    assert!(runtime.live_control(&current).unwrap().is_none());
    assert_eq!(
        runtime
            .admit_live_control(&NativeControlRequest::cancel(
                current.clone(),
                "turn:host",
                "turn:native",
            ))
            .unwrap_err()
            .code,
        ContinuityFailureCode::ReconciliationRequired
    );
    runtime
        .bind_live_control(&current, "turn:host", "")
        .unwrap();
    assert!(runtime.live_control(&previous).unwrap().is_none());
    assert_eq!(
        runtime
            .admit_live_control(&NativeControlRequest::cancel(
                previous,
                "turn:host",
                "turn:native",
            ))
            .unwrap_err()
            .code,
        ContinuityFailureCode::ReconciliationRequired
    );
    assert_eq!(
        runtime
            .admit_live_control(&NativeControlRequest::cancel(
                current.clone(),
                "turn:host",
                "turn:native",
            ))
            .unwrap_err()
            .code,
        ContinuityFailureCode::ReconciliationRequired
    );
    runtime
        .bind_live_control(&current, "turn:host", "turn:native")
        .unwrap();
    runtime
        .admit_live_control(&NativeControlRequest::cancel(
            current.clone(),
            "turn:host",
            "turn:native",
        ))
        .unwrap();
    assert_eq!(
        runtime
            .admit_live_control(&NativeControlRequest::cancel(
                current,
                "turn:other",
                "turn:native",
            ))
            .unwrap_err()
            .code,
        ContinuityFailureCode::IdentityConflict
    );
}
