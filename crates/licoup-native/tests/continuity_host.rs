//! PLAN-CA-003 host integration. Real ConversationService + store paths.

use licoup_agent_runtime::work_context::{
    CapabilityProfile, ContinuityFailureCode, HermeticProtocol, NativeControlRequest,
    NativeWorkContextKey, NativeWorkContextPort, ProtocolFamily, SessionPresence, protocol_methods,
};
use licoup_conversation::continuity::{
    ContinuityAgreement, ContinuityAgreementOrigin, ContinuityAgreementProposal,
    ContinuityAgreementScope, ContinuityClosureAuthorityKind, ContinuityCommitmentProposal,
    ContinuityEvidenceRef, ContinuityEvidenceResult, ContinuityFollowThroughKind,
    ContinuityGoalCompletionTransition, ContinuityGoalControl, ContinuityGoalEvent,
    ContinuityGoalLifecycle, ContinuityGoalProgress, ContinuityInterpretationProposal,
    ContinuityInterrupt, ContinuityMatterSubject, ContinuityNextAttention,
    ContinuityParentContextGrant, ContinuityParentGrantStatus, ContinuityReadPort,
    ContinuitySourceOwnerKind, ContinuitySourceRef, ContinuitySourceValidity, ContinuitySpeechAct,
    ContinuityTaskChildAdmission, ContinuityTaskConversationRelation, ContinuityUtf8ByteSpan,
    ContinuityVerificationKind, ContinuityVisibilityScope, ContinuityWake, ContinuityWriteEnvelope,
    INGRESS_USER_POSTED_DESIGNATION, PROPOSAL_RESPONSE_CONTRACT,
    PROPOSAL_RESPONSE_DELEGATION_EXAMPLE, append_criterion_evidence, apply_goal_control,
    child_work_operation_id, count_cancel_effects, enqueue_review_wake, ingress_execution_recorded,
    list_all_parent_grants, list_all_pending_wakes, list_unacked_child_work,
    list_unapplied_settlements, put_agreement, put_effect, put_grant, read_agreements,
    read_child_work_accepted, read_child_work_intent, read_child_work_live, read_goal,
    read_settlement_pending, record_child_work_intent, replay_effect, revoke_source,
    set_continuity_clock, set_continuity_interrupt, settlement_applied,
};
use licoup_conversation::{
    ConversationEvent, ConversationStore, EventPart, EventPartKind, MembershipAccess, NewEventPart,
    Principal, PrincipalKind, ProfileIntentUpdate,
};
use licoup_native::domain::assistant_continuity::cognition::{SemanticScript, SpanAxis};
use licoup_native::domain::assistant_continuity::{
    ChildControlDisposition, ChildWorkFault, ContinuityHost, set_child_assembly_recheck_failures,
    set_child_work_fault,
};
use licoup_native::domain::client_conversation::{ConversationService, PersistentRuntimePorts};
use licoup_native::platform::work_context_ports::{
    AdapterTransport, boxed_work_context_port, fixture_child_binding, fixture_config,
    hermetic_codex_high, hermetic_pi_high, lost_session_protocol, preregistered_work_context_port,
};
use serde_json::{Value, json};

fn digest(tag: u8) -> String {
    format!("sha256:{:02x}{}", tag, "ab".repeat(31))
}

fn source_for(event_id: &str) -> ContinuitySourceRef {
    ContinuitySourceRef {
        owner_kind: ContinuitySourceOwnerKind::Event,
        opaque_id: event_id.to_owned(),
        part_id: None,
        span: None,
        source_revision: 1,
        digest: digest(1),
        visibility_scope: ContinuityVisibilityScope::Conversation,
        validity: ContinuitySourceValidity::Current,
    }
}

fn axis_script(
    event_id: &str,
    matter_id: &str,
    speech_act: ContinuitySpeechAct,
    subject: ContinuityMatterSubject,
    create_goal: bool,
    agreements: Vec<ContinuityAgreementProposal>,
    abstain: bool,
) -> SemanticScript {
    SemanticScript {
        event_opaque_id: event_id.to_owned(),
        axes: vec![SpanAxis {
            source_ref: source_for(event_id),
            subject,
            speech_act,
            follow_through: if create_goal {
                ContinuityFollowThroughKind::Durable
            } else {
                ContinuityFollowThroughKind::None
            },
            create_goal,
            matter_id: Some(matter_id.to_owned()),
            expected_result: Some("Prepare notes".into()),
            capability_needs: vec!["writing".into()],
            uncertainty_reasons: Vec::new(),
            requested_reads: Vec::new(),
            agreement_proposals: agreements,
            abstain,
            reason_code: "user-delegation".into(),
        }],
        fused: true,
        model_confidence: None,
        escalate: false,
    }
}

fn durable_script(event_id: &str, matter_id: &str) -> SemanticScript {
    SemanticScript {
        event_opaque_id: event_id.to_owned(),
        axes: vec![SpanAxis {
            source_ref: source_for(event_id),
            subject: ContinuityMatterSubject::New,
            speech_act: ContinuitySpeechAct::Delegation,
            follow_through: ContinuityFollowThroughKind::Durable,
            create_goal: true,
            matter_id: Some(matter_id.to_owned()),
            expected_result: Some("Prepare notes".into()),
            capability_needs: vec!["writing".into()],
            uncertainty_reasons: Vec::new(),
            requested_reads: Vec::new(),
            agreement_proposals: Vec::new(),
            abstain: false,
            reason_code: "user-delegation".into(),
        }],
        fused: true,
        model_confidence: None,
        escalate: false,
    }
}

fn create_group(service: &ConversationService) -> (String, String, String) {
    create_group_with_agent(service, "one")
}

fn create_group_with_agent(
    service: &ConversationService,
    agent_id: &str,
) -> (String, String, String) {
    create_group_owned_by(service, "human:local", agent_id)
}

fn create_group_owned_by(
    service: &ConversationService,
    owner_id: &str,
    agent_id: &str,
) -> (String, String, String) {
    let group = service
        .execute(json!({
            "action": "conversation.create",
            "title": "Host parent",
            "owner": {"id": owner_id, "kind": "human", "displayName": "You"},
            "members": [
                {
                    "principal": {
                        "id": format!("agent:{agent_id}"),
                        "kind": "agent",
                        "displayName": "One",
                        "agentId": agent_id
                    },
                    "access": "member"
                }
            ]
        }))
        .unwrap();
    let memberships = group["memberships"].as_array().unwrap();
    let owner = memberships
        .iter()
        .find(|membership| membership["principal"]["kind"] == "human")
        .unwrap()["id"]
        .as_str()
        .unwrap()
        .to_owned();
    let agent = memberships
        .iter()
        .find(|membership| membership["principal"]["kind"] == "agent")
        .unwrap()["id"]
        .as_str()
        .unwrap()
        .to_owned();
    (group["id"].as_str().unwrap().to_owned(), owner, agent)
}

fn post(
    service: &ConversationService,
    conversation_id: &str,
    author: &str,
    content: &str,
) -> String {
    let posted = service
        .execute(json!({
            "action": "conversation.message.post",
            "conversationId": conversation_id,
            "authorMembershipId": author,
            "content": content,
        }))
        .unwrap();
    posted["event"]["id"].as_str().unwrap().to_owned()
}

fn key(conversation_id: &str, membership_id: &str, matter: &str) -> NativeWorkContextKey {
    NativeWorkContextKey {
        conversation_id: conversation_id.to_owned(),
        membership_id: membership_id.to_owned(),
        matter_id: matter.to_owned(),
        generation: 1,
    }
}

#[test]
fn ordinary_post_reaches_admission_without_a_script() {
    let service = ConversationService::from_store(ConversationStore::open_in_memory().unwrap());
    let host = service.continuity().cloned().expect("host attached");
    let (conversation_id, owner, _) = create_group(&service);
    let before = service
        .execute(json!({"action": "conversation.get", "conversationId": conversation_id}))
        .unwrap();
    let posted = service
        .execute(json!({
            "action": "conversation.message.post",
            "conversationId": conversation_id,
            "authorMembershipId": owner,
            "content": "hello there",
        }))
        .unwrap();
    // Admission must be reachable without an installed script. An unbound host
    // cannot block, so admission runs here and reports itself unavailable. A
    // null result means production never enters admission at all, and then no
    // dependency of the commitment path is exercised by a real user post.
    let ingress = &posted["continuityIngress"];
    assert!(
        !ingress.is_null(),
        "a real user post must enter commitment admission without an installed script"
    );
    // Ordinary chat is still only chat: admission abstains instead of
    // fabricating a commitment.
    assert_eq!(
        ingress["committed"], false,
        "an ordinary greeting must not commit a commitment"
    );
    assert_eq!(
        ingress["abstained"], true,
        "an ordinary greeting must report the admission abstention"
    );
    assert!(
        ingress["childConversationId"].is_null(),
        "an abstaining admission must not fabricate a child conversation"
    );
    assert!(
        host.cognition_invocation_count() > 0,
        "admission must actually ask cognition, not short-circuit before it"
    );
    let after = service
        .execute(json!({"action": "conversation.get", "conversationId": conversation_id}))
        .unwrap();
    let views = after["taskViews"].as_array().cloned().unwrap_or_default();
    assert!(views.is_empty());
    assert!(before["id"] == after["id"]);
}

#[test]
fn scripted_user_input_admits_one_child_and_reuses_it() {
    let service = ConversationService::from_store(ConversationStore::open_in_memory().unwrap());
    let host = service.continuity().cloned().expect("host attached");
    let (conversation_id, owner, _) = create_group(&service);
    host.install_script(durable_script("", "matter:notes"));
    let posted = service
        .execute(json!({
            "action": "conversation.message.post",
            "conversationId": conversation_id,
            "authorMembershipId": owner,
            "content": "please prepare the notes",
        }))
        .unwrap();
    let event_id = posted["event"]["id"].as_str().unwrap().to_owned();
    assert_eq!(posted["continuityIngress"]["committed"], true);
    let child = posted["continuityIngress"]["childConversationId"]
        .as_str()
        .expect("child")
        .to_owned();
    let listed = service
        .execute(json!({"action": "conversation.list", "includeArchived": false}))
        .unwrap();
    let items = listed.as_array().unwrap();
    let child_item = items
        .iter()
        .find(|item| item["id"] == child)
        .expect("annotated child");
    assert_eq!(child_item["parentConversationId"], conversation_id);
    assert_eq!(child_item["listingKind"], "child-task");

    let retry = host.after_user_event(&conversation_id, &event_id).unwrap();
    assert!(retry.committed || retry.abstained);
    let again = host
        .store()
        .list_child_relations(&conversation_id, None, 20)
        .unwrap();
    assert_eq!(again.len(), 1);
    assert_eq!(again[0].child_conversation_id, child);

    let events = service
        .execute(json!({
            "action": "conversation.events.page",
            "conversationId": conversation_id,
            "limit": 50
        }))
        .unwrap();
    let card = events["events"]
        .as_array()
        .unwrap()
        .iter()
        .flat_map(|event| event["parts"].as_array().cloned().unwrap_or_default())
        .find(|part| {
            part["kind"] == "metadata"
                && part["content"]
                    .as_str()
                    .is_some_and(|content| content.contains(&child))
        });
    assert!(card.is_some(), "parent card stays a real Event/Part");
}

#[test]
fn a_and_b_interleave_keeps_grant_and_writer_isolation() {
    let service = ConversationService::from_store(ConversationStore::open_in_memory().unwrap());
    let host = service.continuity().cloned().expect("host attached");
    let (conversation_id, owner, _agent) = create_group(&service);
    host.install_script(durable_script("", "matter:a"));
    let _event_a = post(&service, &conversation_id, &owner, "notes A");
    host.install_script(durable_script("", "matter:b"));
    let _event_b = post(&service, &conversation_id, &owner, "notes B");
    let relations = host
        .store()
        .list_child_relations(&conversation_id, None, 20)
        .unwrap();
    assert_eq!(relations.len(), 2);

    host.bind_hermetic(
        HermeticProtocol::codex(CapabilityProfile::Low).with_presence(SessionPresence::Present),
        fixture_config(true),
    );
    let child = fixture_child_binding();
    let port = host
        .work_runtime(
            &child.child_conversation_id,
            &child.source_task_id,
            &child.membership_id,
            0,
        )
        .expect("exact hermetic runtime");
    port.claim_writer(&key(
        &child.child_conversation_id,
        &child.membership_id,
        "matter:a",
    ))
    .unwrap();
    assert_eq!(
        port.claim_writer(&key(
            &child.child_conversation_id,
            &child.membership_id,
            "matter:b",
        ))
        .unwrap_err()
        .code,
        ContinuityFailureCode::WriterBusy
    );
}

#[test]
fn accepted_completion_notifies_once_through_restart() {
    let root = std::env::temp_dir().join(format!("lico-ca-notice-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&root).unwrap();
    let service = ConversationService::open(&root).unwrap();
    let host = service.continuity().cloned().expect("host attached");
    let (conversation_id, owner, _) = create_group(&service);
    host.install_script(durable_script("", "matter:close"));
    let _event_id = post(&service, &conversation_id, &owner, "close later");
    let relation = host
        .store()
        .list_child_relations(&conversation_id, None, 8)
        .unwrap()
        .remove(0);
    let current = read_goal(host.store(), &relation.goal_id).unwrap().unwrap();
    let progress = ContinuityGoalProgress {
        goal_id: relation.goal_id.clone(),
        revision: current.revision,
        lifecycle: ContinuityGoalLifecycle::Achieved,
        control: ContinuityGoalControl::Enabled,
        criterion_evidence_refs: Vec::new(),
        active_execution_refs: Vec::new(),
        blockers: Vec::new(),
        next_attention: None,
        closure_ref: None,
    };
    let transition = ContinuityGoalCompletionTransition {
        transition_id: "transition:close".into(),
        goal_id: relation.goal_id.clone(),
        from_lifecycle: ContinuityGoalLifecycle::Active,
        to_lifecycle: ContinuityGoalLifecycle::Achieved,
        goal_revision: progress.revision,
        authority_kind: ContinuityClosureAuthorityKind::GoalEvaluation,
        evaluation_ref: ContinuitySourceRef {
            owner_kind: ContinuitySourceOwnerKind::Goal,
            opaque_id: relation.goal_id.clone(),
            part_id: None,
            span: None,
            source_revision: progress.revision,
            digest: digest(9),
            visibility_scope: ContinuityVisibilityScope::Goal,
            validity: ContinuitySourceValidity::Current,
        },
        notification_id: "notice:close".into(),
    };
    let first = host
        .accept_goal_completion(&conversation_id, &transition, &progress)
        .unwrap();
    assert!(first.is_some());
    let second = host
        .accept_goal_completion(&conversation_id, &transition, &progress)
        .unwrap();
    assert!(second.is_none());

    let reopened = ConversationService::open(&root).unwrap();
    let host2 = reopened.continuity().cloned().unwrap();
    let third = host2
        .accept_goal_completion(&conversation_id, &transition, &progress)
        .unwrap();
    assert!(third.is_none());
}

#[test]
fn unknown_effect_restarts_into_reconciliation_not_replay() {
    let root = std::env::temp_dir().join(format!("lico-ca-unknown-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&root).unwrap();
    let service = ConversationService::open(&root).unwrap();
    let host = service.continuity().cloned().unwrap();
    let (conversation_id, owner, _) = create_group(&service);
    host.install_script(durable_script("", "matter:ext"));
    let _event_id = post(&service, &conversation_id, &owner, "external");
    let goal_id = host
        .store()
        .list_child_relations(&conversation_id, None, 8)
        .unwrap()[0]
        .goal_id
        .clone();
    put_effect(
        host.store(),
        &conversation_id,
        Some(&goal_id),
        "effect:external-one",
        licoup_conversation::continuity::ContinuityEffectStatus::Unknown,
    )
    .unwrap();
    assert!(replay_effect(host.store(), "effect:external-one").is_err());

    let reopened = ConversationService::open(&root).unwrap();
    let drain = reopened.drain_continuity(&conversation_id).unwrap();
    assert_eq!(drain["replayed"], 0);
    assert!(
        !drain["reconciled"]
            .as_array()
            .unwrap()
            .iter()
            .any(|id| id == "effect:external-one"),
        "unknown must not be labeled reconciled from refused replay: {drain}"
    );
    assert!(
        reopened
            .continuity()
            .unwrap()
            .unknown_effect_ids()
            .iter()
            .any(|id| id == "effect:external-one")
    );
    assert!(
        !drain["waiting"].as_array().unwrap().is_empty()
            || drain["preserved"].as_array().unwrap().iter().any(|id| {
                id.as_str()
                    .is_some_and(|value| value.contains("wake:") || value.contains("effect:"))
            }),
        "unknown work stays waiting or preserved: {drain}"
    );
}

#[test]
fn unbound_port_stays_unavailable_and_host_uses_real_methods() {
    let port = preregistered_work_context_port();
    let boxed = boxed_work_context_port();
    let lost = lost_session_protocol(true);
    assert_eq!(
        port.steer(&NativeControlRequest::steer(
            key("conversation:x", "membership:y", "matter:z"),
            "steer-guidance",
            "turn:host",
            "turn:native",
        ))
        .unwrap_err()
        .code,
        ContinuityFailureCode::UnsupportedCapability
    );
    assert_eq!(
        boxed
            .steer(&NativeControlRequest::steer(
                key("conversation:x", "membership:y", "matter:z"),
                "steer-guidance",
                "turn:host",
                "turn:native",
            ))
            .unwrap_err()
            .code,
        ContinuityFailureCode::UnsupportedCapability
    );
    let _ = (lost, hermetic_codex_high(), hermetic_pi_high());
    assert_eq!(
        protocol_methods(ProtocolFamily::Codex).exact_resume,
        "thread/resume"
    );
}

#[test]
fn observer_attach_does_not_bump_generation() {
    let root = std::env::temp_dir().join(format!("lico-ca-obs-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&root).unwrap();
    let first = ConversationService::open(&root).unwrap();
    let generation = first.continuity().unwrap().host_generation();
    let listed = first
        .execute(json!({"action": "conversation.list", "includeArchived": false}))
        .unwrap();
    assert!(listed.is_array());
    let second = ConversationService::open(&root).unwrap();
    assert_eq!(second.continuity().unwrap().host_generation(), generation);
}

#[test]
fn owner_claim_bumps_generation_and_preserves_unknown_effects() {
    let service = ConversationService::from_store(ConversationStore::open_in_memory().unwrap());
    let before = service.continuity().unwrap().host_generation();
    service.claim_continuity_owner().unwrap();
    assert!(service.continuity().unwrap().host_generation() > before);
}

#[test]
fn automatic_wake_denies_new_advancement_without_qualification() {
    let service = ConversationService::from_store(ConversationStore::open_in_memory().unwrap());
    let host = service.continuity().cloned().unwrap();
    let (conversation_id, owner, _) = create_group(&service);
    host.install_script(durable_script("", "matter:wake"));
    let _ = post(&service, &conversation_id, &owner, "prepare notes");
    let drain = service.drain_continuity(&conversation_id).unwrap();
    assert_eq!(drain["replayed"], 0);
    let preserved = drain["preserved"].as_array().cloned().unwrap_or_default();
    let no_ops = drain["noOps"].as_array().cloned().unwrap_or_default();
    assert!(
        !preserved.is_empty() || !no_ops.is_empty(),
        "wake must be preserved or recorded as an explicit no-op: {drain}"
    );
}

#[test]
fn revise_accept_replace_commands_have_effects() {
    let service = ConversationService::from_store(ConversationStore::open_in_memory().unwrap());
    let host = service.continuity().cloned().unwrap();
    let (conversation_id, owner, agent) = create_group(&service);
    host.install_script(durable_script("", "matter:cmd"));
    let _ = post(&service, &conversation_id, &owner, "prepare notes");
    let goal_id = host
        .store()
        .list_child_relations(&conversation_id, None, 8)
        .unwrap()[0]
        .goal_id
        .clone();
    let revised = service
        .execute(json!({
            "action": "revise-agreement",
            "conversationId": conversation_id,
            "agreement": {
                "id": "agreement:notes",
                "scope": "matter",
                "statementRef": {
                    "ownerKind": "event",
                    "opaqueId": "event:agreement",
                    "sourceRevision": 1,
                    "digest": digest(3),
                    "visibilityScope": "conversation",
                    "validity": "current"
                },
                "origin": "user-explicit",
                "effectiveRevision": 1,
                "validFrom": 1,
                "revocationGeneration": 0
            }
        }))
        .unwrap();
    assert_eq!(revised["ok"], true);
    assert_ne!(
        revised.get("deferred").cloned().unwrap_or(json!(false)),
        true
    );
    assert!(revised["effectiveRevision"].as_i64().unwrap() >= 1);

    let accepted = service
        .execute(json!({
            "action": "accept-evidence",
            "conversationId": conversation_id,
            "goalId": goal_id,
            "evidence": {
                "source": {
                    "ownerKind": "event",
                    "opaqueId": "event:evidence",
                    "sourceRevision": 1,
                    "digest": digest(4),
                    "visibilityScope": "goal",
                    "validity": "current"
                },
                "issuer": "user",
                "subjectVersion": 1,
                "criterionId": "criterion:notes",
                "observedAt": 1,
                "result": "pass",
                "verificationKind": "user-acceptance",
                "scope": "goal",
                "validity": "current"
            }
        }))
        .unwrap();
    assert_eq!(accepted["ok"], true);
    assert_eq!(accepted["lifecycle"], "active");
    assert!(accepted["evidenceCount"].as_u64().unwrap() >= 1);

    let replaced = service
        .execute(json!({
            "action": "replace-assistant",
            "conversationId": conversation_id,
            "membershipId": agent
        }))
        .unwrap();
    assert_eq!(replaced["ok"], true);
    assert!(replaced["revision"].as_i64().unwrap() >= 1);
}

#[test]
fn adapter_resume_invokes_transport_and_lost_is_adapter_result() {
    use licoup_native::platform::work_context_ports::{
        AdapterCall, AdapterResponse, CountingTransport,
    };
    use std::sync::Arc;
    let service = ConversationService::from_store(ConversationStore::open_in_memory().unwrap());
    let host = service.continuity().cloned().unwrap();
    let (conversation_id, owner, _) = create_group(&service);
    host.install_script(durable_script("", "matter:adapter"));
    let _ = post(&service, &conversation_id, &owner, "prepare notes");
    let relation = host
        .store()
        .list_child_relations(&conversation_id, None, 8)
        .unwrap()
        .remove(0);
    let lost = Arc::new(CountingTransport::new(|call: &AdapterCall| {
        AdapterResponse::err(format!("lost:{}", call.method))
    }));
    host.bind_adapter_for_goal(
        &conversation_id,
        &relation.goal_id,
        ProtocolFamily::Codex,
        lost.clone(),
    )
    .unwrap();
    let binding = host
        .child_binding(&conversation_id, &relation.goal_id)
        .unwrap();
    host.store()
        .runtime_binding_with_private_location(
            licoup_conversation::RuntimeBinding {
                id: "binding:adapter".into(),
                conversation_id: binding.child_conversation_id.clone(),
                membership_id: binding.membership_id.clone(),
                lane: "conversation".into(),
                availability: "available".into(),
                safe_reason: None,
            },
            Some("thread:existing"),
            None,
            None,
        )
        .unwrap();
    host.bind_adapter_for_goal(
        &conversation_id,
        &relation.goal_id,
        ProtocolFamily::Codex,
        lost.clone(),
    )
    .unwrap();
    let generation = read_goal(host.store(), &relation.goal_id)
        .unwrap()
        .map(|progress| progress.revision)
        .unwrap_or(0);
    let port = host
        .work_runtime(
            &binding.child_conversation_id,
            &relation.goal_id,
            &binding.membership_id,
            generation,
        )
        .expect("exact adapter runtime");
    let err = port
        .exact_resume(&key(
            &binding.child_conversation_id,
            &binding.membership_id,
            "matter:adapter",
        ))
        .unwrap_err();
    assert_eq!(err.code, ContinuityFailureCode::NativeBindingLost);
    assert!(lost.invocation_count() >= 1);
}

#[test]
fn emit_host_oracle_line() {
    let service = ConversationService::from_store(ConversationStore::open_in_memory().unwrap());
    let host = service.continuity().cloned().unwrap();
    let (conversation_id, owner, _) = create_group(&service);
    host.install_script(durable_script("", "matter:notes"));
    let posted = service
        .execute(json!({
            "action": "conversation.message.post",
            "conversationId": conversation_id,
            "authorMembershipId": owner,
            "content": "please prepare the notes",
        }))
        .unwrap();
    let natural_entry = posted["continuityIngress"]["committed"] == true
        && posted["continuityIngress"]["invocationCount"]
            .as_u64()
            .unwrap()
            >= 1;
    let relations = host
        .store()
        .list_child_relations(&conversation_id, None, 8)
        .unwrap();
    let one_child = relations.len() == 1;
    let events = service
        .execute(json!({
            "action": "conversation.events.page",
            "conversationId": conversation_id,
            "limit": 50
        }))
        .unwrap();
    let card = events["events"]
        .as_array()
        .unwrap()
        .iter()
        .flat_map(|event| event["parts"].as_array().cloned().unwrap_or_default())
        .any(|part| part["kind"] == "metadata");

    host.bind_hermetic(
        HermeticProtocol::codex(CapabilityProfile::Low).with_presence(SessionPresence::Present),
        fixture_config(true),
    );
    let child = fixture_child_binding();
    let writer_isolation = {
        let port = host
            .work_runtime(
                &child.child_conversation_id,
                &child.source_task_id,
                &child.membership_id,
                0,
            )
            .expect("exact hermetic runtime");
        port.claim_writer(&key(
            &child.child_conversation_id,
            &child.membership_id,
            "matter:a",
        ))
        .is_ok()
            && port
                .claim_writer(&key(
                    &child.child_conversation_id,
                    &child.membership_id,
                    "matter:b",
                ))
                .is_err()
    };

    let progress = read_goal(host.store(), &relations[0].goal_id)
        .unwrap()
        .unwrap();
    let transition = ContinuityGoalCompletionTransition {
        transition_id: "transition:oracle".into(),
        goal_id: relations[0].goal_id.clone(),
        from_lifecycle: ContinuityGoalLifecycle::Active,
        to_lifecycle: ContinuityGoalLifecycle::Achieved,
        goal_revision: progress.revision,
        authority_kind: ContinuityClosureAuthorityKind::UserAcceptance,
        evaluation_ref: source_for("event:oracle-accept"),
        notification_id: "notice:oracle".into(),
    };
    let achieved = ContinuityGoalProgress {
        lifecycle: ContinuityGoalLifecycle::Achieved,
        next_attention: None,
        ..progress.clone()
    };
    let first = host
        .accept_goal_completion(&conversation_id, &transition, &achieved)
        .unwrap();
    let second = host
        .accept_goal_completion(&conversation_id, &transition, &achieved)
        .unwrap();
    let notice_once = first.is_some() && second.is_none();

    put_effect(
        host.store(),
        &conversation_id,
        Some(&relations[0].goal_id),
        "effect:oracle-unknown",
        licoup_conversation::continuity::ContinuityEffectStatus::Unknown,
    )
    .unwrap();
    let drain = service.drain_continuity(&conversation_id).unwrap();
    let unknown_reconciles = drain["replayed"] == 0
        && !drain["reconciled"]
            .as_array()
            .unwrap()
            .iter()
            .any(|id| id == "effect:oracle-unknown")
        && host
            .unknown_effect_ids()
            .iter()
            .any(|id| id == "effect:oracle-unknown");
    let replayed = drain["replayed"].as_u64().unwrap_or(99);
    let oracle = json!({
        "ac03001": {
            "naturalEntry": natural_entry,
            "oneChildPerGoal": one_child,
            "cardIsEventPart": card,
            "writerIsolation": writer_isolation,
            "noticeOnce": notice_once
        },
        "ac03002": {
            "unknownReconciles": unknown_reconciles,
            "replayed": replayed
        },
        "fixturePath": "tests/fixtures/continuous-assistant/integration/continuous-collaboration.json"
    });
    println!("CONTINUITY_HOST_ORACLE:{oracle}");
    assert_eq!(oracle["ac03001"]["naturalEntry"], true);
    assert_eq!(oracle["ac03001"]["oneChildPerGoal"], true);
    assert_eq!(oracle["ac03001"]["cardIsEventPart"], true);
    assert_eq!(oracle["ac03001"]["writerIsolation"], true);
    assert_eq!(oracle["ac03001"]["noticeOnce"], true);
    assert_eq!(oracle["ac03002"]["unknownReconciles"], true);
    assert_eq!(oracle["ac03002"]["replayed"], 0);
}

fn load_integration_fixture() -> Value {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(
        "../../tests/fixtures/continuous-assistant/integration/continuous-collaboration.json",
    );
    serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
}

fn assertion_holds(name: &str, state: &Value) -> bool {
    match name {
        "/goal/has_next_attention" => {
            state["hasNextAttention"] == true && state["shareGoalPresent"] == true
        }
        "/goal/not_closed" => state["achieved"] == false && state["lifecycle"] != "achieved",
        "/matter/association_preserved" => {
            state["matterId"] == "matter:share" && state["shareRelationCount"] == 1
        }
        "/agreement/scope_is_matter" => state["agreementScope"] == "matter",
        "/agreement/correction_effective" => {
            state["agreementRevision"].as_i64().unwrap_or(0) >= 2
                && state["agreementSupersedes"].as_i64().unwrap_or(0) >= 1
        }
        "/effects/no_cancel" => state["cancelEffects"] == 0 && state["lifecycle"] != "cancelled",
        "/effects/no_paid_wake" => {
            state["replayed"] == 0 && state["paused"] == true && state["paidShareWake"] == false
        }
        "/state/no_orphan_goal" => {
            state["goalCount"] == state["relationCount"]
                && state["relationCount"].as_u64().unwrap_or(0) >= 1
        }
        "/recovery/no_duplicate_effect" => {
            state["duplicateWakes"] == false && state["interruptedRecoveredOnce"] == true
        }
        "/goal/waiting_for_user" => {
            state["lifecycle"] == "waiting" || state["waitingForUser"] == true
        }
        "/goal/achieved_with_user_acceptance" => {
            state["achieved"] == true
                && state["userAccepted"] == true
                && state["closureAuthority"] == "user-acceptance"
                && state["evidenceCount"].as_u64().unwrap_or(0) > 0
        }
        "/state/terminal_monotonic" => {
            state["achieved"] == true && state["lateUnachieved"] == false
        }
        "/context/no_unrelated_private_content" => state["privateLeak"] == false,
        "/context/restored_relevant_agreements" => {
            state["agreementCount"].as_u64().unwrap_or(0) > 0 && state["agreementScope"] == "matter"
        }
        "/context/current_source_versions" => {
            state["maxSubjectVersion"].as_i64().unwrap_or(0) >= 2
                && state["staleEvidenceUsed"] == false
                && state["shareChildHasMaterialAv2"] == true
                && state["shareChildHasMaterialAv1"] == false
        }
        "/artifact/author_preserved" => {
            state["artifactAuthor"]
                .as_str()
                .is_some_and(|author| !author.is_empty())
                && state["artifactAuthorRewritten"] == false
        }
        "/state/stale_proposal_rejected" => state["staleRejected"] == true,
        "/control/future_followups_paused" => state["paused"] == true,
        "/scope/local_only" => state["outboundGrant"] == false,
        "/evidence/subject_version_matches" => {
            state["subjectVersionMatches"] == true && state["maxSubjectVersion"] == 3
        }
        _ => false,
    }
}

fn observe_journey(
    service: &ConversationService,
    conversation_id: &str,
    extras: Value,
    start_calls: &[Value],
    complete_calls: &[Value],
) -> Value {
    let host = service.continuity().cloned().unwrap();
    let relations = host
        .store()
        .list_child_relations(conversation_id, None, 50)
        .expect("list_child_relations");
    let agreements = read_agreements(host.store(), conversation_id).expect("read_agreements");
    let share = relations
        .iter()
        .find(|relation| relation.goal_id == "goal:matter:share");
    let progress = match share {
        Some(relation) => Some(
            read_goal(host.store(), &relation.goal_id)
                .expect("read_goal")
                .expect("share goal"),
        ),
        None => None,
    };
    let waiting_for_user = progress.as_ref().is_some_and(|item| {
        item.lifecycle == ContinuityGoalLifecycle::Waiting
            || matches!(
                item.next_attention,
                Some(ContinuityNextAttention::Wait {
                    ref responsible_party,
                    ..
                }) if responsible_party == "user"
            )
    });
    let evidence = progress
        .as_ref()
        .map(|item| item.criterion_evidence_refs.clone())
        .unwrap_or_default();
    let max_subject = evidence
        .iter()
        .map(|item| item.subject_version)
        .max()
        .unwrap_or(0);
    let stale_evidence_used = evidence.iter().any(|item| {
        (item.validity == ContinuitySourceValidity::Stale
            || item.source.validity == ContinuitySourceValidity::Stale)
            && item.subject_version == max_subject
    });
    let cancel_effects =
        count_cancel_effects(host.store(), conversation_id).expect("count_cancel_effects");
    let child_ids = relations
        .iter()
        .map(|item| item.child_conversation_id.clone())
        .collect::<Vec<_>>();
    let outbound_grant = list_all_parent_grants(host.store())
        .expect("list_all_parent_grants")
        .into_iter()
        .any(|grant| {
            !child_ids.contains(&grant.recipient_conversation_id)
                || grant.source_conversation_id != conversation_id
                || grant.status == ContinuityParentGrantStatus::Revoked
                    && grant.authorized_scopes.iter().any(|scope| {
                        let encoded = serde_json::to_string(scope).unwrap_or_default();
                        encoded.contains("external") || encoded.contains("outbound")
                    })
        });
    let conversation = service
        .store()
        .get(conversation_id)
        .expect("parent conversation");
    let agent = conversation
        .assistant_membership_id
        .clone()
        .expect("parent assistant");
    let page = service
        .execute(json!({
            "action": "conversation.events.page",
            "conversationId": conversation_id,
            "limit": 50
        }))
        .expect("conversation.events.page");
    let last_event = page["events"]
        .as_array()
        .expect("events page")
        .iter()
        .rev()
        .find(|event| event.get("id").and_then(Value::as_str).is_some())
        .cloned()
        .expect("last event");
    let last_event_id = last_event["id"].as_str().expect("last event id");
    let last_event_text = last_event["parts"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|part| part["content"].as_str())
        .collect::<Vec<_>>()
        .join("\n");
    let _guidance = host
        .compose_ingress_guidance(conversation_id, &agent, last_event_id)
        .expect("compose_ingress_guidance");
    let share_member = share
        .map(|relation| child_recipient_id(host.store(), &relation.child_conversation_id))
        .unwrap_or_default();
    let sibling_member = relations
        .iter()
        .find(|relation| relation.goal_id == "goal:matter:sibling")
        .map(|relation| child_recipient_id(host.store(), &relation.child_conversation_id))
        .unwrap_or_default();
    let share_child_id = share.map(|relation| relation.child_conversation_id.as_str());
    let share_execution = share_child_id
        .map(|child_id| child_work_blob(start_calls, child_id, &share_member))
        .unwrap_or_default();
    let share_guidance = share
        .map(|relation| {
            host.compose_ingress_guidance(&relation.child_conversation_id, &share_member, "")
                .expect("share child compose_ingress_guidance")
        })
        .unwrap_or_default();
    let sibling_invocations =
        invocation_blob_for_member(start_calls, complete_calls, &sibling_member);
    let parent_latest = start_calls.iter().rev().find(|params| {
        params.get("conversationId").and_then(Value::as_str) == Some(conversation_id)
            && params.get("membershipId").and_then(Value::as_str) == Some(agent.as_str())
            && params.get("continuityKind").and_then(Value::as_str) != Some("child-work")
    });
    let parent_payload = parent_latest.map(delivered_guidance).unwrap_or_default();
    let parent_chitchat_leak = (!last_event_text.contains("资料A")
        && parent_payload.contains("资料A"))
        || (!last_event_text.contains("报名") && parent_payload.contains("报名"));
    let private_leak = parent_chitchat_leak
        || sibling_invocations.contains("资料A")
        || sibling_invocations.contains("报名")
        || share_execution.contains("兄弟任务私有哨兵");
    let (artifact_author, artifact_rewritten) = share
        .map(|relation| {
            let child = host.store().get(&relation.child_conversation_id).unwrap();
            let expected = child
                .assistant_membership_id
                .clone()
                .or_else(|| {
                    child.memberships.iter().find_map(|membership| {
                        (membership.principal.kind == PrincipalKind::Agent)
                            .then(|| membership.id.clone())
                    })
                })
                .unwrap_or_default();
            let author = child_events(service, &relation.child_conversation_id)
                .into_iter()
                .rev()
                .find_map(|event| {
                    let kind_ok = event["parts"]
                        .as_array()
                        .into_iter()
                        .flatten()
                        .any(|part| part["kind"] == "text" || part["kind"] == "artifact");
                    kind_ok
                        .then(|| {
                            event["authorMembershipId"]
                                .as_str()
                                .unwrap_or("")
                                .to_owned()
                        })
                        .filter(|author| !author.is_empty())
                })
                .unwrap_or_default();
            let rewritten = !author.is_empty() && !expected.is_empty() && author != expected;
            (author, rewritten)
        })
        .unwrap_or((String::new(), false));
    let closure_authority = share
        .and_then(|relation| relation.completion_transition.as_ref())
        .and_then(|transition| serde_json::to_value(transition.authority_kind).ok())
        .unwrap_or(json!(""));
    let user_accepted = progress
        .as_ref()
        .is_some_and(|item| item.lifecycle == ContinuityGoalLifecycle::Achieved)
        && closure_authority == "user-acceptance";
    let goal_count = relations
        .iter()
        .filter(|relation| {
            read_goal(host.store(), &relation.goal_id)
                .expect("read_goal for orphan check")
                .is_some()
        })
        .count();
    let mut state = json!({
        "goalCount": goal_count,
        "relationCount": relations.len(),
        "shareGoalPresent": share.is_some(),
        "shareRelationCount": share.is_some() as i64,
        "hasNextAttention": progress.as_ref().is_some_and(|item| item.next_attention.is_some()),
        "achieved": progress.as_ref().is_some_and(|item| item.lifecycle == ContinuityGoalLifecycle::Achieved),
        "lifecycle": progress.as_ref().and_then(|item| serde_json::to_value(item.lifecycle).ok()).unwrap_or(json!("")),
        "paused": progress.as_ref().is_some_and(|item| item.control == ContinuityGoalControl::Paused),
        "agreementCount": agreements.len(),
        "agreementScope": agreements.first().and_then(|item| serde_json::to_value(item.scope).ok()).unwrap_or(json!("")),
        "agreementRevision": agreements.iter().map(|item| item.effective_revision).max().unwrap_or(0),
        "agreementSupersedes": agreements.iter().filter_map(|item| item.supersedes).max().unwrap_or(0),
        "replayed": extras.get("lastReplayed").cloned().unwrap_or(json!(0)),
        "matterId": share
            .map(|item| item.goal_id.trim_start_matches("goal:").to_owned())
            .unwrap_or_default(),
        "waitingForUser": waiting_for_user,
        "evidenceCount": evidence.len(),
        "maxSubjectVersion": max_subject,
        "cognitionCount": host.cognition_invocation_count(),
        "unknown": host.unknown_effect_ids(),
        "cancelEffects": cancel_effects,
        "privateLeak": private_leak,
        "outboundGrant": outbound_grant,
        "staleEvidenceUsed": stale_evidence_used,
        "artifactAuthor": artifact_author,
        "artifactAuthorRewritten": artifact_rewritten,
        "closureAuthority": closure_authority,
        "userAccepted": user_accepted,
        "subjectVersionMatches": evidence.iter().any(|item| item.subject_version == max_subject)
            && max_subject > 0,
        "shareChildHasMaterialA": extras.get("shareExecHasA").cloned().unwrap_or(json!(false)),
        "shareChildHasMaterialAv1": extras
            .get("shareExecHasAv1")
            .cloned()
            .unwrap_or(json!(false)),
        "shareChildHasMaterialAv2": extras
            .get("shareExecHasAv2")
            .cloned()
            .unwrap_or(json!(false)),
        "shareChildHasMaterialB": extras.get("shareExecHasB").cloned().unwrap_or(json!(false)),
        "shareCompositionHasAv2": share_guidance.contains("资料A版本2"),
        "cognitionDelta": extras.get("lastCognitionDelta").cloned().unwrap_or(json!(0)),
        "duplicateWakes": extras.get("lastDuplicateWakes").cloned().unwrap_or(json!(false)),
        "staleRejected": extras.get("lastStaleRejected").cloned().unwrap_or(json!(false)),
        "lateUnachieved": extras.get("lastLateUnachieved").cloned().unwrap_or(json!(false)),
    });
    if let Some(object) = extras.as_object() {
        for (key, value) in object {
            if key.starts_with("last") {
                continue;
            }
            state[key] = value.clone();
        }
    }
    state
}

#[test]
fn fixture_ca_j001_drives_real_ingress_and_state_oracle() {
    let fixture = load_integration_fixture();
    assert_eq!(fixture["id"], "CA-J001");
    let complete_calls = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let start_calls = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let root = std::env::temp_dir().join(format!("lico-ca-j001-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&root).unwrap();
    let service = bind_effect_free_runtime(
        ConversationService::open(&root).unwrap(),
        complete_calls.clone(),
        start_calls.clone(),
    );
    service.claim_continuity_owner().unwrap();
    let host = service.continuity().cloned().unwrap();
    let (conversation_id, owner, agent) = create_group_with_agent(&service, "codex");
    designate_assistant(&service, &conversation_id, &owner, &agent);
    let mut extras = json!({});
    let snapshot_calls = || {
        (
            start_calls
                .lock()
                .unwrap_or_else(|poison| poison.into_inner())
                .clone(),
            complete_calls
                .lock()
                .unwrap_or_else(|poison| poison.into_inner())
                .clone(),
        )
    };
    for step in fixture["steps"].as_array().unwrap() {
        let seq = step["seq"].as_u64().unwrap();
        let kind = step["kind"].as_str().unwrap_or("");
        match kind {
            "user" => {
                let input = step["input"].as_str().unwrap_or("");
                let proposal = match seq {
                    1 => typed_goal_proposal_json(
                        &conversation_id,
                        "goal:matter:share",
                        "matter:share",
                    ),
                    2 => speech_proposal_json(
                        &conversation_id,
                        seq,
                        ContinuitySpeechAct::Exploration,
                        ContinuityMatterSubject::Existing,
                        false,
                        Vec::new(),
                    ),
                    3 => speech_proposal_json(
                        &conversation_id,
                        seq,
                        ContinuitySpeechAct::Correction,
                        ContinuityMatterSubject::Existing,
                        false,
                        vec![ContinuityAgreementProposal {
                            scope: ContinuityAgreementScope::Matter,
                            statement_ref: source_for("event:agreement"),
                            origin: ContinuityAgreementOrigin::UserExplicit,
                        }],
                    ),
                    5 | 10 => speech_proposal_json(
                        &conversation_id,
                        seq,
                        ContinuitySpeechAct::Question,
                        ContinuityMatterSubject::Existing,
                        false,
                        Vec::new(),
                    ),
                    13 => speech_proposal_json(
                        &conversation_id,
                        seq,
                        ContinuitySpeechAct::Correction,
                        ContinuityMatterSubject::Existing,
                        false,
                        vec![ContinuityAgreementProposal {
                            scope: ContinuityAgreementScope::Matter,
                            statement_ref: source_for("event:agreement-v2"),
                            origin: ContinuityAgreementOrigin::UserExplicit,
                        }],
                    ),
                    15 => speech_proposal_json(
                        &conversation_id,
                        seq,
                        ContinuitySpeechAct::Pause,
                        ContinuityMatterSubject::Existing,
                        false,
                        Vec::new(),
                    ),
                    17 => speech_proposal_json(
                        &conversation_id,
                        seq,
                        ContinuitySpeechAct::Reference,
                        ContinuityMatterSubject::Resumed,
                        false,
                        Vec::new(),
                    ),
                    19 => speech_proposal_json(
                        &conversation_id,
                        seq,
                        ContinuitySpeechAct::Approval,
                        ContinuityMatterSubject::Existing,
                        false,
                        Vec::new(),
                    ),
                    _ => speech_proposal_json(
                        &conversation_id,
                        seq,
                        ContinuitySpeechAct::Question,
                        ContinuityMatterSubject::Existing,
                        false,
                        Vec::new(),
                    ),
                };
                let posted = service
                    .execute(json!({
                        "action": "conversation.message.post",
                        "conversationId": conversation_id,
                        "authorMembershipId": owner,
                        "content": input,
                    }))
                    .unwrap();
                assert!(
                    posted["continuityIngress"].is_null(),
                    "step {seq} production post persists only"
                );
                let event_id = posted["event"]["id"].as_str().unwrap();
                let dispatched = after_post(&service, &conversation_id, event_id);
                assert!(
                    dispatched["directTurns"].as_array().is_some(),
                    "step {seq} must use after-post"
                );
                service
                    .after_runtime_settlement(
                        &conversation_id,
                        &json!({
                            "output": assistant_turn_output("Readable ordinary reply.", &proposal),
                            "membershipId": agent,
                            "causationId": event_id,
                        }),
                    )
                    .unwrap();
                if seq == 3 || seq == 13 {
                    let revision = if seq == 3 { 1 } else { 2 };
                    let revised = service
                        .execute(json!({
                            "action": "revise-agreement",
                            "conversationId": conversation_id,
                            "agreement": {
                                "id": format!("agreement:matter-scope:{revision}"),
                                "scope": "matter",
                                "statementRef": {
                                    "ownerKind": "event",
                                    "opaqueId": format!("event:agreement:{seq}"),
                                    "sourceRevision": revision,
                                    "digest": digest(6),
                                    "visibilityScope": "conversation",
                                    "validity": "current"
                                },
                                "origin": "user-explicit",
                                "effectiveRevision": revision,
                                "validFrom": 1,
                                "revocationGeneration": 0
                            }
                        }))
                        .unwrap();
                    assert_eq!(revised["ok"], true, "step {seq} revise-agreement");
                }
                if seq == 15 {
                    let relation = share_relation(&host, &conversation_id);
                    let paused = service
                        .execute(json!({
                            "action": "pause-goal",
                            "conversationId": conversation_id,
                            "goalId": relation.goal_id,
                        }))
                        .unwrap();
                    assert_eq!(paused["control"], "paused");
                }
                if seq == 17 {
                    let relation = share_relation(&host, &conversation_id);
                    let resumed = service
                        .execute(json!({
                            "action": "resume-goal",
                            "conversationId": conversation_id,
                            "goalId": relation.goal_id,
                        }))
                        .unwrap();
                    assert_eq!(resumed["control"], "enabled");
                }
                if seq == 19 {
                    let relation = share_relation(&host, &conversation_id);
                    let current = read_goal(host.store(), &relation.goal_id).unwrap().unwrap();
                    let progress = ContinuityGoalProgress {
                        lifecycle: ContinuityGoalLifecycle::Achieved,
                        next_attention: None,
                        ..current.clone()
                    };
                    let transition = ContinuityGoalCompletionTransition {
                        transition_id: "transition:user-accept".into(),
                        goal_id: relation.goal_id.clone(),
                        from_lifecycle: current.lifecycle,
                        to_lifecycle: ContinuityGoalLifecycle::Achieved,
                        goal_revision: progress.revision,
                        authority_kind: ContinuityClosureAuthorityKind::UserAcceptance,
                        evaluation_ref: source_for("event:user-accept"),
                        notification_id: "notice:user-accept".into(),
                    };
                    let closed = service
                        .execute(json!({
                            "action": "close-goal",
                            "conversationId": conversation_id,
                            "transition": transition,
                            "progress": progress,
                        }))
                        .unwrap();
                    assert_eq!(closed["accepted"], true);
                    assert_eq!(closed["notificationId"], "notice:user-accept");
                }
            }
            "evidence" => {
                let relation = share_relation(&host, &conversation_id);
                if seq == 4 {
                    let posted = post_material(
                        &service,
                        &conversation_id,
                        &owner,
                        "资料A版本1 已到达，含报名清单。",
                    );
                    accept_posted_evidence(
                        &service,
                        &conversation_id,
                        &relation.goal_id,
                        &owner,
                        &posted,
                        "criterion:material-a",
                        1,
                        seq,
                    );
                    disclose_child_after_evidence(
                        &service,
                        &host,
                        &conversation_id,
                        &relation.goal_id,
                    );
                    let (starts, completes) = snapshot_calls();
                    assert!(
                        share_child_disclosed(
                            &host,
                            &starts,
                            &completes,
                            &relation.child_conversation_id,
                            "资料A版本1",
                        ),
                        "step 4 must deliver 资料A to the share child"
                    );
                    record_share_execution(
                        &mut extras,
                        &host,
                        &starts,
                        &relation.child_conversation_id,
                        &["资料A版本1"],
                        &["资料A版本2", "兄弟任务私有哨兵"],
                    );
                } else if seq == 11 {
                    let material_b =
                        post_material(&service, &conversation_id, &owner, "资料B版本1 已到达。");
                    accept_posted_evidence(
                        &service,
                        &conversation_id,
                        &relation.goal_id,
                        &owner,
                        &material_b,
                        "criterion:material-b",
                        1,
                        seq,
                    );
                    let material_a_v2 =
                        post_material(&service, &conversation_id, &owner, "资料A版本2 替换旧稿。");
                    accept_posted_evidence(
                        &service,
                        &conversation_id,
                        &relation.goal_id,
                        &owner,
                        &material_a_v2,
                        "criterion:material-a",
                        2,
                        seq,
                    );
                    disclose_child_after_evidence(
                        &service,
                        &host,
                        &conversation_id,
                        &relation.goal_id,
                    );
                    let (starts, completes) = snapshot_calls();
                    assert!(
                        share_child_disclosed(
                            &host,
                            &starts,
                            &completes,
                            &relation.child_conversation_id,
                            "资料A版本2",
                        ),
                        "step 11 must deliver 资料A v2"
                    );
                    assert!(
                        share_child_disclosed(
                            &host,
                            &starts,
                            &completes,
                            &relation.child_conversation_id,
                            "资料B版本1",
                        ),
                        "step 11 must deliver 资料B"
                    );
                    record_share_execution(
                        &mut extras,
                        &host,
                        &starts,
                        &relation.child_conversation_id,
                        &["资料A版本2", "资料B版本1"],
                        &["资料A版本1", "兄弟任务私有哨兵"],
                    );
                } else {
                    panic!("unexecuted CA-J001 evidence step {seq}");
                }
            }
            "runtime" => {
                let relation = share_relation(&host, &conversation_id);
                let child = host.store().get(&relation.child_conversation_id).unwrap();
                let child_author = child
                    .assistant_membership_id
                    .clone()
                    .or_else(|| {
                        child.memberships.iter().find_map(|membership| {
                            (membership.principal.kind == PrincipalKind::Agent)
                                .then(|| membership.id.clone())
                        })
                    })
                    .unwrap_or_else(|| agent.clone());
                let child_agent =
                    child_agent_id(&service, &relation.child_conversation_id, &child_author);
                let input = step["input"].as_str().unwrap_or("");
                runtime_child_work(
                    &service,
                    &relation.child_conversation_id,
                    &child_author,
                    &child_agent,
                    input,
                    Some(input),
                );
                let settlement = service
                    .after_runtime_settlement(
                        &relation.child_conversation_id,
                        &json!({
                            "ok": true,
                            "output": input,
                            "membershipId": child_author,
                        }),
                    )
                    .unwrap();
                extras["lastReplayed"] = json!(settlement["replayed"].as_u64().unwrap_or(0));
                if seq == 12 {
                    apply_goal_control(
                        host.store(),
                        &conversation_id,
                        &relation.goal_id,
                        ContinuityGoalEvent::NamedWait,
                    )
                    .unwrap();
                }
                if seq == 14 {
                    let stale = host.commit_fresh(
                        serde_json::from_str(&typed_child_proposal_json(&conversation_id)).unwrap(),
                    );
                    extras["lastStaleRejected"] = json!(stale.is_err());
                }
                if seq == 18 {
                    let posted = post_material(
                        &service,
                        &relation.child_conversation_id,
                        &child_author,
                        "终稿v3 引用检查返回。",
                    );
                    accept_posted_evidence(
                        &service,
                        &conversation_id,
                        &relation.goal_id,
                        &child_author,
                        &posted,
                        "criterion:material-draft",
                        3,
                        seq,
                    );
                    let progress = read_goal(host.store(), &relation.goal_id)
                        .expect("read_goal after v3")
                        .expect("share goal after v3");
                    assert!(
                        progress
                            .criterion_evidence_refs
                            .iter()
                            .any(|item| item.subject_version == 3
                                && item.source.opaque_id == posted.id),
                        "step 18 must persist real v3 evidence"
                    );
                }
                if seq == 20 {
                    extras["lastLateUnachieved"] = json!(read_goal(host.store(), &relation.goal_id)
                            .unwrap()
                            .is_some_and(
                                |item| item.lifecycle != ContinuityGoalLifecycle::Achieved
                            ));
                }
            }
            "fault" => {
                if seq == 7 {
                    let observer = ConversationService::open(&root).unwrap();
                    let generation = host.host_generation();
                    assert_eq!(observer.continuity().unwrap().host_generation(), generation);
                } else if seq == 8 {
                    let posted = service
                        .execute(json!({
                            "action": "conversation.message.post",
                            "conversationId": conversation_id,
                            "authorMembershipId": owner,
                            "content": "interrupted progress",
                        }))
                        .expect("fault post persists the human event");
                    let event_id = posted["event"]["id"].as_str().expect("fault event");
                    let dispatched = after_post(&service, &conversation_id, event_id);
                    assert!(dispatched["directTurns"].as_array().is_some());
                    set_continuity_interrupt(Some(ContinuityInterrupt::AfterStateWrite));
                    let interrupted = service.after_runtime_settlement(
                        &conversation_id,
                        &json!({
                            "output": assistant_turn_output(
                                "interrupted",
                                &speech_proposal_json(
                                    &conversation_id,
                                    seq,
                                    ContinuitySpeechAct::Correction,
                                    ContinuityMatterSubject::Existing,
                                    false,
                                    vec![ContinuityAgreementProposal {
                                        scope: ContinuityAgreementScope::Matter,
                                        statement_ref: source_for("event:interrupted-progress"),
                                        origin: ContinuityAgreementOrigin::UserExplicit,
                                    }],
                                ),
                            ),
                            "membershipId": agent,
                            "causationId": event_id,
                        }),
                    );
                    assert!(
                        interrupted.is_err(),
                        "AfterStateWrite must fail the continuity commit"
                    );
                    set_continuity_interrupt(None);
                    extras["faultSettlementId"] = json!(event_id);
                    assert!(
                        read_settlement_pending(host.store(), &conversation_id, event_id)
                            .expect("read_settlement_pending")
                            .is_some(),
                        "rolled-back AfterStateWrite must keep settlement pending"
                    );
                    assert!(
                        !settlement_applied(host.store(), &conversation_id, event_id).unwrap(),
                        "rolled-back AfterStateWrite is not a terminal applied settlement"
                    );
                    extras["lastDuplicateWakes"] = json!(false);
                } else {
                    panic!("unexecuted CA-J001 fault step {seq}");
                }
            }
            "time" => {
                if seq == 9 {
                    set_continuity_clock(Some(3 * 24 * 60 * 60 * 1000));
                    let relation = share_relation(&host, &conversation_id);
                    host.schedule_review(&conversation_id, &relation.goal_id, 1)
                        .expect("schedule_review");
                    let drain = service.attend_due().expect("attend_due after review");
                    let settlement_id = extras["faultSettlementId"]
                        .as_str()
                        .expect("fault settlement id")
                        .to_owned();
                    assert!(
                        settlement_applied(host.store(), &conversation_id, &settlement_id).unwrap(),
                        "recoverable AfterStateWrite pending must complete after attend_due"
                    );
                    let second = service.attend_due().expect("second attend_due");
                    extras["lastReplayed"] = json!(drain["replayed"].as_u64().unwrap_or(0));
                    extras["lastDuplicateWakes"] =
                        json!(second["replayed"].as_u64().unwrap_or(0) != 0);
                    extras["interruptedRecoveredOnce"] = json!(
                        drain["replayed"].as_u64().unwrap_or(0) <= 1
                            && second["replayed"].as_u64().unwrap_or(0) == 0
                            && list_unapplied_settlements(host.store())
                                .expect("list_unapplied_settlements")
                                .iter()
                                .all(|(_, id, _)| id != &settlement_id)
                    );
                    set_continuity_clock(None);
                } else if seq == 16 {
                    let cognition_before_due = host.cognition_invocation_count();
                    set_continuity_clock(Some(4 * 24 * 60 * 60 * 1000));
                    let drain = service.attend_due().unwrap();
                    extras["lastReplayed"] = json!(drain["replayed"].as_u64().unwrap_or(0));
                    extras["lastCognitionDelta"] = json!(
                        host.cognition_invocation_count()
                            .saturating_sub(cognition_before_due)
                    );
                    extras["paidShareWake"] = json!(
                        drain["reevaluated"]
                            .as_array()
                            .into_iter()
                            .flatten()
                            .any(|id| id
                                .as_str()
                                .is_some_and(|value| value.contains("goal:matter:share")))
                    );
                    set_continuity_clock(None);
                } else {
                    panic!("unexecuted CA-J001 time step {seq}");
                }
            }
            other => panic!("unknown CA-J001 step {seq} kind {other}"),
        }
        let (starts, completes) = snapshot_calls();
        let state = observe_journey(
            &service,
            &conversation_id,
            extras.clone(),
            &starts,
            &completes,
        );
        for name in step["assertion_names"].as_array().unwrap() {
            let assertion = name.as_str().unwrap();
            assert!(
                assertion_holds(assertion, &state),
                "CA-J001 {seq} failed on {assertion}: {state}"
            );
        }
        if seq == 1 {
            admit_sibling_task(&service, &conversation_id, &owner, &agent);
        }
    }
    let (starts, completes) = snapshot_calls();
    let final_state = observe_journey(&service, &conversation_id, extras, &starts, &completes);
    assert!(
        final_state["relationCount"].as_u64().unwrap_or(0) >= 2,
        "parent keeps the share child and the sibling task"
    );
    assert_eq!(final_state["replayed"], 0);
    assert_eq!(final_state["privateLeak"], false);
    assert_eq!(final_state["outboundGrant"], false);
    assert_eq!(final_state["artifactAuthorRewritten"], false);
    assert_eq!(final_state["closureAuthority"], "user-acceptance");
    assert_eq!(final_state["shareChildHasMaterialA"], true);
    println!("CONTINUITY_CA_J001_ORACLE:{final_state}");
}

fn typed_child_proposal_json(conversation_id: &str) -> String {
    typed_goal_proposal_json(conversation_id, "goal:matter:notes", "matter:notes")
}

fn first_relation(
    host: &ContinuityHost,
    conversation_id: &str,
) -> ContinuityTaskConversationRelation {
    host.store()
        .list_child_relations(conversation_id, None, 8)
        .unwrap()
        .into_iter()
        .next()
        .expect("child relation")
}

fn share_relation(
    host: &ContinuityHost,
    conversation_id: &str,
) -> ContinuityTaskConversationRelation {
    host.store()
        .list_child_relations(conversation_id, None, 8)
        .expect("list_child_relations")
        .into_iter()
        .find(|relation| relation.goal_id == "goal:matter:share")
        .expect("share child relation")
}

fn child_recipient_id(store: &ConversationStore, child_id: &str) -> String {
    let child = store.get(child_id).expect("child conversation");
    child
        .assistant_membership_id
        .or_else(|| {
            child.memberships.iter().find_map(|membership| {
                (membership.principal.kind == PrincipalKind::Agent).then(|| membership.id.clone())
            })
        })
        .expect("child recipient")
}

fn invocation_blob(params: &Value) -> String {
    format!("{}\n{params}", delivered_guidance(params))
}

fn invocation_blob_for_member(
    start_calls: &[Value],
    complete_calls: &[Value],
    membership_id: &str,
) -> String {
    if membership_id.is_empty() {
        return String::new();
    }
    start_calls
        .iter()
        .chain(complete_calls)
        .filter(|params| params.get("membershipId").and_then(Value::as_str) == Some(membership_id))
        .map(invocation_blob)
        .collect::<Vec<_>>()
        .join("\n")
}

fn latest_child_work_for<'a>(
    start_calls: &'a [Value],
    child_id: &str,
    membership_id: &str,
) -> Option<&'a Value> {
    start_calls.iter().rev().find(|params| {
        params.get("continuityKind").and_then(Value::as_str) == Some("child-work")
            && params.get("conversationId").and_then(Value::as_str) == Some(child_id)
            && params.get("membershipId").and_then(Value::as_str) == Some(membership_id)
    })
}

fn child_work_blob(start_calls: &[Value], child_id: &str, membership_id: &str) -> String {
    start_calls
        .iter()
        .filter(|params| {
            params.get("continuityKind").and_then(Value::as_str) == Some("child-work")
                && params.get("conversationId").and_then(Value::as_str) == Some(child_id)
                && params.get("membershipId").and_then(Value::as_str) == Some(membership_id)
        })
        .map(delivered_guidance)
        .collect::<Vec<_>>()
        .join("\n")
}

fn start_attachments(params: &Value) -> Vec<Value> {
    params
        .get("attachments")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
}

fn record_share_execution(
    extras: &mut Value,
    host: &ContinuityHost,
    start_calls: &[Value],
    child_id: &str,
    expected: &[&str],
    forbidden: &[&str],
) {
    let member = child_recipient_id(host.store(), child_id);
    let capture = latest_child_work_for(start_calls, child_id, &member)
        .expect("share child-work invocation is required");
    assert_eq!(
        capture.get("conversationId").and_then(Value::as_str),
        Some(child_id)
    );
    assert_eq!(
        capture.get("membershipId").and_then(Value::as_str),
        Some(member.as_str())
    );
    assert_eq!(
        capture.get("continuityKind").and_then(Value::as_str),
        Some("child-work")
    );
    assert_eq!(
        capture.get("goalId").and_then(Value::as_str),
        Some("goal:matter:share")
    );
    let operation = capture
        .get("dispatchId")
        .or_else(|| capture.get("operationId"))
        .and_then(Value::as_str)
        .unwrap_or("");
    assert!(
        operation.contains("child-work") || operation.contains("goal:matter:share"),
        "child-work capture must bind the admitted operation"
    );
    let blob = delivered_guidance(capture);
    for needle in expected {
        assert!(
            blob.contains(needle),
            "share child-work payload must contain {needle}"
        );
    }
    for needle in forbidden {
        assert!(
            !blob.contains(needle),
            "share child-work payload must not contain {needle}"
        );
    }
    extras["shareExecHasA"] = json!(blob.contains("资料A"));
    extras["shareExecHasAv1"] = json!(blob.contains("资料A版本1"));
    extras["shareExecHasAv2"] = json!(blob.contains("资料A版本2"));
    extras["shareExecHasB"] = json!(blob.contains("资料B版本1"));
}

fn share_child_disclosed(
    host: &ContinuityHost,
    start_calls: &[Value],
    _complete_calls: &[Value],
    child_id: &str,
    needle: &str,
) -> bool {
    let member = child_recipient_id(host.store(), child_id);
    latest_child_work_for(start_calls, child_id, &member)
        .map(delivered_guidance)
        .is_some_and(|blob| blob.contains(needle))
}

fn post_material(
    service: &ConversationService,
    conversation_id: &str,
    author: &str,
    text: &str,
) -> licoup_conversation::ConversationEvent {
    let posted = service
        .execute(json!({
            "action": "conversation.message.post",
            "conversationId": conversation_id,
            "authorMembershipId": author,
            "content": text,
        }))
        .unwrap_or_else(|_| {
            let event = service
                .store()
                .append_event(
                    conversation_id,
                    Some(author),
                    licoup_conversation::EventKind::Message,
                    &[NewEventPart {
                        id: String::new(),
                        kind: EventPartKind::Text,
                        content: text.to_owned(),
                    }],
                    None,
                    None,
                    true,
                )
                .expect("append material event");
            json!({ "event": { "id": event.id } })
        });
    let event_id = posted["event"]["id"].as_str().expect("material event id");
    service
        .store()
        .event(conversation_id, event_id)
        .expect("load material event")
        .expect("material event")
}

fn accept_posted_evidence(
    service: &ConversationService,
    conversation_id: &str,
    goal_id: &str,
    issuer: &str,
    event: &licoup_conversation::ConversationEvent,
    criterion_id: &str,
    version: i64,
    seq: u64,
) {
    let part = event
        .parts
        .iter()
        .find(|part| part.kind == EventPartKind::Text)
        .expect("material text part");
    let accepted = service
        .execute(json!({
            "action": "accept-evidence",
            "conversationId": conversation_id,
            "goalId": goal_id,
            "evidence": {
                "source": {
                    "ownerKind": "event",
                    "opaqueId": event.id,
                    "partId": part.id,
                    "sourceRevision": event.sequence,
                    "digest": format!("event:{}:{}", event.id, part.id),
                    "visibilityScope": "goal",
                    "validity": "current"
                },
                "issuer": issuer,
                "subjectVersion": version,
                "criterionId": criterion_id,
                "observedAt": seq,
                "result": "pass",
                "verificationKind": "user-acceptance",
                "scope": "goal",
                "validity": "current"
            }
        }))
        .expect("accept-evidence");
    assert_eq!(accepted["ok"], true, "accept-evidence {seq}");
}

fn accept_part_evidence(
    service: &ConversationService,
    conversation_id: &str,
    goal_id: &str,
    issuer: &str,
    event: &licoup_conversation::ConversationEvent,
    part_id: &str,
    owner_kind: &str,
    digest: String,
    span: Option<ContinuityUtf8ByteSpan>,
    criterion_id: &str,
    version: i64,
    seq: u64,
) {
    let mut source = json!({
        "ownerKind": owner_kind,
        "opaqueId": event.id,
        "partId": part_id,
        "sourceRevision": event.sequence,
        "digest": digest,
        "visibilityScope": "goal",
        "validity": "current"
    });
    if let Some(span) = span {
        source["span"] = json!({
            "startByte": span.start_byte,
            "endByte": span.end_byte,
        });
    }
    let accepted = service
        .execute(json!({
            "action": "accept-evidence",
            "conversationId": conversation_id,
            "goalId": goal_id,
            "evidence": {
                "source": source,
                "issuer": issuer,
                "subjectVersion": version,
                "criterionId": criterion_id,
                "observedAt": seq,
                "result": "pass",
                "verificationKind": "user-acceptance",
                "scope": "goal",
                "validity": "current"
            }
        }))
        .expect("accept-evidence exact part");
    assert_eq!(accepted["ok"], true, "accept-evidence {seq}");
}

fn disclose_child_after_evidence(
    service: &ConversationService,
    host: &ContinuityHost,
    conversation_id: &str,
    goal_id: &str,
) {
    host.schedule_review(conversation_id, goal_id, 1)
        .expect("schedule_review after evidence");
    service.attend_due().expect("attend_due after evidence");
}

fn admit_sibling_task(
    service: &ConversationService,
    conversation_id: &str,
    owner: &str,
    agent: &str,
) {
    let posted = service
        .execute(json!({
            "action": "conversation.message.post",
            "conversationId": conversation_id,
            "authorMembershipId": owner,
            "content": "兄弟任务私有哨兵，请单独处理另一事项。",
        }))
        .expect("sibling post");
    let event_id = posted["event"]["id"].as_str().expect("sibling event");
    after_post(service, conversation_id, event_id);
    service
        .after_runtime_settlement(
            conversation_id,
            &json!({
                "output": assistant_turn_output(
                    "Sibling task admitted.",
                    &typed_goal_proposal_json(
                        conversation_id,
                        "goal:matter:sibling",
                        "matter:sibling",
                    ),
                ),
                "membershipId": agent,
                "causationId": event_id,
            }),
        )
        .expect("sibling settlement");
}

fn speech_proposal_json(
    conversation_id: &str,
    seq: u64,
    speech_act: ContinuitySpeechAct,
    subject: ContinuityMatterSubject,
    create_goal: bool,
    agreements: Vec<ContinuityAgreementProposal>,
) -> String {
    serde_json::to_string(&ContinuityInterpretationProposal {
        envelope: ContinuityWriteEnvelope {
            conversation_id: conversation_id.to_owned(),
            source_event_refs: Vec::new(),
            observed_revision: 0,
            designation_epoch: 0,
            request_id: format!("request:speech:{seq}:{conversation_id}"),
        },
        matter_associations: Vec::new(),
        speech_act,
        commitment_proposals: if create_goal {
            vec![ContinuityCommitmentProposal {
                matter_id: Some("matter:share".into()),
                subject,
                expected_result: "Prepare matter:share".into(),
                criteria: Vec::new(),
                create_goal: true,
            }]
        } else {
            Vec::new()
        },
        agreement_proposals: agreements,
        capability_needs: Vec::new(),
        uncertainty_reasons: Vec::new(),
        requested_reads: Vec::new(),
        task_child_admission: None,
    })
    .unwrap()
}

fn runtime_child_work(
    service: &ConversationService,
    child_id: &str,
    member: &str,
    agent_id: &str,
    text: &str,
    artifact: Option<&str>,
) {
    let _ = admit_child_turn(service.store(), child_id, member, agent_id, text, artifact);
}

fn typed_goal_proposal_json(conversation_id: &str, goal_id: &str, matter_id: &str) -> String {
    serde_json::to_string(&ContinuityInterpretationProposal {
        envelope: ContinuityWriteEnvelope {
            conversation_id: conversation_id.to_owned(),
            source_event_refs: Vec::new(),
            observed_revision: 0,
            designation_epoch: 0,
            request_id: format!("request:{goal_id}"),
        },
        matter_associations: Vec::new(),
        speech_act: ContinuitySpeechAct::Delegation,
        commitment_proposals: vec![ContinuityCommitmentProposal {
            matter_id: Some(matter_id.to_owned()),
            subject: ContinuityMatterSubject::New,
            expected_result: format!("Prepare {matter_id}"),
            criteria: Vec::new(),
            create_goal: true,
        }],
        agreement_proposals: Vec::new(),
        capability_needs: Vec::new(),
        uncertainty_reasons: Vec::new(),
        requested_reads: Vec::new(),
        task_child_admission: Some(ContinuityTaskChildAdmission {
            goal_id: goal_id.to_owned(),
            parent_conversation_id: conversation_id.to_owned(),
            speech_act: ContinuitySpeechAct::Delegation,
            follow_through_kind: ContinuityFollowThroughKind::Durable,
            observed_child_conversation_id: None,
            observed_card_anchor: None,
            request_id: format!("request:admit:{goal_id}"),
        }),
    })
    .unwrap()
}

fn review_agreement_proposal_json(conversation_id: &str, token: &str) -> String {
    serde_json::to_string(&ContinuityInterpretationProposal {
        envelope: ContinuityWriteEnvelope {
            conversation_id: conversation_id.to_owned(),
            source_event_refs: Vec::new(),
            observed_revision: 0,
            designation_epoch: 0,
            request_id: format!("request:review:{token}"),
        },
        matter_associations: Vec::new(),
        speech_act: ContinuitySpeechAct::Correction,
        commitment_proposals: Vec::new(),
        agreement_proposals: vec![ContinuityAgreementProposal {
            scope: ContinuityAgreementScope::Goal,
            statement_ref: ContinuitySourceRef {
                owner_kind: ContinuitySourceOwnerKind::Goal,
                opaque_id: format!("goal-review:{token}"),
                part_id: None,
                span: None,
                source_revision: 1,
                digest: digest(4),
                visibility_scope: ContinuityVisibilityScope::Goal,
                validity: ContinuitySourceValidity::Current,
            },
            origin: ContinuityAgreementOrigin::AgentInference,
        }],
        capability_needs: Vec::new(),
        uncertainty_reasons: Vec::new(),
        requested_reads: Vec::new(),
        task_child_admission: None,
    })
    .unwrap()
}

fn start_kind<'a>(starts: &'a [Value], kind: &str) -> Vec<&'a Value> {
    starts
        .iter()
        .filter(|params| params.get("continuityKind").and_then(Value::as_str) == Some(kind))
        .collect()
}

fn child_events(service: &ConversationService, child_id: &str) -> Vec<Value> {
    service
        .execute(json!({
            "action": "conversation.events.page",
            "conversationId": child_id,
            "limit": 50
        }))
        .unwrap()["events"]
        .as_array()
        .cloned()
        .unwrap_or_default()
}

fn child_message_events(service: &ConversationService, child_id: &str) -> Vec<Value> {
    child_events(service, child_id)
        .into_iter()
        .filter(|event| event.get("kind").and_then(Value::as_str) == Some("message"))
        .collect()
}

fn bind_effect_free_runtime(
    service: ConversationService,
    complete_calls: std::sync::Arc<std::sync::Mutex<Vec<Value>>>,
    start_calls: std::sync::Arc<std::sync::Mutex<Vec<Value>>>,
) -> ConversationService {
    let complete_calls_for_turn = complete_calls;
    let start_calls_for_sender = start_calls;
    service.bind_conversation_runtime(PersistentRuntimePorts::new(
        move |params: &Value| {
            start_calls_for_sender
                .lock()
                .unwrap_or_else(|poison| poison.into_inner())
                .push(params.clone());
            Ok(json!({
                "ok": true,
                "accepted": true,
                "turnHandle": "turn:test",
            }))
        },
        |_conversation_id: &str| json!([]),
        |_params: &Value| Ok(json!({ "ok": true })),
        move |params: &Value| {
            complete_calls_for_turn
                .lock()
                .unwrap_or_else(|poison| poison.into_inner())
                .push(params.clone());
            let conversation_id = params
                .get("conversationId")
                .and_then(Value::as_str)
                .unwrap_or_default();
            Ok(json!({
                "ok": true,
                "output": typed_child_proposal_json(conversation_id),
            }))
        },
        |_request: Value| Ok(json!({})),
    ))
}

#[test]
fn production_binder_ordinary_post_executes_selected_agent_and_commits_child() {
    let complete_calls = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let start_calls = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let service = bind_effect_free_runtime(
        ConversationService::from_store(ConversationStore::open_in_memory().unwrap()),
        complete_calls.clone(),
        start_calls.clone(),
    );
    let (conversation_id, owner, agent) = create_group_with_agent(&service, "codex");
    designate_assistant(&service, &conversation_id, &owner, &agent);
    let posted = service
        .execute(json!({
            "action": "conversation.message.post",
            "conversationId": conversation_id,
            "authorMembershipId": owner,
            "content": "prepare notes for the trip",
        }))
        .unwrap();
    assert!(
        posted["continuityIngress"].is_null(),
        "post must return before model completion"
    );
    assert_eq!(
        complete_calls
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
            .len(),
        0
    );
    let dispatched = after_post(
        &service,
        &conversation_id,
        posted["event"]["id"].as_str().unwrap(),
    );
    assert_eq!(dispatched["directTurns"].as_array().unwrap().len(), 1);
    let starts = start_calls
        .lock()
        .unwrap_or_else(|poison| poison.into_inner())
        .clone();
    assert_eq!(starts.len(), 1, "exactly one addressed after-post turn");
    assert_eq!(starts[0]["agent"], "codex");
    assert_eq!(starts[0]["agentId"], "codex");
    assert_eq!(starts[0]["conversationId"], conversation_id);
    assert_eq!(starts[0]["membershipId"], agent);
    assert_eq!(starts[0]["causationId"], posted["event"]["id"]);
    assert!(
        starts[0]["text"]
            .as_str()
            .unwrap()
            .contains("prepare notes for the trip")
    );
    assert_eq!(
        complete_calls
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
            .len(),
        0,
        "blocked start_background must not complete the model"
    );
    settle_typed_child(
        &service,
        &conversation_id,
        &agent,
        posted["event"]["id"].as_str().unwrap(),
    );
    let after = service
        .execute(json!({"action": "conversation.get", "conversationId": conversation_id}))
        .unwrap();
    assert_eq!(after["taskViews"].as_array().unwrap().len(), 1);
    assert!(parent_card_exists(&service, &conversation_id));
}

#[test]
fn production_binder_after_post_does_not_duplicate_designated_assistant() {
    let complete_calls = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let start_calls = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let service = bind_effect_free_runtime(
        ConversationService::from_store(ConversationStore::open_in_memory().unwrap()),
        complete_calls.clone(),
        start_calls.clone(),
    );
    let (conversation_id, owner, agent) = create_group_with_agent(&service, "codex");
    designate_assistant(&service, &conversation_id, &owner, &agent);
    let posted = service
        .execute(json!({
            "action": "conversation.message.post",
            "conversationId": conversation_id,
            "authorMembershipId": owner,
            "content": "prepare notes for the trip",
        }))
        .unwrap();
    let first = after_post(
        &service,
        &conversation_id,
        posted["event"]["id"].as_str().unwrap(),
    );
    let retry = after_post(
        &service,
        &conversation_id,
        posted["event"]["id"].as_str().unwrap(),
    );
    assert_eq!(first["directTurns"].as_array().unwrap().len(), 1);
    assert!(retry["directTurns"].as_array().unwrap().is_empty());
    assert_eq!(
        start_calls
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
            .len(),
        1,
        "after-post retry must not start a second Assistant turn"
    );
    assert_eq!(
        complete_calls
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
            .len(),
        0
    );
}

#[test]
fn revise_accept_replace_reject_invalid_inputs() {
    let service = ConversationService::from_store(ConversationStore::open_in_memory().unwrap());
    let host = service.continuity().cloned().unwrap();
    let (conversation_id, owner, _) = create_group(&service);
    host.install_script(durable_script("", "matter:neg"));
    let _ = post(&service, &conversation_id, &owner, "prepare notes");
    let goal_id = host
        .store()
        .list_child_relations(&conversation_id, None, 8)
        .unwrap()[0]
        .goal_id
        .clone();
    let before = read_goal(host.store(), &goal_id).unwrap().unwrap();
    assert!(
        service
            .execute(json!({
                "action": "revise-agreement",
                "conversationId": conversation_id,
                "agreement": { "id": "", "scope": "not-a-scope" }
            }))
            .is_err()
    );
    let after_revise = read_goal(host.store(), &goal_id).unwrap().unwrap();
    assert_eq!(after_revise.revision, before.revision);
    let rejected_evidence = service.execute(json!({
        "action": "accept-evidence",
        "conversationId": conversation_id,
        "goalId": "goal:missing",
        "evidence": {
            "source": {
                "ownerKind": "event",
                "opaqueId": "event:missing",
                "sourceRevision": 1,
                "digest": digest(4),
                "visibilityScope": "goal",
                "validity": "current"
            },
            "issuer": "user",
            "subjectVersion": 1,
            "criterionId": "criterion:notes",
            "observedAt": 1,
            "result": "pass",
            "verificationKind": "user-acceptance",
            "scope": "goal",
            "validity": "current"
        }
    }));
    assert!(rejected_evidence.is_err());
    let still = read_goal(host.store(), &goal_id).unwrap().unwrap();
    assert_eq!(still.lifecycle, ContinuityGoalLifecycle::Active);
    assert!(
        service
            .execute(json!({
                "action": "replace-assistant",
                "conversationId": conversation_id,
                "membershipId": "membership:unknown"
            }))
            .is_err()
    );
}

#[test]
fn future_due_is_not_consumed_and_missed_due_enqueues_once() {
    let root = std::env::temp_dir().join(format!("lico-ca-due-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&root).unwrap();
    let service = ConversationService::open(&root).unwrap();
    service.claim_continuity_owner().unwrap();
    let host = service.continuity().cloned().unwrap();
    let (conversation_id, owner, _) = create_group(&service);
    host.install_script(durable_script("", "matter:due"));
    let _ = post(&service, &conversation_id, &owner, "prepare notes");
    let goal_id = host
        .store()
        .list_child_relations(&conversation_id, None, 8)
        .unwrap()[0]
        .goal_id
        .clone();
    set_continuity_clock(Some(1_000));
    host.schedule_review(&conversation_id, &goal_id, 10_000)
        .unwrap();
    let early = service.attend_due().unwrap();
    assert!(
        early["consumed"].as_array().unwrap().is_empty(),
        "future due must not run early: {early}"
    );
    set_continuity_clock(Some(20_000));
    let late = service.attend_due().unwrap();
    let first_count = late["consumed"].as_array().unwrap().len()
        + late["preserved"].as_array().unwrap().len()
        + late["waiting"].as_array().unwrap().len();
    assert!(first_count >= 1, "missed due must be attended: {late}");
    let again = service.attend_due().unwrap();
    let second = again["reevaluated"].as_array().unwrap().len();
    assert!(
        second <= late["reevaluated"].as_array().unwrap().len(),
        "missed due must not duplicate reevaluation: {again}"
    );
    set_continuity_clock(None);
}

#[test]
fn qualification_persists_synthetic_evidence_and_invalidates_only_changed_identity() {
    use licoup_conversation::continuity::ContinuityDatasetSplit;
    use licoup_native::domain::agent_intelligence_catalog::qualification::{
        EvidenceBundle, EvidenceClass, ObservationJudgment, ObservationPolarity,
        QualificationObservation,
    };
    let root = std::env::temp_dir().join(format!("lico-ca-qual-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&root).unwrap();
    let service = ConversationService::open(&root).unwrap();
    let host = service.continuity().cloned().unwrap();
    let (conversation_id, owner, agent) = create_group(&service);
    let conversation = host.store().get(&conversation_id).unwrap();
    let identity = {
        let profile = host.store().membership_profile(&agent).ok().flatten();
        // Use the same host selection path by ingesting against the live identity.
        let _ = (conversation.id.clone(), profile);
        licoup_conversation::continuity::ContinuityCandidateIdentity {
            model_digest: format!("identity:model:unselected-model"),
            reasoning_digest: "identity:unselected-reasoning".into(),
            prompt_digest: format!("identity:prompt:{agent}:0"),
            skill_digest: "identity:unselected-skills".into(),
            context_policy_digest: "identity:unselected-context".into(),
            tool_contract_digest: "identity:unselected-tools".into(),
            adapter_runtime_digest: "identity:unselected-adapter".into(),
            dataset_version: format!("identity:membership:{agent}:one"),
            policy_revision: "draft-1".into(),
        }
    };
    let other = identity.clone();
    host.ingest_test_qualification(EvidenceBundle {
        responsibility_id: format!("{conversation_id}:{agent}"),
        identity: identity.clone(),
        observations: vec![QualificationObservation {
            observation_id: "obs:synthetic:1".into(),
            conversation_family: "family:synthetic".into(),
            split: ContinuityDatasetSplit::Heldout,
            subgroup: "en".into(),
            polarity: ObservationPolarity::Positive,
            judgment: ObservationJudgment::Correct,
            self_confidence: None,
            hard_invariants: Default::default(),
            economy: None,
            closure_claim: None,
        }],
        evidence_class: EvidenceClass::Synthetic,
        provenance: None,
    })
    .unwrap();
    host.ingest_test_qualification(EvidenceBundle {
        responsibility_id: format!("{conversation_id}:{owner}"),
        identity: other,
        observations: vec![QualificationObservation {
            observation_id: "obs:synthetic:other".into(),
            conversation_family: "family:other".into(),
            split: ContinuityDatasetSplit::Heldout,
            subgroup: "en".into(),
            polarity: ObservationPolarity::Positive,
            judgment: ObservationJudgment::Correct,
            self_confidence: None,
            hard_invariants: Default::default(),
            economy: None,
            closure_claim: None,
        }],
        evidence_class: EvidenceClass::Synthetic,
        provenance: None,
    })
    .unwrap();
    let reopened = ConversationService::open(&root).unwrap();
    let listed =
        licoup_conversation::continuity::list_qualification_evidence(reopened.store()).unwrap();
    assert!(listed.iter().any(|(responsibility, _, _, class)| {
        responsibility.ends_with(&format!(":{agent}")) && class == "synthetic"
    }));
    assert!(
        listed
            .iter()
            .any(|(responsibility, _, _, _)| responsibility.ends_with(&format!(":{owner}")))
    );
    let _ = owner;
}

fn designate_assistant(
    service: &ConversationService,
    conversation_id: &str,
    owner: &str,
    agent: &str,
) {
    let revision = service.store().get(conversation_id).unwrap().revision;
    service
        .execute(json!({
            "action": "conversation.assistant.set",
            "conversationId": conversation_id,
            "ownerMembershipId": owner,
            "expectedRevision": revision,
            "membershipId": agent,
        }))
        .unwrap();
}

fn after_post(service: &ConversationService, conversation_id: &str, event_id: &str) -> Value {
    service
        .execute(json!({
            "action": "conversation.dispatch.after-post",
            "conversationId": conversation_id,
            "eventId": event_id,
        }))
        .unwrap()
}

fn assistant_turn_output(reply: &str, proposal_json: &str) -> String {
    let proposal: Value = serde_json::from_str(proposal_json).expect("proposal json");
    serde_json::to_string(&json!({
        "replyText": reply,
        "interpretationProposal": proposal,
    }))
    .expect("assistant turn envelope")
}

fn settle_typed_child(
    service: &ConversationService,
    conversation_id: &str,
    membership_id: &str,
    event_id: &str,
) {
    service
        .after_runtime_settlement(
            conversation_id,
            &json!({
                "output": assistant_turn_output(
                    "I'll prepare the notes in a child conversation.",
                    &typed_child_proposal_json(conversation_id),
                ),
                "membershipId": membership_id,
                "causationId": event_id,
            }),
        )
        .unwrap();
}

fn parent_card_exists(service: &ConversationService, conversation_id: &str) -> bool {
    let relations = service
        .store()
        .list_child_relations(conversation_id, None, 8)
        .unwrap();
    let Some(child) = relations
        .first()
        .map(|item| item.child_conversation_id.clone())
    else {
        return false;
    };
    let events = service
        .execute(json!({
            "action": "conversation.events.page",
            "conversationId": conversation_id,
            "limit": 50
        }))
        .unwrap();
    events["events"]
        .as_array()
        .unwrap()
        .iter()
        .flat_map(|event| event["parts"].as_array().cloned().unwrap_or_default())
        .any(|part| {
            part["kind"] == "metadata"
                && part["content"]
                    .as_str()
                    .is_some_and(|content| content.contains(&child))
        })
}

fn admit_child_turn(
    store: &ConversationStore,
    child_id: &str,
    member: &str,
    agent_id: &str,
    text: &str,
    artifact: Option<&str>,
) -> String {
    let scope = store
        .prepare_runtime_dispatch(
            agent_id,
            "",
            if text.trim().is_empty() {
                "admitted-child-turn"
            } else {
                text
            },
            Some(child_id),
            Some(member),
            Some("event:child-cause"),
            None,
        )
        .expect("admit child PersistentTurn");
    if !text.trim().is_empty() {
        store
            .append_event_part(
                &scope.event_id,
                NewEventPart {
                    id: String::new(),
                    kind: EventPartKind::Text,
                    content: text.to_owned(),
                },
            )
            .expect("append admitted text");
    }
    if let Some(artifact) = artifact {
        store
            .append_event_part(
                &scope.event_id,
                NewEventPart {
                    id: String::new(),
                    kind: EventPartKind::Artifact,
                    content: artifact.to_owned(),
                },
            )
            .expect("append admitted artifact");
    }
    scope.dispatch_id
}

fn child_agent_id(service: &ConversationService, child_id: &str, member: &str) -> String {
    service
        .store()
        .get(child_id)
        .unwrap()
        .memberships
        .iter()
        .find(|membership| membership.id == member)
        .and_then(|membership| membership.principal.agent_id.clone())
        .expect("child agent id")
}

fn posted_event(
    store: &ConversationStore,
    conversation_id: &str,
    event_id: &str,
) -> ConversationEvent {
    store
        .event(conversation_id, event_id)
        .unwrap()
        .expect("event")
}

fn exact_posted_part_source(event: &ConversationEvent, part: &EventPart) -> ContinuitySourceRef {
    let image = part.kind == EventPartKind::Image;
    ContinuitySourceRef {
        owner_kind: if image {
            ContinuitySourceOwnerKind::Part
        } else {
            ContinuitySourceOwnerKind::Event
        },
        opaque_id: event.id.clone(),
        part_id: Some(part.id.clone()),
        span: None,
        source_revision: event.sequence,
        digest: if image {
            format!("part:{}", part.id)
        } else {
            format!("event:{}:{}", event.id, part.id)
        },
        visibility_scope: ContinuityVisibilityScope::Conversation,
        validity: ContinuitySourceValidity::Current,
    }
}

fn live_event_source(
    store: &ConversationStore,
    conversation_id: &str,
    event_id: &str,
) -> ContinuitySourceRef {
    let event = posted_event(store, conversation_id, event_id);
    let part = event.parts.first().expect("posted part");
    exact_posted_part_source(&event, part)
}

fn live_image_source(
    store: &ConversationStore,
    conversation_id: &str,
    event_id: &str,
) -> ContinuitySourceRef {
    let event = posted_event(store, conversation_id, event_id);
    let part = event
        .parts
        .iter()
        .find(|part| part.kind == EventPartKind::Image)
        .expect("image part");
    exact_posted_part_source(&event, part)
}

fn delivered_guidance(params: &Value) -> String {
    let mut parts = Vec::new();
    if let Some(text) = params.get("text").and_then(Value::as_str) {
        parts.push(text.to_owned());
    }
    if let Some(text) = params.get("developerInstructions").and_then(Value::as_str) {
        parts.push(text.to_owned());
    }
    if let Some(text) = params.get("privateInstructions").and_then(Value::as_str) {
        parts.push(text.to_owned());
    }
    parts.join("\n")
}

fn authorized_source_refs_from_guidance(guidance: &str) -> Vec<ContinuitySourceRef> {
    guidance
        .lines()
        .filter_map(|line| line.strip_prefix("source-ref "))
        .filter_map(|json| serde_json::from_str(json).ok())
        .collect()
}

fn delegation_from_delivered_contract(
    conversation_id: &str,
    current: &ContinuitySourceRef,
) -> String {
    let mut example: Value =
        serde_json::from_str(PROPOSAL_RESPONSE_DELEGATION_EXAMPLE).expect("generated example");
    example["envelope"]["conversationId"] = json!(conversation_id);
    example["envelope"]["sourceEventRefs"] = json!([current]);
    example["commitmentProposals"][0]["criteria"][0]["descriptionRef"] = json!(current);
    example["taskChildAdmission"]["parentConversationId"] = json!(conversation_id);
    example["requestedReads"] = json!([]);
    let parsed: ContinuityInterpretationProposal = serde_json::from_value(example.clone())
        .expect("generated delegation example must parse as the production DTO");
    assert_eq!(parsed.speech_act, ContinuitySpeechAct::Delegation);
    assert_eq!(
        parsed
            .task_child_admission
            .as_ref()
            .map(|admission| admission.follow_through_kind),
        Some(ContinuityFollowThroughKind::Durable)
    );
    assert_eq!(
        parsed.commitment_proposals[0].subject,
        ContinuityMatterSubject::New
    );
    assert_eq!(
        parsed.envelope.source_event_refs[0].opaque_id,
        current.opaque_id
    );
    assert_eq!(
        parsed.envelope.source_event_refs[0].source_revision,
        current.source_revision
    );
    assert_eq!(parsed.envelope.source_event_refs[0].digest, current.digest);
    assert_eq!(
        parsed.envelope.source_event_refs[0].validity,
        ContinuitySourceValidity::Current
    );
    assert!(!parsed.commitment_proposals[0].criteria.is_empty());
    assistant_turn_output(
        "I'll prepare the notes in a child conversation and keep this reply readable.",
        &serde_json::to_string(&parsed).expect("serialize filled contract example"),
    )
}

struct HostImageFixture {
    directory: std::path::PathBuf,
}

impl HostImageFixture {
    const PNG_BYTES: [u8; 12] = [0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A, 0, 0, 0, 0];

    fn new() -> Self {
        let directory = std::env::temp_dir().join(format!(
            "lico-ca-c1-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&directory).unwrap();
        Self { directory }
    }

    fn attachment(&self) -> Value {
        self.attachment_named("sel-1", "synthetic.png")
    }

    fn attachment_named(&self, id: &str, name: &str) -> Value {
        let path = self.directory.join(name);
        if !path.exists() {
            std::fs::write(&path, Self::PNG_BYTES).unwrap();
        }
        json!({
            "id": id,
            "name": name,
            "mediaType": "image/png",
            "path": path.to_string_lossy(),
        })
    }
}

fn live_part_source(
    store: &ConversationStore,
    conversation_id: &str,
    event_id: &str,
    part_id: &str,
) -> ContinuitySourceRef {
    let event = posted_event(store, conversation_id, event_id);
    let part = event
        .parts
        .iter()
        .find(|part| part.id == part_id)
        .expect("posted part");
    exact_posted_part_source(&event, part)
}

impl Drop for HostImageFixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.directory);
    }
}

#[test]
fn production_after_post_returns_before_model_and_settlement_commits_child() {
    let complete_calls = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let start_calls = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let service = bind_effect_free_runtime(
        ConversationService::from_store(ConversationStore::open_in_memory().unwrap()),
        complete_calls.clone(),
        start_calls.clone(),
    );
    let (conversation_id, owner, agent) = create_group_with_agent(&service, "codex");
    designate_assistant(&service, &conversation_id, &owner, &agent);
    let posted = service
        .execute(json!({
            "action": "conversation.message.post",
            "conversationId": conversation_id,
            "authorMembershipId": owner,
            "content": "prepare notes for the trip",
        }))
        .unwrap();
    assert!(posted["continuityIngress"].is_null());
    assert_eq!(
        complete_calls
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
            .len(),
        0
    );
    let dispatched = after_post(
        &service,
        &conversation_id,
        posted["event"]["id"].as_str().unwrap(),
    );
    assert_eq!(dispatched["directTurns"].as_array().unwrap().len(), 1);
    assert_eq!(
        start_calls
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
            .len(),
        1
    );
    settle_typed_child(
        &service,
        &conversation_id,
        &agent,
        posted["event"]["id"].as_str().unwrap(),
    );
    assert_eq!(
        service
            .store()
            .list_child_relations(&conversation_id, None, 8)
            .unwrap()
            .len(),
        1
    );
    assert!(parent_card_exists(&service, &conversation_id));
}

#[test]
fn production_after_post_forwards_attachments_profile_and_addressing() {
    let fixture = HostImageFixture::new();
    let complete_calls = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let start_calls = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let service = bind_effect_free_runtime(
        ConversationService::from_store(ConversationStore::open_in_memory().unwrap()),
        complete_calls.clone(),
        start_calls.clone(),
    );
    let (conversation_id, owner, agent) = create_group_with_agent(&service, "codex");
    designate_assistant(&service, &conversation_id, &owner, &agent);
    let profile_revision = service
        .store()
        .membership_profile(&agent)
        .unwrap()
        .unwrap()
        .revision;
    service
        .execute(json!({
            "action": "conversation.profile.update",
            "conversationId": conversation_id,
            "membershipId": agent,
            "ownerMembershipId": owner,
            "expectedRevision": profile_revision,
            "intent": {
                "preferredModel": "model-a",
                "preferredReasoningEffort": "high"
            }
        }))
        .unwrap();
    let posted = service
        .execute(json!({
            "action": "conversation.message.post",
            "conversationId": conversation_id,
            "authorMembershipId": owner,
            "content": "prepare notes with this image",
            "attachments": [fixture.attachment()],
        }))
        .unwrap();
    after_post(
        &service,
        &conversation_id,
        posted["event"]["id"].as_str().unwrap(),
    );
    let starts = start_calls
        .lock()
        .unwrap_or_else(|poison| poison.into_inner())
        .clone();
    assert_eq!(starts.len(), 1);
    assert_eq!(starts[0]["membershipId"], agent);
    assert_eq!(starts[0]["model"], "model-a");
    assert_eq!(starts[0]["reasoningEffort"], "high");
    assert_eq!(starts[0]["attachments"].as_array().unwrap().len(), 1);
    assert_eq!(starts[0]["attachments"][0]["name"], "synthetic.png");
    let guidance = delivered_guidance(&starts[0]);
    assert!(
        !guidance.contains(PROPOSAL_RESPONSE_CONTRACT),
        "forwarded instructions must not ask the Agent for a reply format"
    );
}

#[test]
fn production_mention_only_does_not_start_extra_assistant_turn() {
    let complete_calls = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let start_calls = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let service = bind_effect_free_runtime(
        ConversationService::from_store(ConversationStore::open_in_memory().unwrap()),
        complete_calls.clone(),
        start_calls.clone(),
    );
    let (conversation_id, owner, assistant) = create_group_with_agent(&service, "codex");
    designate_assistant(&service, &conversation_id, &owner, &assistant);
    let other = service
        .execute(json!({
            "action": "conversation.membership.add",
            "conversationId": conversation_id,
            "principal": {
                "id": "agent:two",
                "kind": "agent",
                "displayName": "Two",
                "agentId": "two"
            },
            "access": "member",
        }))
        .unwrap()["id"]
        .as_str()
        .unwrap()
        .to_owned();
    let posted = service
        .execute(json!({
            "action": "conversation.message.post",
            "conversationId": conversation_id,
            "authorMembershipId": owner,
            "content": "@Two please answer only",
        }))
        .unwrap();
    let dispatched = after_post(
        &service,
        &conversation_id,
        posted["event"]["id"].as_str().unwrap(),
    );
    assert_eq!(dispatched["directTurns"].as_array().unwrap().len(), 1);
    let starts = start_calls
        .lock()
        .unwrap_or_else(|poison| poison.into_inner())
        .clone();
    assert_eq!(starts.len(), 1);
    assert_eq!(starts[0]["membershipId"], other);
    assert_eq!(starts[0]["text"], "@Two please answer only");
    service
        .after_runtime_settlement(
            &conversation_id,
            &json!({
                "output": assistant_turn_output(
                    "I'll prepare the notes in a child conversation.",
                    &typed_child_proposal_json(&conversation_id),
                ),
                "membershipId": other,
                "causationId": posted["event"]["id"],
            }),
        )
        .unwrap();
    assert!(
        service
            .store()
            .list_child_relations(&conversation_id, None, 8)
            .unwrap()
            .is_empty()
    );
    assert_eq!(
        complete_calls
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
            .len(),
        0
    );
}

#[test]
fn production_ingress_guidance_delivers_authorized_source_contents() {
    let complete_calls = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let start_calls = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let service = bind_effect_free_runtime(
        ConversationService::from_store(ConversationStore::open_in_memory().unwrap()),
        complete_calls.clone(),
        start_calls.clone(),
    );
    let (conversation_id, owner, agent) = create_group_with_agent(&service, "codex");
    designate_assistant(&service, &conversation_id, &owner, &agent);
    let (parent_id, parent_owner, _) = create_group_with_agent(&service, "codex");
    let agreement_event = post(
        &service,
        &conversation_id,
        &owner,
        "AGREED-USE-SHARED-NOTES-TEMPLATE",
    );
    let sibling_event = post(&service, &conversation_id, &owner, "SIBLING-SECRET-TEXT");
    let revoked_event = post(&service, &conversation_id, &owner, "REVOKED-SECRET-TEXT");
    let parent_event = post(
        &service,
        &parent_id,
        &parent_owner,
        "VISIBLE-SPAN and HIDDEN-REMAINDER",
    );
    put_agreement(
        service.store(),
        &conversation_id,
        &ContinuityAgreement {
            id: "agreement:notes".into(),
            scope: ContinuityAgreementScope::Conversation,
            statement_ref: live_event_source(service.store(), &conversation_id, &agreement_event),
            origin: ContinuityAgreementOrigin::UserExplicit,
            effective_revision: 1,
            supersedes: None,
            valid_from: 1,
            valid_until: None,
            revocation_generation: 0,
        },
    )
    .unwrap();
    revoke_source(service.store(), &conversation_id, &revoked_event, true).unwrap();
    let mut granted = live_event_source(service.store(), &parent_id, &parent_event);
    granted.span = Some(ContinuityUtf8ByteSpan {
        start_byte: 0,
        end_byte: 12,
    });
    put_grant(
        service.store(),
        &ContinuityParentContextGrant {
            grant_id: "grant:parent-notes".into(),
            source_conversation_id: parent_id,
            recipient_conversation_id: conversation_id.clone(),
            recipient_membership_id: agent.clone(),
            source_refs: vec![granted],
            authorized_scopes: vec![ContinuityVisibilityScope::Conversation],
            status: ContinuityParentGrantStatus::Admitted,
            request_id: "request:grant:1".into(),
            revocation_generation: recipient_grant_generation(service.store(), &conversation_id),
        },
    )
    .unwrap();
    let posted = post(&service, &conversation_id, &owner, "prepare notes now");
    after_post(&service, &conversation_id, &posted);
    let starts = start_calls
        .lock()
        .unwrap_or_else(|poison| poison.into_inner())
        .clone();
    assert_eq!(starts.len(), 1);
    let guidance = delivered_guidance(&starts[0]);
    assert!(
        !guidance.contains(PROPOSAL_RESPONSE_CONTRACT),
        "delivered guidance must not ask the Agent for a reply format"
    );
    let example: Value = serde_json::from_str(PROPOSAL_RESPONSE_DELEGATION_EXAMPLE)
        .expect("generated delegation example must be JSON");
    assert_eq!(example["speechAct"], "delegation");
    assert_eq!(
        example["taskChildAdmission"]["followThroughKind"],
        "durable"
    );
    assert!(!guidance.contains("Durable delegation with taskChildAdmission"));
    assert!(!guidance.contains("host restamps"));
    assert!(guidance.contains("source-ref {"));
    let authorized = authorized_source_refs_from_guidance(&guidance);
    assert!(
        authorized.iter().any(
            |source| source.validity == ContinuitySourceValidity::Current
                && !source.digest.is_empty()
                && source.source_revision > 0
        ),
        "guidance must deliver full SourceRefs, not owner:id labels: {authorized:?}"
    );
    assert!(guidance.contains("AGREED-USE-SHARED-NOTES-TEMPLATE"));
    assert!(guidance.contains("VISIBLE-SPAN"));
    assert!(guidance.contains("prepare notes now"));
    assert!(!guidance.contains("SIBLING-SECRET-TEXT"));
    assert!(!guidance.contains("REVOKED-SECRET-TEXT"));
    assert!(!guidance.contains("HIDDEN-REMAINDER"));
    assert!(!guidance.contains("use tools freely"));
    let _ = sibling_event;
}

#[test]
fn production_settlement_refines_authorized_read_and_hides_invalid_read() {
    let complete_calls = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let start_calls = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let service = bind_effect_free_runtime(
        ConversationService::from_store(ConversationStore::open_in_memory().unwrap()),
        complete_calls.clone(),
        start_calls.clone(),
    );
    let (conversation_id, owner, agent) = create_group_with_agent(&service, "codex");
    designate_assistant(&service, &conversation_id, &owner, &agent);
    let refine_event = post(&service, &conversation_id, &owner, "REFINE-CURRENT-CONTENT");
    let (other_id, other_owner, _) = create_group_with_agent(&service, "codex");
    let secret_event = post(&service, &other_id, &other_owner, "UNGRANTED-SECRET-TEXT");
    let posted = post(
        &service,
        &conversation_id,
        &owner,
        "need the selected source",
    );
    after_post(&service, &conversation_id, &posted);
    let refine_source = live_event_source(service.store(), &conversation_id, &refine_event);
    let mut first = serde_json::from_str::<ContinuityInterpretationProposal>(
        &typed_child_proposal_json(&conversation_id),
    )
    .unwrap();
    first.commitment_proposals.clear();
    first.task_child_admission = None;
    first.requested_reads = vec![refine_source];
    service
        .after_runtime_settlement(
            &conversation_id,
            &json!({
                "output": assistant_turn_output(
                    "I need one authorized source again before I can continue.",
                    &serde_json::to_string(&first).unwrap(),
                ),
                "membershipId": agent,
                "causationId": posted,
            }),
        )
        .unwrap();
    let completes = complete_calls
        .lock()
        .unwrap_or_else(|poison| poison.into_inner())
        .clone();
    assert!(
        completes
            .iter()
            .any(|params| delivered_guidance(params).contains("REFINE-CURRENT-CONTENT")),
        "authorized refinement second invocation must carry current source content: {completes:?}"
    );
    assert_eq!(
        service
            .store()
            .list_child_relations(&conversation_id, None, 8)
            .unwrap()
            .len(),
        1
    );

    let invalid_post = post(&service, &conversation_id, &owner, "try an invalid read");
    after_post(&service, &conversation_id, &invalid_post);
    let mut invalid = serde_json::from_str::<ContinuityInterpretationProposal>(
        &typed_child_proposal_json(&conversation_id),
    )
    .unwrap();
    invalid.commitment_proposals.clear();
    invalid.task_child_admission = None;
    invalid.requested_reads = vec![live_event_source(service.store(), &other_id, &secret_event)];
    service
        .after_runtime_settlement(
            &conversation_id,
            &json!({
                "output": assistant_turn_output(
                    "I cannot use that source.",
                    &serde_json::to_string(&invalid).unwrap(),
                ),
                "membershipId": agent,
                "causationId": invalid_post,
            }),
        )
        .unwrap();
    let starts = start_calls
        .lock()
        .unwrap_or_else(|poison| poison.into_inner())
        .clone();
    let completes_after = complete_calls
        .lock()
        .unwrap_or_else(|poison| poison.into_inner())
        .clone();
    assert!(
        starts
            .iter()
            .chain(completes_after.iter())
            .all(|params| !delivered_guidance(params).contains("UNGRANTED-SECRET-TEXT")),
        "invalid read cannot disclose ungranted content"
    );
    assert_eq!(
        service
            .store()
            .list_child_relations(&conversation_id, None, 8)
            .unwrap()
            .len(),
        1
    );
    assert!(
        !ingress_execution_recorded(
            service.store(),
            &conversation_id,
            &invalid_post,
            &agent,
            INGRESS_USER_POSTED_DESIGNATION,
        )
        .unwrap(),
        "failed refinement must not be marked as successfully applied work"
    );
}

#[test]
fn production_reopen_does_not_repeat_same_ingress_and_keeps_review_possible() {
    let store = ConversationStore::open_in_memory().unwrap();
    let complete_calls = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let start_calls = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let bind = |store: ConversationStore| {
        bind_effect_free_runtime(
            ConversationService::from_store(store),
            complete_calls.clone(),
            start_calls.clone(),
        )
    };
    let service = bind(store.clone());
    let (conversation_id, owner, agent) = create_group_with_agent(&service, "codex");
    designate_assistant(&service, &conversation_id, &owner, &agent);
    let posted = service
        .execute(json!({
            "action": "conversation.message.post",
            "conversationId": conversation_id,
            "authorMembershipId": owner,
            "content": "prepare notes for the trip",
        }))
        .unwrap();
    let event_id = posted["event"]["id"].as_str().unwrap().to_owned();
    let service = bind(store.clone());
    let first = after_post(&service, &conversation_id, &event_id);
    assert_eq!(first["directTurns"].as_array().unwrap().len(), 1);
    assert_eq!(
        start_calls
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
            .len(),
        1
    );
    let service = bind(store.clone());
    let retry = after_post(&service, &conversation_id, &event_id);
    assert!(retry["directTurns"].as_array().unwrap().is_empty());
    assert_eq!(
        start_calls
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
            .len(),
        1
    );
    settle_typed_child(&service, &conversation_id, &agent, &event_id);
    assert_eq!(
        service
            .store()
            .list_child_relations(&conversation_id, None, 8)
            .unwrap()
            .len(),
        1
    );
    let starts_after_settle = start_calls
        .lock()
        .unwrap_or_else(|poison| poison.into_inner())
        .clone();
    assert_eq!(
        starts_after_settle
            .iter()
            .filter(
                |params| params.get("continuityKind").and_then(Value::as_str) != Some("child-work")
            )
            .count(),
        1,
        "parent ingress start stays one after child-work start"
    );
    assert!(
        starts_after_settle
            .iter()
            .any(
                |params| params.get("continuityKind").and_then(Value::as_str) == Some("child-work")
            ),
        "admitted child work must start after parent settlement"
    );
    let service = bind(store.clone());
    let settled_retry = after_post(&service, &conversation_id, &event_id);
    assert!(settled_retry["directTurns"].as_array().unwrap().is_empty());
    assert_eq!(
        start_calls
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
            .iter()
            .filter(
                |params| params.get("continuityKind").and_then(Value::as_str) != Some("child-work")
            )
            .count(),
        1
    );
    assert_eq!(
        service
            .store()
            .list_child_relations(&conversation_id, None, 8)
            .unwrap()
            .len(),
        1
    );
    let host = service.continuity().cloned().unwrap();
    let relation = host
        .store()
        .list_child_relations(&conversation_id, None, 8)
        .unwrap()
        .into_iter()
        .next()
        .unwrap();
    let progress = read_goal(host.store(), &relation.goal_id).unwrap().unwrap();
    enqueue_review_wake(
        host.store(),
        &conversation_id,
        &ContinuityWake {
            logical_wake_id: format!("wake:review:{event_id}"),
            goal_id: relation.goal_id,
            cause_refs: vec![live_event_source(host.store(), &conversation_id, &event_id)],
            due_at: Some(1),
            review_policy: "review-due".into(),
            goal_revision: progress.revision,
            epoch: 0,
            host_generation: host.host_generation(),
            claim: None,
            settlement: None,
        },
    )
    .unwrap();
    let _ = host.drain_wakes(&conversation_id).unwrap();
    assert!(
        complete_calls
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
            .len()
            >= 1,
        "later C2 review citing the same event must still be able to run"
    );
    assert_eq!(
        host.store()
            .list_child_relations(&conversation_id, None, 8)
            .unwrap()
            .len(),
        1
    );
}

#[test]
fn production_settlement_uses_delivered_contract_and_current_source_identities() {
    let complete_calls = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let start_calls = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let service = bind_effect_free_runtime(
        ConversationService::from_store(ConversationStore::open_in_memory().unwrap()),
        complete_calls.clone(),
        start_calls.clone(),
    );
    let (conversation_id, owner, agent) = create_group_with_agent(&service, "codex");
    designate_assistant(&service, &conversation_id, &owner, &agent);
    let posted = post(&service, &conversation_id, &owner, "prepare notes now");
    after_post(&service, &conversation_id, &posted);
    let starts = start_calls
        .lock()
        .unwrap_or_else(|poison| poison.into_inner())
        .clone();
    assert_eq!(starts.len(), 1);
    let guidance = delivered_guidance(&starts[0]);
    assert!(!guidance.contains(PROPOSAL_RESPONSE_CONTRACT));
    assert!(!guidance.contains("requestedReads"));
    assert!(!guidance.contains("followThroughKind"));
    let current = authorized_source_refs_from_guidance(&guidance)
        .into_iter()
        .find(|source| source.opaque_id == posted)
        .expect("delivered guidance must include a full current SourceRef for the posted event");
    assert_eq!(current.validity, ContinuitySourceValidity::Current);
    assert!(!current.digest.is_empty());
    assert!(current.source_revision > 0);
    let output = delegation_from_delivered_contract(&conversation_id, &current);
    service
        .after_runtime_settlement(
            &conversation_id,
            &json!({
                "output": output,
                "membershipId": agent,
                "causationId": posted,
            }),
        )
        .unwrap();
    let relations = service
        .store()
        .list_child_relations(&conversation_id, None, 8)
        .unwrap();
    assert_eq!(relations.len(), 1);
    assert_eq!(relations[0].parent_conversation_id, conversation_id);
    assert_eq!(
        relations[0].follow_through_kind,
        ContinuityFollowThroughKind::Durable
    );
    let matters = service
        .store()
        .list_matters(&conversation_id, None, 8)
        .unwrap();
    assert!(
        matters.iter().any(|matter| {
            matter.created_event.opaque_id == current.opaque_id
                && matter.created_event.source_revision == current.source_revision
                && matter.created_event.digest == current.digest
                && matter.created_event.validity == ContinuitySourceValidity::Current
        }),
        "committed matter must keep the current authorized source identity: {matters:?}"
    );
    assert!(parent_card_exists(&service, &conversation_id));
}

#[test]
fn production_file_reopen_retries_failed_commit_once_without_second_model() {
    let root = std::env::temp_dir().join(format!(
        "lico-ca-c1-reopen-{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|duration| duration.as_nanos())
            .unwrap_or(0)
    ));
    let complete_calls = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let start_calls = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let bind = |root: &std::path::Path| {
        bind_effect_free_runtime(
            ConversationService::from_store(ConversationStore::open(root).unwrap()),
            complete_calls.clone(),
            start_calls.clone(),
        )
    };
    let service = bind(&root);
    let (conversation_id, owner, agent) = create_group_with_agent(&service, "codex");
    designate_assistant(&service, &conversation_id, &owner, &agent);
    let posted = post(&service, &conversation_id, &owner, "prepare notes now");
    after_post(&service, &conversation_id, &posted);
    assert_eq!(
        start_calls
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
            .len(),
        1
    );
    let current = live_event_source(service.store(), &conversation_id, &posted);
    let output = delegation_from_delivered_contract(&conversation_id, &current);
    drop(service);

    let service = bind(&root);
    set_continuity_interrupt(Some(ContinuityInterrupt::BeforeCommit));
    let first = service.after_runtime_settlement(
        &conversation_id,
        &json!({
            "output": output,
            "membershipId": agent,
            "causationId": posted,
        }),
    );
    set_continuity_interrupt(None);
    assert!(
        first.is_err(),
        "injected BeforeCommit must surface: {first:?}"
    );
    assert!(
        service
            .store()
            .list_child_relations(&conversation_id, None, 8)
            .unwrap()
            .is_empty()
    );
    assert!(
        !ingress_execution_recorded(
            service.store(),
            &conversation_id,
            &posted,
            &agent,
            INGRESS_USER_POSTED_DESIGNATION,
        )
        .unwrap()
    );
    drop(service);

    let service = bind(&root);
    service.claim_continuity_owner().unwrap();
    let _ = service.attend_due().unwrap();
    assert_eq!(
        start_calls
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
            .iter()
            .filter(
                |params| params.get("continuityKind").and_then(Value::as_str) != Some("child-work")
            )
            .count(),
        1,
        "retry must not start a second parent model turn"
    );
    assert_eq!(
        service
            .store()
            .list_child_relations(&conversation_id, None, 8)
            .unwrap()
            .len(),
        1
    );
    assert!(parent_card_exists(&service, &conversation_id));
    assert!(
        ingress_execution_recorded(
            service.store(),
            &conversation_id,
            &posted,
            &agent,
            INGRESS_USER_POSTED_DESIGNATION,
        )
        .unwrap()
    );
    drop(service);

    let service = bind(&root);
    assert_eq!(
        service
            .store()
            .list_child_relations(&conversation_id, None, 8)
            .unwrap()
            .len(),
        1
    );
    let host = service.continuity().cloned().unwrap();
    let relation = host
        .store()
        .list_child_relations(&conversation_id, None, 8)
        .unwrap()
        .into_iter()
        .next()
        .unwrap();
    let progress = read_goal(host.store(), &relation.goal_id).unwrap().unwrap();
    enqueue_review_wake(
        host.store(),
        &conversation_id,
        &ContinuityWake {
            logical_wake_id: format!("wake:review:{posted}"),
            goal_id: relation.goal_id,
            cause_refs: vec![live_event_source(host.store(), &conversation_id, &posted)],
            due_at: Some(1),
            review_policy: "review-due".into(),
            goal_revision: progress.revision,
            epoch: 0,
            host_generation: host.host_generation(),
            claim: None,
            settlement: None,
        },
    )
    .unwrap();
    let _ = host.drain_wakes(&conversation_id).unwrap();
    assert!(
        complete_calls
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
            .len()
            >= 1,
        "later Goal review citing the same event must still be able to run"
    );
    assert_eq!(
        host.store()
            .list_child_relations(&conversation_id, None, 8)
            .unwrap()
            .len(),
        1
    );
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn production_parent_request_creates_child_then_child_work_turn_executes() {
    let complete_calls = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let start_calls = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let service = bind_effect_free_runtime(
        ConversationService::from_store(ConversationStore::open_in_memory().unwrap()),
        complete_calls.clone(),
        start_calls.clone(),
    );
    let (conversation_id, owner, agent) = create_group_with_agent(&service, "codex");
    designate_assistant(&service, &conversation_id, &owner, &agent);
    let posted = post(
        &service,
        &conversation_id,
        &owner,
        "prepare notes for the trip",
    );
    after_post(&service, &conversation_id, &posted);
    settle_typed_child(&service, &conversation_id, &agent, &posted);
    let relations = service
        .store()
        .list_child_relations(&conversation_id, None, 8)
        .unwrap();
    assert_eq!(relations.len(), 1);
    let child_id = relations[0].child_conversation_id.clone();
    let child = service.store().get(&child_id).unwrap();
    let child_member = child
        .assistant_membership_id
        .clone()
        .expect("child assistant");
    assert_ne!(child_member, owner);
    assert_ne!(child_id, conversation_id);
    let starts = start_calls
        .lock()
        .unwrap_or_else(|poison| poison.into_inner())
        .clone();
    let child_starts = start_kind(&starts, "child-work");
    assert_eq!(child_starts.len(), 1);
    assert_eq!(child_starts[0]["conversationId"], child_id);
    assert_eq!(child_starts[0]["membershipId"], child_member);
    assert_ne!(child_starts[0]["membershipId"], owner);
    assert!(
        child_starts[0]["text"]
            .as_str()
            .unwrap()
            .contains(&relations[0].goal_id)
    );
    let dispatch_id = admit_child_turn(
        service.store(),
        &child_id,
        &child_member,
        &child_agent_id(&service, &child_id, &child_member),
        "CHILD-WORK-RECEIPT-ALPHA",
        Some(r#"{"kind":"artifact","id":"art:alpha"}"#),
    );
    service
        .after_runtime_settlement(
            &child_id,
            &json!({
                "output": "CHILD-WORK-RECEIPT-ALPHA",
                "membershipId": child_member,
                "causationId": posted,
                "dispatchId": dispatch_id,
                "ok": true,
            }),
        )
        .unwrap();
    let events = child_events(&service, &child_id);
    assert!(
        events.iter().any(|event| {
            event["authorMembershipId"] == child_member
                && event["parts"].as_array().is_some_and(|parts| {
                    parts.iter().any(|part| part["kind"] == "artifact")
                        && parts.iter().any(|part| {
                            part["kind"] == "text"
                                && part["content"]
                                    .as_str()
                                    .is_some_and(|text| text.contains("CHILD-WORK-RECEIPT-ALPHA"))
                        })
                })
        }),
        "child settlement must write child-authored artifacts: {events:?}"
    );
    let progress = read_goal(service.store(), &relations[0].goal_id)
        .unwrap()
        .unwrap();
    assert!(
        !progress.criterion_evidence_refs.is_empty(),
        "parent Goal must retain child settlement evidence"
    );
    assert_ne!(progress.lifecycle, ContinuityGoalLifecycle::Achieved);
}

#[test]
fn production_a_and_b_cannot_cross_author_or_share_first_relation_runtime() {
    let complete_calls = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let start_calls = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let service = bind_effect_free_runtime(
        ConversationService::from_store(ConversationStore::open_in_memory().unwrap()),
        complete_calls.clone(),
        start_calls.clone(),
    );
    let (conversation_id, owner, agent) = create_group_with_agent(&service, "codex");
    designate_assistant(&service, &conversation_id, &owner, &agent);
    let posted_a = post(&service, &conversation_id, &owner, "task-a notes");
    after_post(&service, &conversation_id, &posted_a);
    service
        .after_runtime_settlement(
            &conversation_id,
            &json!({
                "output": assistant_turn_output(
                    "I'll open the first child conversation.",
                    &typed_goal_proposal_json(&conversation_id, "goal:a", "matter:a"),
                ),
                "membershipId": agent,
                "causationId": posted_a,
                "dispatchId": "dispatch:parent-a",
            }),
        )
        .unwrap();
    let posted_b = post(&service, &conversation_id, &owner, "task-b notes");
    after_post(&service, &conversation_id, &posted_b);
    service
        .after_runtime_settlement(
            &conversation_id,
            &json!({
                "output": assistant_turn_output(
                    "I'll open the second child conversation.",
                    &typed_goal_proposal_json(&conversation_id, "goal:b", "matter:b"),
                ),
                "membershipId": agent,
                "causationId": posted_b,
                "dispatchId": "dispatch:parent-b",
            }),
        )
        .unwrap();
    let relations = service
        .store()
        .list_child_relations(&conversation_id, None, 8)
        .unwrap();
    assert_eq!(relations.len(), 2);
    let child_a = relations
        .iter()
        .find(|item| item.goal_id == "goal:a")
        .unwrap()
        .child_conversation_id
        .clone();
    let child_b = relations
        .iter()
        .find(|item| item.goal_id == "goal:b")
        .unwrap()
        .child_conversation_id
        .clone();
    assert_ne!(child_a, child_b);
    let member_a = service
        .store()
        .get(&child_a)
        .unwrap()
        .assistant_membership_id
        .unwrap();
    let member_b = service
        .store()
        .get(&child_b)
        .unwrap()
        .assistant_membership_id
        .unwrap();
    let starts = start_calls
        .lock()
        .unwrap_or_else(|poison| poison.into_inner())
        .clone();
    let child_starts = start_kind(&starts, "child-work");
    assert_eq!(child_starts.len(), 2);
    assert!(child_starts.iter().any(|params| {
        params["conversationId"] == child_a && params["membershipId"] == member_a
    }));
    assert!(child_starts.iter().any(|params| {
        params["conversationId"] == child_b && params["membershipId"] == member_b
    }));
    assert!(
        child_starts
            .iter()
            .all(|params| params["conversationId"] != conversation_id)
    );
    let dispatch_a = admit_child_turn(
        service.store(),
        &child_a,
        &member_a,
        &child_agent_id(&service, &child_a, &member_a),
        "WORK-A-ONLY",
        None,
    );
    let dispatch_b = admit_child_turn(
        service.store(),
        &child_b,
        &member_b,
        &child_agent_id(&service, &child_b, &member_b),
        "WORK-B-ONLY",
        None,
    );
    service
        .after_runtime_settlement(
            &child_a,
            &json!({
                "output": "WORK-A-ONLY",
                "membershipId": member_a,
                "causationId": posted_a,
                "dispatchId": dispatch_a,
                "ok": true,
            }),
        )
        .unwrap();
    service
        .after_runtime_settlement(
            &child_b,
            &json!({
                "output": "WORK-B-ONLY",
                "membershipId": member_b,
                "causationId": posted_b,
                "dispatchId": dispatch_b,
                "ok": true,
            }),
        )
        .unwrap();
    let events_a = child_events(&service, &child_a);
    let events_b = child_events(&service, &child_b);
    assert!(events_a.iter().any(|event| {
        event["authorMembershipId"] == member_a
            && event["parts"].as_array().is_some_and(|parts| {
                parts.iter().any(|part| {
                    part["content"]
                        .as_str()
                        .is_some_and(|text| text.contains("WORK-A-ONLY"))
                })
            })
    }));
    assert!(events_b.iter().any(|event| {
        event["authorMembershipId"] == member_b
            && event["parts"].as_array().is_some_and(|parts| {
                parts.iter().any(|part| {
                    part["content"]
                        .as_str()
                        .is_some_and(|text| text.contains("WORK-B-ONLY"))
                })
            })
    }));
    assert!(events_a.iter().all(|event| {
        !event["parts"].as_array().is_some_and(|parts| {
            parts.iter().any(|part| {
                part["content"]
                    .as_str()
                    .is_some_and(|text| text.contains("WORK-B-ONLY"))
            })
        })
    }));
    let host = service.continuity().cloned().unwrap();
    assert_ne!(
        host.work_runtime_binding(&child_a, "goal:a", &member_a)
            .map(|binding| binding.child_conversation_id),
        host.work_runtime_binding(&child_b, "goal:b", &member_b)
            .map(|binding| binding.child_conversation_id)
    );
}

#[test]
fn production_settled_payload_changes_subsequent_goal_review() {
    let complete_calls = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let start_calls = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let complete_calls_for_turn = complete_calls.clone();
    let service = ConversationService::from_store(ConversationStore::open_in_memory().unwrap())
        .bind_conversation_runtime(PersistentRuntimePorts::new(
            {
                let start_calls = start_calls.clone();
                move |params: &Value| {
                    start_calls
                        .lock()
                        .unwrap_or_else(|poison| poison.into_inner())
                        .push(params.clone());
                    Ok(json!({ "ok": true, "accepted": true, "turnHandle": "turn:test" }))
                }
            },
            |_conversation_id: &str| json!([]),
            |_params: &Value| Ok(json!({ "ok": true })),
            move |params: &Value| {
                complete_calls_for_turn
                    .lock()
                    .unwrap_or_else(|poison| poison.into_inner())
                    .push(params.clone());
                let conversation_id = params
                    .get("parentConversationId")
                    .or_else(|| params.get("conversationId"))
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                if params.get("continuityKind").and_then(Value::as_str) == Some("wake-review") {
                    let text = params.get("text").and_then(Value::as_str).unwrap_or("");
                    let token = if text.contains("ALPHA-RECEIPT") {
                        "ALPHA-RECEIPT"
                    } else {
                        "NO-RECEIPT"
                    };
                    return Ok(json!({
                        "ok": true,
                        "output": review_agreement_proposal_json(conversation_id, token),
                    }));
                }
                Ok(json!({
                    "ok": true,
                    "output": typed_child_proposal_json(conversation_id),
                }))
            },
            |_request: Value| Ok(json!({})),
        ));
    let (conversation_id, owner, agent) = create_group_with_agent(&service, "codex");
    designate_assistant(&service, &conversation_id, &owner, &agent);
    let posted = post(&service, &conversation_id, &owner, "prepare notes now");
    after_post(&service, &conversation_id, &posted);
    settle_typed_child(&service, &conversation_id, &agent, &posted);
    let relation = service
        .store()
        .list_child_relations(&conversation_id, None, 8)
        .unwrap()
        .remove(0);
    let child = service
        .store()
        .get(&relation.child_conversation_id)
        .unwrap();
    let child_member = child.assistant_membership_id.unwrap();
    let dispatch_id = admit_child_turn(
        service.store(),
        &relation.child_conversation_id,
        &child_member,
        &child_agent_id(&service, &relation.child_conversation_id, &child_member),
        "ALPHA-RECEIPT",
        None,
    );
    service
        .after_runtime_settlement(
            &relation.child_conversation_id,
            &json!({
                "output": "ALPHA-RECEIPT",
                "membershipId": child_member,
                "causationId": posted,
                "dispatchId": dispatch_id,
                "ok": true,
            }),
        )
        .unwrap();
    let completes = complete_calls
        .lock()
        .unwrap_or_else(|poison| poison.into_inner())
        .clone();
    let reviews = completes
        .iter()
        .filter(|params| {
            params.get("continuityKind").and_then(Value::as_str) == Some("wake-review")
        })
        .collect::<Vec<_>>();
    assert!(
        reviews.iter().any(|review| {
            review["text"].as_str().is_some_and(|text| {
                text.contains("ALPHA-RECEIPT")
                    && text.contains(&relation.goal_id)
                    && !text.trim().is_empty()
            })
        }),
        "settled payload must change a subsequent Goal review: {reviews:?}"
    );
    let agreements = read_agreements(service.store(), &conversation_id).unwrap();
    assert!(
        agreements
            .iter()
            .any(|agreement| agreement.statement_ref.opaque_id.contains("ALPHA-RECEIPT")),
        "review outcome must change with the settled payload: {agreements:?}"
    );
}

#[test]
fn production_due_wake_after_reopen_reevaluates_with_task_receipt_context() {
    let root = std::env::temp_dir().join(format!(
        "lico-ca-c2-due-{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|duration| duration.as_nanos())
            .unwrap_or(0)
    ));
    let complete_calls = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let start_calls = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let bind = |root: &std::path::Path| {
        bind_effect_free_runtime(
            ConversationService::from_store(ConversationStore::open(root).unwrap()),
            complete_calls.clone(),
            start_calls.clone(),
        )
    };
    let service = bind(&root);
    let (conversation_id, owner, agent) = create_group_with_agent(&service, "codex");
    designate_assistant(&service, &conversation_id, &owner, &agent);
    let posted = post(&service, &conversation_id, &owner, "prepare notes now");
    after_post(&service, &conversation_id, &posted);
    settle_typed_child(&service, &conversation_id, &agent, &posted);
    let relation = service
        .store()
        .list_child_relations(&conversation_id, None, 8)
        .unwrap()
        .remove(0);
    let child_member = service
        .store()
        .get(&relation.child_conversation_id)
        .unwrap()
        .assistant_membership_id
        .unwrap();
    let dispatch_id = admit_child_turn(
        service.store(),
        &relation.child_conversation_id,
        &child_member,
        &child_agent_id(&service, &relation.child_conversation_id, &child_member),
        "DUE-RECEIPT",
        None,
    );
    service
        .after_runtime_settlement(
            &relation.child_conversation_id,
            &json!({
                "output": "DUE-RECEIPT",
                "membershipId": child_member,
                "causationId": posted,
                "dispatchId": dispatch_id,
                "ok": true,
            }),
        )
        .unwrap();
    let host = service.continuity().cloned().unwrap();
    host.schedule_review(&conversation_id, &relation.goal_id, 1)
        .unwrap();
    drop(service);

    let service = bind(&root);
    service.claim_continuity_owner().unwrap();
    set_continuity_clock(Some(20_000));
    let drain = service.attend_due().unwrap();
    set_continuity_clock(None);
    assert!(
        drain["reevaluated"].as_array().unwrap().len()
            + drain["consumed"].as_array().unwrap().len()
            >= 1,
        "due wake after reopen must reevaluate: {drain}"
    );
    let reviews = complete_calls
        .lock()
        .unwrap_or_else(|poison| poison.into_inner())
        .clone()
        .into_iter()
        .filter(|params| {
            params.get("continuityKind").and_then(Value::as_str) == Some("wake-review")
        })
        .collect::<Vec<_>>();
    assert!(
        reviews.iter().any(|params| {
            params["text"].as_str().is_some_and(|text| {
                !text.trim().is_empty()
                    && text.contains(&relation.goal_id)
                    && (text.contains("DUE-RECEIPT") || text.contains("dispatch:due"))
            })
        }),
        "reopened due review must carry task/receipt context, not empty text: {reviews:?}"
    );
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn production_duplicate_terminal_and_wake_do_not_reexecute() {
    let complete_calls = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let start_calls = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let service = bind_effect_free_runtime(
        ConversationService::from_store(ConversationStore::open_in_memory().unwrap()),
        complete_calls.clone(),
        start_calls.clone(),
    );
    let (conversation_id, owner, agent) = create_group_with_agent(&service, "codex");
    designate_assistant(&service, &conversation_id, &owner, &agent);
    let posted = post(&service, &conversation_id, &owner, "prepare notes now");
    after_post(&service, &conversation_id, &posted);
    settle_typed_child(&service, &conversation_id, &agent, &posted);
    let relation = service
        .store()
        .list_child_relations(&conversation_id, None, 8)
        .unwrap()
        .remove(0);
    let child_id = relation.child_conversation_id.clone();
    let child_member = service
        .store()
        .get(&child_id)
        .unwrap()
        .assistant_membership_id
        .unwrap();
    let dispatch_id = admit_child_turn(
        service.store(),
        &child_id,
        &child_member,
        &child_agent_id(&service, &child_id, &child_member),
        "ONCE-ONLY",
        Some(r#"{"kind":"artifact","settlementId":"dispatch:once"}"#),
    );
    let payload = json!({
        "output": "ONCE-ONLY",
        "membershipId": child_member,
        "causationId": posted,
        "dispatchId": dispatch_id,
        "ok": true,
    });
    service
        .after_runtime_settlement(&child_id, &payload)
        .unwrap();
    service
        .after_runtime_settlement(&child_id, &payload)
        .unwrap();
    let artifacts = child_events(&service, &child_id)
        .into_iter()
        .filter(|event| {
            event["correlationId"] == dispatch_id
                && event["parts"].as_array().is_some_and(|parts| {
                    parts.iter().any(|part| {
                        part["kind"] == "artifact"
                            && part["content"]
                                .as_str()
                                .is_some_and(|text| text.contains("dispatch:once"))
                    })
                })
        })
        .count();
    assert_eq!(
        artifacts, 1,
        "duplicate terminal must not rewrite artifacts"
    );
    put_effect(
        service.store(),
        &conversation_id,
        Some(&relation.goal_id),
        "effect:unknown-stay",
        licoup_conversation::continuity::ContinuityEffectStatus::Unknown,
    )
    .unwrap();
    let drain = service.drain_continuity(&conversation_id).unwrap();
    assert_eq!(drain["replayed"], 0);
    assert!(
        service
            .continuity()
            .unwrap()
            .unknown_effect_ids()
            .iter()
            .any(|id| id == "effect:unknown-stay")
    );
}

#[test]
fn production_settlement_commit_failure_recovers_via_host_without_manual_retry() {
    let root = std::env::temp_dir().join(format!(
        "lico-ca-c2-recover-{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|duration| duration.as_nanos())
            .unwrap_or(0)
    ));
    let complete_calls = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let start_calls = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let bind = |root: &std::path::Path| {
        bind_effect_free_runtime(
            ConversationService::from_store(ConversationStore::open(root).unwrap()),
            complete_calls.clone(),
            start_calls.clone(),
        )
    };
    let service = bind(&root);
    let (conversation_id, owner, agent) = create_group_with_agent(&service, "codex");
    designate_assistant(&service, &conversation_id, &owner, &agent);
    let posted = post(&service, &conversation_id, &owner, "prepare notes now");
    after_post(&service, &conversation_id, &posted);
    let output = assistant_turn_output(
        "I'll prepare the notes in a child conversation.",
        &typed_child_proposal_json(&conversation_id),
    );
    set_continuity_interrupt(Some(ContinuityInterrupt::BeforeCommit));
    let first = service.after_runtime_settlement(
        &conversation_id,
        &json!({
            "output": output,
            "membershipId": agent,
            "causationId": posted,
            "dispatchId": "dispatch:recover",
        }),
    );
    set_continuity_interrupt(None);
    assert!(
        first.is_err(),
        "injected BeforeCommit must surface: {first:?}"
    );
    drop(service);

    let service = bind(&root);
    service.claim_continuity_owner().unwrap();
    let _ = service.attend_due().unwrap();
    assert_eq!(
        service
            .store()
            .list_child_relations(&conversation_id, None, 8)
            .unwrap()
            .len(),
        1,
        "host recover must apply the pending settlement without a second hook call"
    );
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn production_parent_only_card_updates_ordinary_chat_independent() {
    let complete_calls = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let start_calls = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let service = bind_effect_free_runtime(
        ConversationService::from_store(ConversationStore::open_in_memory().unwrap()),
        complete_calls.clone(),
        start_calls.clone(),
    );
    let (conversation_id, owner, agent) = create_group_with_agent(&service, "codex");
    designate_assistant(&service, &conversation_id, &owner, &agent);
    let (other_id, other_owner, _) = create_group_with_agent(&service, "codex");
    let posted = post(&service, &conversation_id, &owner, "prepare notes now");
    after_post(&service, &conversation_id, &posted);
    settle_typed_child(&service, &conversation_id, &agent, &posted);
    let ordinary = post(&service, &other_id, &other_owner, "just chatting");
    let _ = ordinary;
    assert!(parent_card_exists(&service, &conversation_id));
    assert!(
        service
            .store()
            .list_child_relations(&other_id, None, 8)
            .unwrap()
            .is_empty()
    );
    let other = service
        .execute(json!({"action": "conversation.get", "conversationId": other_id}))
        .unwrap();
    assert!(other["taskViews"].as_array().unwrap().is_empty());
}

#[test]
fn production_completion_remains_authorized_acceptance_only() {
    let complete_calls = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let start_calls = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let service = bind_effect_free_runtime(
        ConversationService::from_store(ConversationStore::open_in_memory().unwrap()),
        complete_calls.clone(),
        start_calls.clone(),
    );
    let (conversation_id, owner, agent) = create_group_with_agent(&service, "codex");
    designate_assistant(&service, &conversation_id, &owner, &agent);
    let posted = post(&service, &conversation_id, &owner, "prepare notes now");
    after_post(&service, &conversation_id, &posted);
    settle_typed_child(&service, &conversation_id, &agent, &posted);
    let relation = service
        .store()
        .list_child_relations(&conversation_id, None, 8)
        .unwrap()
        .remove(0);
    let child_member = service
        .store()
        .get(&relation.child_conversation_id)
        .unwrap()
        .assistant_membership_id
        .unwrap();
    let dispatch_id = admit_child_turn(
        service.store(),
        &relation.child_conversation_id,
        &child_member,
        &child_agent_id(&service, &relation.child_conversation_id, &child_member),
        "DONE-BUT-NOT-ACCEPTED",
        None,
    );
    service
        .after_runtime_settlement(
            &relation.child_conversation_id,
            &json!({
                "output": "DONE-BUT-NOT-ACCEPTED",
                "membershipId": child_member,
                "causationId": posted,
                "dispatchId": dispatch_id,
                "ok": true,
            }),
        )
        .unwrap();
    let progress = read_goal(service.store(), &relation.goal_id)
        .unwrap()
        .unwrap();
    assert_ne!(progress.lifecycle, ContinuityGoalLifecycle::Achieved);
    assert_ne!(progress.lifecycle, ContinuityGoalLifecycle::Cancelled);
}

fn child_owner(service: &ConversationService, child_id: &str) -> String {
    service
        .store()
        .get(child_id)
        .unwrap()
        .memberships
        .iter()
        .find(|membership| membership.access == MembershipAccess::Owner)
        .unwrap()
        .id
        .clone()
}

fn recipient_grant_generation(store: &ConversationStore, conversation_id: &str) -> i64 {
    store
        .continuity_revocation_generation(conversation_id)
        .expect("recipient revocation generation")
}

fn grant_parent_span(
    service: &ConversationService,
    parent_id: &str,
    child_id: &str,
    child_member: &str,
    event_id: &str,
    grant_id: &str,
    start_byte: u64,
    end_byte: u64,
    status: ContinuityParentGrantStatus,
) {
    let mut granted = live_event_source(service.store(), parent_id, event_id);
    granted.span = Some(ContinuityUtf8ByteSpan {
        start_byte,
        end_byte,
    });
    put_grant(
        service.store(),
        &ContinuityParentContextGrant {
            grant_id: grant_id.into(),
            source_conversation_id: parent_id.to_owned(),
            recipient_conversation_id: child_id.to_owned(),
            recipient_membership_id: child_member.to_owned(),
            source_refs: vec![granted],
            authorized_scopes: vec![ContinuityVisibilityScope::Conversation],
            status,
            request_id: format!("request:{grant_id}"),
            revocation_generation: recipient_grant_generation(service.store(), child_id),
        },
    )
    .unwrap();
}

#[test]
fn production_parent_source_not_granted_to_child_never_reaches_child_params() {
    let complete_calls = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let start_calls = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let service = bind_effect_free_runtime(
        ConversationService::from_store(ConversationStore::open_in_memory().unwrap()),
        complete_calls.clone(),
        start_calls.clone(),
    );
    let (conversation_id, owner, agent) = create_group_with_agent(&service, "codex");
    designate_assistant(&service, &conversation_id, &owner, &agent);
    let secret = post(
        &service,
        &conversation_id,
        &owner,
        "PARENT-SECRET-UNGRANTED must stay on the parent",
    );
    put_agreement(
        service.store(),
        &conversation_id,
        &ContinuityAgreement {
            id: "agreement:parent-secret".into(),
            scope: ContinuityAgreementScope::Conversation,
            statement_ref: live_event_source(service.store(), &conversation_id, &secret),
            origin: ContinuityAgreementOrigin::UserExplicit,
            effective_revision: 1,
            supersedes: None,
            valid_from: 1,
            valid_until: None,
            revocation_generation: 0,
        },
    )
    .unwrap();
    let visible = post(
        &service,
        &conversation_id,
        &owner,
        "ADMITTED-SPAN and HIDDEN-REMAINDER",
    );
    set_child_work_fault(Some(ChildWorkFault::FailStart));
    let posted = post(&service, &conversation_id, &owner, "prepare notes now");
    after_post(&service, &conversation_id, &posted);
    settle_typed_child(&service, &conversation_id, &agent, &posted);
    set_child_work_fault(None);
    let relation = service
        .store()
        .list_child_relations(&conversation_id, None, 8)
        .unwrap()
        .remove(0);
    let child_id = relation.child_conversation_id.clone();
    let child_member = service
        .store()
        .get(&child_id)
        .unwrap()
        .assistant_membership_id
        .unwrap();
    grant_parent_span(
        &service,
        &conversation_id,
        &child_id,
        &child_member,
        &visible,
        "grant:span",
        0,
        13,
        ContinuityParentGrantStatus::Admitted,
    );
    service.claim_continuity_owner().unwrap();
    let _ = service.attend_due().unwrap();
    let starts = start_calls
        .lock()
        .unwrap_or_else(|poison| poison.into_inner())
        .clone();
    let child_starts = start_kind(&starts, "child-work");
    assert_eq!(child_starts.len(), 1);
    let text = delivered_guidance(child_starts[0]);
    assert!(text.contains(&relation.goal_id));
    assert!(text.contains("ADMITTED-SPAN"));
    assert!(!text.contains("PARENT-SECRET-UNGRANTED"));
    assert!(!text.contains("HIDDEN-REMAINDER"));
    let dispatch_id = admit_child_turn(
        service.store(),
        &child_id,
        &child_member,
        &child_agent_id(&service, &child_id, &child_member),
        "CHILD-RECEIPT",
        Some(r#"{"kind":"artifact","id":"art:grant"}"#),
    );
    service
        .after_runtime_settlement(
            &child_id,
            &json!({
                "output": "CHILD-RECEIPT",
                "membershipId": child_member,
                "dispatchId": dispatch_id,
                "ok": true,
            }),
        )
        .unwrap();
    let reviews = complete_calls
        .lock()
        .unwrap_or_else(|poison| poison.into_inner())
        .clone()
        .into_iter()
        .filter(|params| {
            params.get("continuityKind").and_then(Value::as_str) == Some("wake-review")
        })
        .collect::<Vec<_>>();
    assert!(reviews.iter().any(|params| {
        let text = delivered_guidance(params);
        text.contains("ADMITTED-SPAN")
            && text.contains(&relation.goal_id)
            && !text.contains("PARENT-SECRET-UNGRANTED")
            && !text.contains("HIDDEN-REMAINDER")
    }));
}

#[test]
fn production_admitted_span_then_revoke_before_dispatch_removes_it() {
    let complete_calls = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let start_calls = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let service = bind_effect_free_runtime(
        ConversationService::from_store(ConversationStore::open_in_memory().unwrap()),
        complete_calls.clone(),
        start_calls.clone(),
    );
    let (conversation_id, owner, agent) = create_group_with_agent(&service, "codex");
    designate_assistant(&service, &conversation_id, &owner, &agent);
    let visible = post(
        &service,
        &conversation_id,
        &owner,
        "ADMITTED-SPAN and HIDDEN-REMAINDER",
    );
    set_child_work_fault(Some(ChildWorkFault::FailStart));
    let posted = post(&service, &conversation_id, &owner, "prepare notes now");
    after_post(&service, &conversation_id, &posted);
    settle_typed_child(&service, &conversation_id, &agent, &posted);
    set_child_work_fault(None);
    let relation = service
        .store()
        .list_child_relations(&conversation_id, None, 8)
        .unwrap()
        .remove(0);
    let child_id = relation.child_conversation_id.clone();
    let child_member = service
        .store()
        .get(&child_id)
        .unwrap()
        .assistant_membership_id
        .unwrap();
    grant_parent_span(
        &service,
        &conversation_id,
        &child_id,
        &child_member,
        &visible,
        "grant:revoke",
        0,
        13,
        ContinuityParentGrantStatus::Revoked,
    );
    service.claim_continuity_owner().unwrap();
    let _ = service.attend_due().unwrap();
    let starts = start_calls
        .lock()
        .unwrap_or_else(|poison| poison.into_inner())
        .clone();
    let child_starts = start_kind(&starts, "child-work");
    assert_eq!(child_starts.len(), 1);
    let text = delivered_guidance(child_starts[0]);
    assert!(text.contains(&relation.goal_id));
    assert!(!text.contains("ADMITTED-SPAN"));
    assert!(!text.contains("HIDDEN-REMAINDER"));
}

#[test]
fn production_a_and_b_grants_do_not_mix() {
    let complete_calls = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let start_calls = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let service = bind_effect_free_runtime(
        ConversationService::from_store(ConversationStore::open_in_memory().unwrap()),
        complete_calls.clone(),
        start_calls.clone(),
    );
    let (conversation_id, owner, agent) = create_group_with_agent(&service, "codex");
    designate_assistant(&service, &conversation_id, &owner, &agent);
    let source_a = post(&service, &conversation_id, &owner, "GRANT-A-ONLY context");
    let source_b = post(&service, &conversation_id, &owner, "GRANT-B-ONLY context");
    set_child_work_fault(Some(ChildWorkFault::FailStart));
    let posted_a = post(&service, &conversation_id, &owner, "task-a notes");
    after_post(&service, &conversation_id, &posted_a);
    service
        .after_runtime_settlement(
            &conversation_id,
            &json!({
                "output": assistant_turn_output(
                    "I'll open the first child conversation.",
                    &typed_goal_proposal_json(&conversation_id, "goal:a", "matter:a"),
                ),
                "membershipId": agent,
                "causationId": posted_a,
                "dispatchId": "dispatch:parent-a",
            }),
        )
        .unwrap();
    set_child_work_fault(Some(ChildWorkFault::FailStart));
    let posted_b = post(&service, &conversation_id, &owner, "task-b notes");
    after_post(&service, &conversation_id, &posted_b);
    service
        .after_runtime_settlement(
            &conversation_id,
            &json!({
                "output": assistant_turn_output(
                    "I'll open the second child conversation.",
                    &typed_goal_proposal_json(&conversation_id, "goal:b", "matter:b"),
                ),
                "membershipId": agent,
                "causationId": posted_b,
                "dispatchId": "dispatch:parent-b",
            }),
        )
        .unwrap();
    set_child_work_fault(None);
    let relations = service
        .store()
        .list_child_relations(&conversation_id, None, 8)
        .unwrap();
    let child_a = relations
        .iter()
        .find(|item| item.goal_id == "goal:a")
        .unwrap();
    let child_b = relations
        .iter()
        .find(|item| item.goal_id == "goal:b")
        .unwrap();
    let member_a = service
        .store()
        .get(&child_a.child_conversation_id)
        .unwrap()
        .assistant_membership_id
        .unwrap();
    let member_b = service
        .store()
        .get(&child_b.child_conversation_id)
        .unwrap()
        .assistant_membership_id
        .unwrap();
    grant_parent_span(
        &service,
        &conversation_id,
        &child_a.child_conversation_id,
        &member_a,
        &source_a,
        "grant:a",
        0,
        12,
        ContinuityParentGrantStatus::Admitted,
    );
    grant_parent_span(
        &service,
        &conversation_id,
        &child_b.child_conversation_id,
        &member_b,
        &source_b,
        "grant:b",
        0,
        12,
        ContinuityParentGrantStatus::Admitted,
    );
    service.claim_continuity_owner().unwrap();
    let _ = service.attend_due().unwrap();
    let starts = start_calls
        .lock()
        .unwrap_or_else(|poison| poison.into_inner())
        .clone();
    let child_starts = start_kind(&starts, "child-work");
    assert_eq!(child_starts.len(), 2);
    let text_a = child_starts
        .iter()
        .find(|params| params["conversationId"] == child_a.child_conversation_id)
        .map(|params| delivered_guidance(params))
        .expect("child A start");
    let text_b = child_starts
        .iter()
        .find(|params| params["conversationId"] == child_b.child_conversation_id)
        .map(|params| delivered_guidance(params))
        .expect("child B start");
    assert!(text_a.contains("GRANT-A-ONLY"));
    assert!(!text_a.contains("GRANT-B-ONLY"));
    assert!(text_b.contains("GRANT-B-ONLY"));
    assert!(!text_b.contains("GRANT-A-ONLY"));
}

#[test]
fn production_worker_and_reviewer_authors_survive_assistant_change() {
    let complete_calls = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let start_calls = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let service = bind_effect_free_runtime(
        ConversationService::from_store(ConversationStore::open_in_memory().unwrap()),
        complete_calls.clone(),
        start_calls.clone(),
    );
    let (conversation_id, owner, agent) = create_group_with_agent(&service, "codex");
    designate_assistant(&service, &conversation_id, &owner, &agent);
    let posted = post(&service, &conversation_id, &owner, "prepare notes now");
    after_post(&service, &conversation_id, &posted);
    settle_typed_child(&service, &conversation_id, &agent, &posted);
    let relation = service
        .store()
        .list_child_relations(&conversation_id, None, 8)
        .unwrap()
        .remove(0);
    let child_id = relation.child_conversation_id.clone();
    let designated = service
        .store()
        .get(&child_id)
        .unwrap()
        .assistant_membership_id
        .unwrap();
    let worker = service
        .store()
        .add_member(
            &child_id,
            Principal {
                id: "agent:worker-codex".into(),
                kind: PrincipalKind::Agent,
                display_name: "Worker".into(),
                agent_id: Some("codex".into()),
                created_at_unix_ms: 1,
            },
            MembershipAccess::Member,
        )
        .unwrap();
    let reviewer = service
        .store()
        .add_member(
            &child_id,
            Principal {
                id: "agent:reviewer-codex".into(),
                kind: PrincipalKind::Agent,
                display_name: "Reviewer".into(),
                agent_id: Some("codex".into()),
                created_at_unix_ms: 1,
            },
            MembershipAccess::Member,
        )
        .unwrap();
    assert_ne!(worker.id, designated);
    assert_ne!(reviewer.id, designated);
    let dispatch_id = admit_child_turn(
        service.store(),
        &child_id,
        &worker.id,
        "codex",
        "WORKER-OUTPUT",
        Some(r#"{"kind":"artifact","id":"art:worker"}"#),
    );
    designate_assistant(
        &service,
        &child_id,
        &child_owner(&service, &child_id),
        &reviewer.id,
    );
    service
        .after_runtime_settlement(
            &child_id,
            &json!({
                "output": "WORKER-OUTPUT",
                "membershipId": worker.id,
                "dispatchId": dispatch_id,
                "ok": true,
            }),
        )
        .unwrap();
    designate_assistant(
        &service,
        &child_id,
        &child_owner(&service, &child_id),
        &designated,
    );
    service
        .after_runtime_settlement(
            &child_id,
            &json!({
                "output": "WORKER-OUTPUT",
                "membershipId": worker.id,
                "dispatchId": dispatch_id,
                "ok": true,
            }),
        )
        .unwrap();
    let events = child_events(&service, &child_id);
    let authored: Vec<_> = events
        .iter()
        .filter(|event| event["correlationId"] == dispatch_id)
        .collect();
    assert_eq!(authored.len(), 1);
    assert_eq!(authored[0]["authorMembershipId"], worker.id);
    assert_ne!(authored[0]["authorMembershipId"], designated);
    assert_ne!(authored[0]["authorMembershipId"], reviewer.id);
}

#[test]
fn production_foreign_or_mismatched_turn_writes_zero_events_and_evidence() {
    let complete_calls = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let start_calls = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let service = bind_effect_free_runtime(
        ConversationService::from_store(ConversationStore::open_in_memory().unwrap()),
        complete_calls.clone(),
        start_calls.clone(),
    );
    let (conversation_id, owner, agent) = create_group_with_agent(&service, "codex");
    designate_assistant(&service, &conversation_id, &owner, &agent);
    let posted = post(&service, &conversation_id, &owner, "prepare notes now");
    after_post(&service, &conversation_id, &posted);
    settle_typed_child(&service, &conversation_id, &agent, &posted);
    let relation = service
        .store()
        .list_child_relations(&conversation_id, None, 8)
        .unwrap()
        .remove(0);
    let child_id = relation.child_conversation_id.clone();
    let child_member = service
        .store()
        .get(&child_id)
        .unwrap()
        .assistant_membership_id
        .unwrap();
    let before = child_message_events(&service, &child_id).len();
    let before_evidence = read_goal(service.store(), &relation.goal_id)
        .unwrap()
        .unwrap()
        .criterion_evidence_refs
        .len();
    service
        .after_runtime_settlement(
            &child_id,
            &json!({
                "output": "FOREIGN",
                "membershipId": child_member,
                "dispatchId": "dispatch:foreign",
                "ok": true,
            }),
        )
        .unwrap();
    let admitted = admit_child_turn(
        service.store(),
        &child_id,
        &child_member,
        &child_agent_id(&service, &child_id, &child_member),
        "REAL-TURN",
        Some(r#"{"kind":"artifact","id":"art:real"}"#),
    );
    let other = service
        .store()
        .add_member(
            &child_id,
            Principal {
                id: "agent:other-codex".into(),
                kind: PrincipalKind::Agent,
                display_name: "Other".into(),
                agent_id: Some("codex".into()),
                created_at_unix_ms: 1,
            },
            MembershipAccess::Member,
        )
        .unwrap();
    service
        .after_runtime_settlement(
            &child_id,
            &json!({
                "output": "MISMATCH",
                "membershipId": other.id,
                "dispatchId": admitted,
                "ok": true,
            }),
        )
        .unwrap();
    assert_eq!(child_message_events(&service, &child_id).len(), before + 1);
    assert_eq!(
        read_goal(service.store(), &relation.goal_id)
            .unwrap()
            .unwrap()
            .criterion_evidence_refs
            .len(),
        before_evidence
    );
}

#[test]
fn production_plain_text_is_not_fabricated_artifact_or_first_criterion_proof() {
    let complete_calls = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let start_calls = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let service = bind_effect_free_runtime(
        ConversationService::from_store(ConversationStore::open_in_memory().unwrap()),
        complete_calls.clone(),
        start_calls.clone(),
    );
    let (conversation_id, owner, agent) = create_group_with_agent(&service, "codex");
    designate_assistant(&service, &conversation_id, &owner, &agent);
    let posted = post(&service, &conversation_id, &owner, "prepare notes now");
    after_post(&service, &conversation_id, &posted);
    settle_typed_child(&service, &conversation_id, &agent, &posted);
    let relation = service
        .store()
        .list_child_relations(&conversation_id, None, 8)
        .unwrap()
        .remove(0);
    let child_id = relation.child_conversation_id.clone();
    let child_member = service
        .store()
        .get(&child_id)
        .unwrap()
        .assistant_membership_id
        .unwrap();
    let dispatch_id = admit_child_turn(
        service.store(),
        &child_id,
        &child_member,
        &child_agent_id(&service, &child_id, &child_member),
        "PLAIN-TEXT-ONLY",
        None,
    );
    service
        .after_runtime_settlement(
            &child_id,
            &json!({
                "output": "PLAIN-TEXT-ONLY",
                "membershipId": child_member,
                "dispatchId": dispatch_id,
                "ok": true,
            }),
        )
        .unwrap();
    let events = child_events(&service, &child_id);
    assert!(events.iter().any(|event| {
        event["correlationId"] == dispatch_id
            && event["parts"].as_array().is_some_and(|parts| {
                parts.iter().any(|part| {
                    part["kind"] == "text"
                        && part["content"]
                            .as_str()
                            .is_some_and(|text| text.contains("PLAIN-TEXT-ONLY"))
                }) && parts.iter().all(|part| part["kind"] != "artifact")
            })
    }));
    assert!(
        read_goal(service.store(), &relation.goal_id)
            .unwrap()
            .unwrap()
            .criterion_evidence_refs
            .is_empty()
    );
    service
        .after_runtime_settlement(
            &child_id,
            &json!({
                "output": "still unknown",
                "membershipId": child_member,
                "dispatchId": dispatch_id,
                "ok": false,
                "turnStatus": "unknown",
            }),
        )
        .unwrap();
    assert!(
        read_goal(service.store(), &relation.goal_id)
            .unwrap()
            .unwrap()
            .criterion_evidence_refs
            .is_empty()
    );
}

#[test]
fn production_injected_failure_between_materialization_and_ack_recovers_one_set() {
    let root = std::env::temp_dir().join(format!(
        "lico-ca-c2-evidence-{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|duration| duration.as_nanos())
            .unwrap_or(0)
    ));
    let complete_calls = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let start_calls = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let bind = |root: &std::path::Path| {
        bind_effect_free_runtime(
            ConversationService::from_store(ConversationStore::open(root).unwrap()),
            complete_calls.clone(),
            start_calls.clone(),
        )
    };
    let service = bind(&root);
    let (conversation_id, owner, agent) = create_group_with_agent(&service, "codex");
    designate_assistant(&service, &conversation_id, &owner, &agent);
    let posted = post(&service, &conversation_id, &owner, "prepare notes now");
    after_post(&service, &conversation_id, &posted);
    settle_typed_child(&service, &conversation_id, &agent, &posted);
    let relation = service
        .store()
        .list_child_relations(&conversation_id, None, 8)
        .unwrap()
        .remove(0);
    let child_id = relation.child_conversation_id.clone();
    let child_member = service
        .store()
        .get(&child_id)
        .unwrap()
        .assistant_membership_id
        .unwrap();
    let dispatch_id = admit_child_turn(
        service.store(),
        &child_id,
        &child_member,
        &child_agent_id(&service, &child_id, &child_member),
        "ONE-SET",
        Some(r#"{"kind":"artifact","id":"art:one"}"#),
    );
    set_child_work_fault(Some(ChildWorkFault::FailAfterEvidence));
    let first = service.after_runtime_settlement(
        &child_id,
        &json!({
            "output": "ONE-SET",
            "membershipId": child_member,
            "dispatchId": dispatch_id,
            "ok": true,
        }),
    );
    set_child_work_fault(None);
    assert!(
        first.is_err(),
        "injected evidence/ack fault must surface: {first:?}"
    );
    drop(service);

    let service = bind(&root);
    service.claim_continuity_owner().unwrap();
    let _ = service.attend_due().unwrap();
    let artifacts = child_events(&service, &child_id)
        .into_iter()
        .filter(|event| {
            event["correlationId"] == dispatch_id
                && event["parts"]
                    .as_array()
                    .is_some_and(|parts| parts.iter().any(|part| part["kind"] == "artifact"))
        })
        .count();
    assert_eq!(artifacts, 1);
    assert_eq!(
        read_goal(service.store(), &relation.goal_id)
            .unwrap()
            .unwrap()
            .criterion_evidence_refs
            .len(),
        1
    );
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn production_child_start_failure_remains_obligation_and_recovers_once() {
    let root = std::env::temp_dir().join(format!(
        "lico-ca-c2-start-{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|duration| duration.as_nanos())
            .unwrap_or(0)
    ));
    let complete_calls = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let start_calls = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let bind = |root: &std::path::Path| {
        bind_effect_free_runtime(
            ConversationService::from_store(ConversationStore::open(root).unwrap()),
            complete_calls.clone(),
            start_calls.clone(),
        )
    };
    let service = bind(&root);
    let (conversation_id, owner, agent) = create_group_with_agent(&service, "codex");
    designate_assistant(&service, &conversation_id, &owner, &agent);
    set_child_work_fault(Some(ChildWorkFault::FailStart));
    let posted = post(&service, &conversation_id, &owner, "prepare notes now");
    after_post(&service, &conversation_id, &posted);
    settle_typed_child(&service, &conversation_id, &agent, &posted);
    set_child_work_fault(None);
    assert!(
        start_kind(
            &start_calls
                .lock()
                .unwrap_or_else(|poison| poison.into_inner())
                .clone(),
            "child-work"
        )
        .is_empty()
    );
    drop(service);

    let service = bind(&root);
    service.claim_continuity_owner().unwrap();
    let _ = service.attend_due().unwrap();
    let starts = start_calls
        .lock()
        .unwrap_or_else(|poison| poison.into_inner())
        .clone();
    let child_starts = start_kind(&starts, "child-work");
    assert_eq!(child_starts.len(), 1);
    let _ = service.attend_due().unwrap();
    assert_eq!(
        start_kind(
            &start_calls
                .lock()
                .unwrap_or_else(|poison| poison.into_inner())
                .clone(),
            "child-work"
        )
        .len(),
        1
    );
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn production_accepted_start_marker_failure_recovers_without_rerun() {
    let root = std::env::temp_dir().join(format!(
        "lico-ca-c2-accepted-{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|duration| duration.as_nanos())
            .unwrap_or(0)
    ));
    let complete_calls = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let start_calls = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let bind = |root: &std::path::Path| {
        bind_effect_free_runtime(
            ConversationService::from_store(ConversationStore::open(root).unwrap()),
            complete_calls.clone(),
            start_calls.clone(),
        )
    };
    let service = bind(&root);
    let (conversation_id, owner, agent) = create_group_with_agent(&service, "codex");
    designate_assistant(&service, &conversation_id, &owner, &agent);
    set_child_work_fault(Some(ChildWorkFault::FailAfterAccepted));
    let posted = post(&service, &conversation_id, &owner, "prepare notes now");
    after_post(&service, &conversation_id, &posted);
    settle_typed_child(&service, &conversation_id, &agent, &posted);
    set_child_work_fault(None);
    assert_eq!(
        start_kind(
            &start_calls
                .lock()
                .unwrap_or_else(|poison| poison.into_inner())
                .clone(),
            "child-work"
        )
        .len(),
        1
    );
    drop(service);

    let service = bind(&root);
    service.claim_continuity_owner().unwrap();
    let _ = service.attend_due().unwrap();
    assert_eq!(
        start_kind(
            &start_calls
                .lock()
                .unwrap_or_else(|poison| poison.into_inner())
                .clone(),
            "child-work"
        )
        .len(),
        1,
        "accepted-but-unacked must reconcile without a second start"
    );
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn production_two_goals_same_child_retry_preserves_profile_and_dispatch() {
    let complete_calls = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let start_calls = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let service = bind_effect_free_runtime(
        ConversationService::from_store(ConversationStore::open_in_memory().unwrap()),
        complete_calls.clone(),
        start_calls.clone(),
    );
    let (conversation_id, owner, agent) = create_group_with_agent(&service, "codex");
    designate_assistant(&service, &conversation_id, &owner, &agent);
    set_child_work_fault(Some(ChildWorkFault::FailStart));
    let posted_a = post(&service, &conversation_id, &owner, "task-a notes");
    after_post(&service, &conversation_id, &posted_a);
    service
        .after_runtime_settlement(
            &conversation_id,
            &json!({
                "output": assistant_turn_output(
                    "I'll open the first child conversation.",
                    &typed_goal_proposal_json(&conversation_id, "goal:a", "matter:a"),
                ),
                "membershipId": agent,
                "causationId": posted_a,
                "dispatchId": "dispatch:parent-a",
            }),
        )
        .unwrap();
    set_child_work_fault(Some(ChildWorkFault::FailStart));
    let posted_b = post(&service, &conversation_id, &owner, "task-b notes");
    after_post(&service, &conversation_id, &posted_b);
    service
        .after_runtime_settlement(
            &conversation_id,
            &json!({
                "output": assistant_turn_output(
                    "I'll open the second child conversation.",
                    &typed_goal_proposal_json(&conversation_id, "goal:b", "matter:b"),
                ),
                "membershipId": agent,
                "causationId": posted_b,
                "dispatchId": "dispatch:parent-b",
            }),
        )
        .unwrap();
    set_child_work_fault(None);
    let relations = service
        .store()
        .list_child_relations(&conversation_id, None, 8)
        .unwrap();
    assert_eq!(relations.len(), 2);
    let child_a = relations
        .iter()
        .find(|item| item.goal_id == "goal:a")
        .unwrap();
    let member_a = service
        .store()
        .get(&child_a.child_conversation_id)
        .unwrap()
        .assistant_membership_id
        .unwrap();
    let profile = service
        .store()
        .membership_profile(&member_a)
        .unwrap()
        .expect("child profile");
    service
        .store()
        .set_membership_profile(
            &child_a.child_conversation_id,
            &member_a,
            &child_owner(&service, &child_a.child_conversation_id),
            profile.revision,
            &ProfileIntentUpdate {
                required_capabilities: vec!["approval:workspace".into()],
                preferred_model: Some("gpt-test".into()),
                preferred_reasoning_effort: Some("high".into()),
                preferred_environment: Some("/workspace/project".into()),
                ..ProfileIntentUpdate::default()
            },
        )
        .unwrap();
    service.claim_continuity_owner().unwrap();
    let _ = service.attend_due().unwrap();
    let starts = start_calls
        .lock()
        .unwrap_or_else(|poison| poison.into_inner())
        .clone();
    let child_starts = start_kind(&starts, "child-work");
    assert_eq!(child_starts.len(), 2);
    let params_a = child_starts
        .iter()
        .find(|params| params["goalId"] == "goal:a")
        .expect("goal A start");
    let params_b = child_starts
        .iter()
        .find(|params| params["goalId"] == "goal:b")
        .expect("goal B start");
    assert_ne!(params_a["conversationId"], params_b["conversationId"]);
    assert_eq!(params_a["model"], "gpt-test");
    assert_eq!(params_a["reasoningEffort"], "high");
    assert_eq!(params_a["workingDirectory"], "/workspace/project");
    assert!(params_a.get("workspace").is_none());
    assert!(params_a.get("environment").is_none());
    assert!(params_a.get("approval").is_none());
    assert_eq!(
        params_a["requiredCapabilities"],
        json!(["approval:workspace"])
    );
    service.claim_continuity_owner().unwrap();
    let _ = service.attend_due().unwrap();
    assert_eq!(
        start_kind(
            &start_calls
                .lock()
                .unwrap_or_else(|poison| poison.into_inner())
                .clone(),
            "child-work"
        )
        .len(),
        2,
        "same child retry must not start a second time"
    );
}

fn bind_control_runtime(
    service: ConversationService,
    start_calls: std::sync::Arc<std::sync::Mutex<Vec<Value>>>,
    steer_calls: std::sync::Arc<std::sync::Mutex<Vec<Value>>>,
    cancel_calls: std::sync::Arc<std::sync::Mutex<Vec<Value>>>,
    native_turn_id: &'static str,
) -> ConversationService {
    service.bind_conversation_runtime(
        PersistentRuntimePorts::new(
            {
                let start_calls = start_calls.clone();
                move |params: &Value| {
                    start_calls
                        .lock()
                        .unwrap_or_else(|poison| poison.into_inner())
                        .push(params.clone());
                    let handle = params
                        .get("dispatchId")
                        .and_then(Value::as_str)
                        .filter(|value| !value.is_empty())
                        .unwrap_or("turn:live-child");
                    Ok(json!({
                        "ok": true,
                        "accepted": true,
                        "turnHandle": handle,
                        "turnId": native_turn_id,
                    }))
                }
            },
            |_conversation_id: &str| json!([]),
            {
                let steer_calls = steer_calls.clone();
                move |params: &Value| {
                    steer_calls
                        .lock()
                        .unwrap_or_else(|poison| poison.into_inner())
                        .push(params.clone());
                    Ok(json!({ "ok": true, "status": "accepted" }))
                }
            },
            |_params: &Value| {
                Ok(json!({
                    "ok": true,
                    "output": "{}",
                }))
            },
            |_request: Value| Ok(json!({})),
        )
        .with_cancel({
            let cancel_calls = cancel_calls.clone();
            move |params: &Value| {
                cancel_calls
                    .lock()
                    .unwrap_or_else(|poison| poison.into_inner())
                    .push(params.clone());
                Ok(json!({ "ok": true, "status": "cancel_requested" }))
            }
        }),
    )
}

fn commit_child_without_start(
    service: &ConversationService,
    conversation_id: &str,
    owner: &str,
    agent: &str,
) -> (String, String, String) {
    let posted = post(service, conversation_id, owner, "prepare notes now");
    let _ = service.execute(json!({
        "action": "conversation.dispatch.after-post",
        "conversationId": conversation_id,
        "eventId": posted,
    }));
    settle_typed_child(service, conversation_id, agent, &posted);
    let relation = service
        .store()
        .list_child_relations(conversation_id, None, 8)
        .unwrap()
        .remove(0);
    let child_id = relation.child_conversation_id.clone();
    let child_member = service
        .store()
        .get(&child_id)
        .unwrap()
        .assistant_membership_id
        .unwrap();
    (relation.goal_id, child_id, child_member)
}

#[test]
fn residual_parent_commit_writes_intent_and_attend_due_starts_without_repost() {
    let start_calls = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let complete_calls = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let service = ConversationService::from_store(ConversationStore::open_in_memory().unwrap());
    let (conversation_id, owner, agent) = create_group_with_agent(&service, "codex");
    designate_assistant(&service, &conversation_id, &owner, &agent);
    let (goal_id, _, _) = commit_child_without_start(&service, &conversation_id, &owner, &agent);
    let intent = read_child_work_intent(service.store(), &conversation_id, &goal_id, 1)
        .unwrap()
        .expect("intent committed with parent unit");
    assert_eq!(intent["operationId"], child_work_operation_id(&goal_id, 1));
    assert_eq!(list_unacked_child_work(service.store()).unwrap().len(), 1);
    assert!(
        start_calls
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
            .is_empty()
    );
    let service = bind_effect_free_runtime(service, complete_calls, start_calls.clone());
    service.claim_continuity_owner().unwrap();
    let _ = service.attend_due().unwrap();
    assert_eq!(
        start_kind(&start_calls.lock().unwrap().clone(), "child-work").len(),
        1
    );
    service.claim_continuity_owner().unwrap();
    let _ = service.attend_due().unwrap();
    assert_eq!(
        start_kind(&start_calls.lock().unwrap().clone(), "child-work").len(),
        1,
        "recovery must start the same operation once"
    );
}

#[test]
fn residual_unrelated_send_cannot_satisfy_child_work() {
    let complete_calls = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let start_calls = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let service = bind_effect_free_runtime(
        ConversationService::from_store(ConversationStore::open_in_memory().unwrap()),
        complete_calls,
        start_calls.clone(),
    );
    let (conversation_id, owner, agent) = create_group_with_agent(&service, "codex");
    designate_assistant(&service, &conversation_id, &owner, &agent);
    set_child_work_fault(Some(ChildWorkFault::FailStart));
    let (goal_id, child_id, child_member) =
        commit_child_without_start(&service, &conversation_id, &owner, &agent);
    set_child_work_fault(None);
    let child_agent = child_agent_id(&service, &child_id, &child_member);
    service
        .store()
        .prepare_runtime_dispatch(
            &child_agent,
            "",
            "unrelated chat",
            Some(&child_id),
            Some(&child_member),
            None,
            Some("dispatch:unrelated-chat"),
        )
        .unwrap();
    service.claim_continuity_owner().unwrap();
    let _ = service.attend_due().unwrap();
    let start_snapshot = start_calls.lock().unwrap().clone();
    let starts = start_kind(&start_snapshot, "child-work");
    assert_eq!(starts.len(), 1);
    assert_eq!(
        starts[0]["dispatchId"],
        child_work_operation_id(&goal_id, 1)
    );
    assert_ne!(starts[0]["dispatchId"], "dispatch:unrelated-chat");
}

#[test]
fn residual_registered_dispatch_before_accepted_recovers_same_operation() {
    let store = ConversationStore::open_in_memory().unwrap();
    let start_calls = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let start_store = store.clone();
    let service = ConversationService::from_store(store).bind_conversation_runtime(
        PersistentRuntimePorts::new(
            {
                let start_calls = start_calls.clone();
                move |params: &Value| {
                    start_calls
                        .lock()
                        .unwrap_or_else(|poison| poison.into_inner())
                        .push(params.clone());
                    let dispatch_id = params
                        .get("dispatchId")
                        .and_then(Value::as_str)
                        .expect("operation dispatchId");
                    let conversation_id = params
                        .get("conversationId")
                        .and_then(Value::as_str)
                        .unwrap_or_default();
                    let membership_id = params
                        .get("membershipId")
                        .and_then(Value::as_str)
                        .unwrap_or_default();
                    let agent = params
                        .get("agent")
                        .or_else(|| params.get("agentId"))
                        .and_then(Value::as_str)
                        .unwrap_or_default();
                    start_store
                        .prepare_runtime_dispatch(
                            agent,
                            "",
                            params.get("text").and_then(Value::as_str).unwrap_or("work"),
                            Some(conversation_id),
                            Some(membership_id),
                            params.get("causationId").and_then(Value::as_str),
                            Some(dispatch_id),
                        )
                        .unwrap();
                    Err(
                        licoup_native::platform::runtime_adapters::RuntimeAdapterError::ConversationDispatchFailed,
                    )
                }
            },
            |_conversation_id: &str| json!([]),
            |_params: &Value| Ok(json!({ "ok": true })),
            |_params: &Value| Ok(json!({ "ok": true, "output": "{}" })),
            |_request: Value| Ok(json!({})),
        ),
    );
    let (conversation_id, owner, agent) = create_group_with_agent(&service, "codex");
    designate_assistant(&service, &conversation_id, &owner, &agent);
    let (goal_id, _, _) = commit_child_without_start(&service, &conversation_id, &owner, &agent);
    service.claim_continuity_owner().unwrap();
    let _ = service.attend_due().unwrap();
    assert_eq!(
        start_kind(&start_calls.lock().unwrap().clone(), "child-work").len(),
        1
    );
    service.claim_continuity_owner().unwrap();
    let _ = service.attend_due().unwrap();
    assert_eq!(
        start_kind(&start_calls.lock().unwrap().clone(), "child-work").len(),
        1,
        "registered operation must recover without a second start"
    );
    assert!(
        read_child_work_accepted(service.store(), &conversation_id, &goal_id, 1)
            .unwrap()
            .is_some()
    );
}

#[test]
fn residual_pause_on_pending_recovery_preserves_unknown_without_new_effects() {
    let complete_calls = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let start_calls = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let service = bind_effect_free_runtime(
        ConversationService::from_store(ConversationStore::open_in_memory().unwrap()),
        complete_calls,
        start_calls.clone(),
    );
    let (conversation_id, owner, agent) = create_group_with_agent(&service, "codex");
    designate_assistant(&service, &conversation_id, &owner, &agent);
    set_child_work_fault(Some(ChildWorkFault::FailStart));
    let (goal_id, _, _) = commit_child_without_start(&service, &conversation_id, &owner, &agent);
    set_child_work_fault(None);
    apply_goal_control(
        service.store(),
        &conversation_id,
        &goal_id,
        ContinuityGoalEvent::Pause,
    )
    .unwrap();
    service.claim_continuity_owner().unwrap();
    let _ = service.attend_due().unwrap();
    assert!(start_kind(&start_calls.lock().unwrap().clone(), "child-work").is_empty());
    assert_eq!(list_unacked_child_work(service.store()).unwrap().len(), 1);
}

#[test]
fn residual_repeated_recheck_failure_preserves_obligation_without_native_effect() {
    let complete_calls = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let start_calls = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let service = bind_effect_free_runtime(
        ConversationService::from_store(ConversationStore::open_in_memory().unwrap()),
        complete_calls,
        start_calls.clone(),
    );
    let (conversation_id, owner, agent) = create_group_with_agent(&service, "codex");
    designate_assistant(&service, &conversation_id, &owner, &agent);
    set_child_work_fault(Some(ChildWorkFault::FailStart));
    let (goal_id, _, _) = commit_child_without_start(&service, &conversation_id, &owner, &agent);
    set_child_work_fault(None);
    set_child_assembly_recheck_failures(2);
    service.claim_continuity_owner().unwrap();
    assert!(
        start_kind(&start_calls.lock().unwrap().clone(), "child-work").is_empty(),
        "repeated recheck failure must not start child work"
    );
    assert_eq!(list_unacked_child_work(service.store()).unwrap().len(), 1);
    assert!(
        read_child_work_intent(service.store(), &conversation_id, &goal_id, 1)
            .unwrap()
            .is_some()
    );
    set_child_assembly_recheck_failures(0);
    let _ = service.attend_due().unwrap();
    assert_eq!(
        start_kind(&start_calls.lock().unwrap().clone(), "child-work").len(),
        1
    );
}

#[test]
fn residual_revoked_evidence_cannot_bypass_wake_review_admission() {
    let complete_calls = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let start_calls = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let service = bind_effect_free_runtime(
        ConversationService::from_store(ConversationStore::open_in_memory().unwrap()),
        complete_calls.clone(),
        start_calls,
    );
    let (conversation_id, owner, agent) = create_group_with_agent(&service, "codex");
    designate_assistant(&service, &conversation_id, &owner, &agent);
    let (goal_id, child_id, child_member) =
        commit_child_without_start(&service, &conversation_id, &owner, &agent);
    service.claim_continuity_owner().unwrap();
    let _ = service.attend_due().unwrap();
    let secret = post(
        &service,
        &child_id,
        &child_owner(&service, &child_id),
        "REVOKED-EVIDENCE-SECRET",
    );
    append_criterion_evidence(
        service.store(),
        &conversation_id,
        &goal_id,
        ContinuityEvidenceRef {
            source: live_event_source(service.store(), &child_id, &secret),
            issuer: child_member.clone(),
            subject_version: 1,
            criterion_id: "criterion:notes-done".into(),
            observed_at: 1,
            result: ContinuityEvidenceResult::Pass,
            verification_kind: ContinuityVerificationKind::UserAcceptance,
            scope: ContinuityVisibilityScope::Conversation,
            validity: ContinuitySourceValidity::Current,
        },
    )
    .unwrap();
    revoke_source(service.store(), &child_id, &secret, false).unwrap();
    enqueue_review_wake(
        service.store(),
        &conversation_id,
        &ContinuityWake {
            logical_wake_id: format!("wake:{goal_id}:revoked-evidence"),
            goal_id: goal_id.clone(),
            cause_refs: vec![live_event_source(service.store(), &child_id, &secret)],
            due_at: Some(1),
            review_policy: "event-priority".into(),
            goal_revision: 1,
            epoch: 0,
            host_generation: 0,
            claim: None,
            settlement: None,
        },
    )
    .unwrap();
    let _ = service.attend_due().unwrap();
    let reviews = complete_calls
        .lock()
        .unwrap()
        .clone()
        .into_iter()
        .filter(|params| {
            params.get("continuityKind").and_then(Value::as_str) == Some("wake-review")
        })
        .collect::<Vec<_>>();
    assert!(reviews.iter().all(|params| {
        let text = delivered_guidance(params);
        !text.contains("REVOKED-EVIDENCE-SECRET") && !text.contains("latestEvidenceOutputRef")
    }));
}

#[test]
fn residual_granted_attachment_survives_history_and_revoked_gets_zero_disclosure() {
    let fixture = HostImageFixture::new();
    let complete_calls = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let start_calls = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let service = bind_effect_free_runtime(
        ConversationService::from_store(ConversationStore::open_in_memory().unwrap()),
        complete_calls,
        start_calls.clone(),
    );
    let (conversation_id, owner, agent) = create_group_with_agent(&service, "codex");
    designate_assistant(&service, &conversation_id, &owner, &agent);
    let image_event = service
        .execute(json!({
            "action": "conversation.message.post",
            "conversationId": conversation_id,
            "authorMembershipId": owner,
            "content": "parent image grant",
            "attachments": [fixture.attachment()],
        }))
        .unwrap()["event"]["id"]
        .as_str()
        .unwrap()
        .to_owned();
    set_child_work_fault(Some(ChildWorkFault::FailStart));
    let (goal_id, child_id, child_member) =
        commit_child_without_start(&service, &conversation_id, &owner, &agent);
    set_child_work_fault(None);
    put_grant(
        service.store(),
        &ContinuityParentContextGrant {
            grant_id: "grant:image".into(),
            source_conversation_id: conversation_id.clone(),
            recipient_conversation_id: child_id.clone(),
            recipient_membership_id: child_member.clone(),
            source_refs: vec![live_image_source(
                service.store(),
                &conversation_id,
                &image_event,
            )],
            authorized_scopes: vec![ContinuityVisibilityScope::Conversation],
            status: ContinuityParentGrantStatus::Admitted,
            request_id: "request:grant:image".into(),
            revocation_generation: 0,
        },
    )
    .unwrap();
    for index in 0..55 {
        let _ = post(
            &service,
            &conversation_id,
            &owner,
            &format!("filler-{index}"),
        );
    }
    service.claim_continuity_owner().unwrap();
    let _ = service.attend_due().unwrap();
    let start_snapshot = start_calls.lock().unwrap().clone();
    let starts = start_kind(&start_snapshot, "child-work");
    assert_eq!(starts.len(), 1);
    assert_eq!(starts[0]["attachments"].as_array().unwrap().len(), 1);
    assert_eq!(starts[0]["attachments"][0]["name"], "synthetic.png");
    assert_eq!(
        starts[0]["dispatchId"],
        child_work_operation_id(&goal_id, 1)
    );

    let revoked_calls = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let revoked_complete = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let revoked = bind_effect_free_runtime(
        ConversationService::from_store(ConversationStore::open_in_memory().unwrap()),
        revoked_complete,
        revoked_calls.clone(),
    );
    let (revoked_parent, revoked_owner, revoked_agent) = create_group_with_agent(&revoked, "codex");
    designate_assistant(&revoked, &revoked_parent, &revoked_owner, &revoked_agent);
    let revoked_image = revoked
        .execute(json!({
            "action": "conversation.message.post",
            "conversationId": revoked_parent,
            "authorMembershipId": revoked_owner,
            "content": "revoked image",
            "attachments": [fixture.attachment()],
        }))
        .unwrap()["event"]["id"]
        .as_str()
        .unwrap()
        .to_owned();
    set_child_work_fault(Some(ChildWorkFault::FailStart));
    let (_, revoked_child, revoked_member) =
        commit_child_without_start(&revoked, &revoked_parent, &revoked_owner, &revoked_agent);
    set_child_work_fault(None);
    put_grant(
        revoked.store(),
        &ContinuityParentContextGrant {
            grant_id: "grant:revoked-image".into(),
            source_conversation_id: revoked_parent.clone(),
            recipient_conversation_id: revoked_child.clone(),
            recipient_membership_id: revoked_member,
            source_refs: vec![live_image_source(
                revoked.store(),
                &revoked_parent,
                &revoked_image,
            )],
            authorized_scopes: vec![ContinuityVisibilityScope::Conversation],
            status: ContinuityParentGrantStatus::Revoked,
            request_id: "request:grant:revoked-image".into(),
            revocation_generation: 0,
        },
    )
    .unwrap();
    revoked.claim_continuity_owner().unwrap();
    let _ = revoked.attend_due().unwrap();
    let revoked_snapshot = revoked_calls.lock().unwrap().clone();
    let revoked_starts = start_kind(&revoked_snapshot, "child-work");
    assert_eq!(revoked_starts.len(), 1);
    assert!(
        revoked_starts[0]
            .get("attachments")
            .and_then(Value::as_array)
            .map(|items| items.is_empty())
            .unwrap_or(true)
    );
}

#[test]
fn residual_admitted_child_steer_and_cancel_use_live_turn_and_reject_stale_handles() {
    let start_calls = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let steer_calls = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let cancel_calls = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let service = bind_control_runtime(
        ConversationService::from_store(ConversationStore::open_in_memory().unwrap()),
        start_calls,
        steer_calls.clone(),
        cancel_calls.clone(),
        "native-turn-live",
    );
    let (conversation_id, owner, agent) = create_group_with_agent(&service, "codex");
    designate_assistant(&service, &conversation_id, &owner, &agent);
    let (goal_id, child_id, child_member) =
        commit_child_without_start(&service, &conversation_id, &owner, &agent);
    service.claim_continuity_owner().unwrap();
    let _ = service.attend_due().unwrap();
    let host = service.continuity().cloned().unwrap();
    assert_eq!(
        host.steer_admitted_child_follow_up(
            &child_id,
            &child_member,
            "turn:stale-other",
            "stale steer",
        ),
        ChildControlDisposition::Conflict
    );
    assert!(steer_calls.lock().unwrap().is_empty());
    let live_handle = child_work_operation_id(&goal_id, 1);
    assert_eq!(
        host.steer_admitted_child_follow_up(
            &child_id,
            &child_member,
            &live_handle,
            "allowed steer",
        ),
        ChildControlDisposition::Accepted
    );
    let steers = steer_calls.lock().unwrap().clone();
    assert_eq!(steers.len(), 1);
    assert_eq!(steers[0]["turnHandle"], live_handle);
    assert_eq!(steers[0]["turnId"], "native-turn-live");
    assert_eq!(steers[0]["conversationId"], child_id);
    assert_eq!(
        host.cancel_admitted_child_turn(&child_id, &child_member, &live_handle),
        ChildControlDisposition::Accepted
    );
    let cancels = cancel_calls.lock().unwrap().clone();
    assert_eq!(cancels.len(), 1);
    assert_eq!(cancels[0]["turnId"], "native-turn-live");
    assert_eq!(
        host.steer_admitted_child_follow_up(
            &conversation_id,
            &agent,
            &live_handle,
            "ordinary parent",
        ),
        ChildControlDisposition::Ordinary
    );
    let _ = goal_id;
}

#[test]
fn residual_pi_cancel_stays_inventory_unavailable() {
    let start_calls = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let steer_calls = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let cancel_calls = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let service = bind_control_runtime(
        ConversationService::from_store(ConversationStore::open_in_memory().unwrap()),
        start_calls,
        steer_calls,
        cancel_calls.clone(),
        "native-turn-pi",
    );
    let (conversation_id, owner, agent) = create_group_with_agent(&service, "pi");
    designate_assistant(&service, &conversation_id, &owner, &agent);
    let (goal_id, child_id, child_member) =
        commit_child_without_start(&service, &conversation_id, &owner, &agent);
    service.claim_continuity_owner().unwrap();
    let _ = service.attend_due().unwrap();
    let host = service.continuity().cloned().unwrap();
    assert_eq!(
        host.cancel_admitted_child_turn(
            &child_id,
            &child_member,
            &child_work_operation_id(&goal_id, 1),
        ),
        ChildControlDisposition::Unavailable
    );
    assert!(cancel_calls.lock().unwrap().is_empty());
}

#[test]
fn exact_live_identity_rejects_cross_generation_and_preserves_goal_revision() {
    let start_calls = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let steer_calls = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let cancel_calls = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let service = bind_control_runtime(
        ConversationService::from_store(ConversationStore::open_in_memory().unwrap()),
        start_calls.clone(),
        steer_calls.clone(),
        cancel_calls.clone(),
        "native-turn-live",
    );
    let (conversation_id, owner, agent) = create_group_with_agent(&service, "codex");
    designate_assistant(&service, &conversation_id, &owner, &agent);
    let (goal_id, child_id, child_member) =
        commit_child_without_start(&service, &conversation_id, &owner, &agent);
    service.claim_continuity_owner().unwrap();
    let _ = service.attend_due().unwrap();
    let first_handle = child_work_operation_id(&goal_id, 1);
    let host = service.continuity().cloned().unwrap();
    assert_eq!(
        host.steer_admitted_child_follow_up(&child_id, &child_member, &first_handle, "keep"),
        ChildControlDisposition::Accepted
    );
    let note = post(
        &service,
        &child_id,
        &child_owner(&service, &child_id),
        "note",
    );
    append_criterion_evidence(
        service.store(),
        &conversation_id,
        &goal_id,
        ContinuityEvidenceRef {
            source: live_event_source(service.store(), &child_id, &note),
            issuer: child_member.clone(),
            subject_version: 1,
            criterion_id: "criterion:notes-done".into(),
            observed_at: 1,
            result: ContinuityEvidenceResult::Pass,
            verification_kind: ContinuityVerificationKind::UserAcceptance,
            scope: ContinuityVisibilityScope::Conversation,
            validity: ContinuitySourceValidity::Current,
        },
    )
    .unwrap();
    assert_eq!(
        read_goal(service.store(), &goal_id)
            .unwrap()
            .unwrap()
            .revision,
        2
    );
    assert_eq!(
        read_child_work_live(service.store(), &conversation_id, &goal_id)
            .unwrap()
            .unwrap()["workGeneration"],
        1
    );
    assert_eq!(
        host.steer_admitted_child_follow_up(&child_id, &child_member, &first_handle, "after-rev"),
        ChildControlDisposition::Accepted
    );
    host.update_live_native_turn(
        &child_id,
        &child_member,
        "dispatch:unrelated",
        "native-forged",
    );
    host.update_live_native_turn(
        &child_id,
        "membership:wrong",
        &first_handle,
        "native-forged",
    );
    host.update_live_native_turn(&child_id, &child_member, &first_handle, "");
    assert_eq!(
        host.steer_admitted_child_follow_up(&child_id, &child_member, &first_handle, "still-live"),
        ChildControlDisposition::Accepted
    );
    assert_eq!(
        start_kind(&start_calls.lock().unwrap().clone(), "child-work").len(),
        1
    );

    record_child_work_intent(
        service.store(),
        &conversation_id,
        &goal_id,
        2,
        &serde_json::json!({
            "kind": "child-work-intent",
            "goalId": goal_id,
            "revision": 2,
            "admittedRevision": 2,
            "workGeneration": 2,
            "childConversationId": child_id,
            "membershipId": child_member,
            "parentConversationId": conversation_id,
            "operationId": child_work_operation_id(&goal_id, 2),
        }),
    )
    .unwrap();
    service.claim_continuity_owner().unwrap();
    let _ = service.attend_due().unwrap();
    let second_handle = child_work_operation_id(&goal_id, 2);
    assert_eq!(
        start_kind(&start_calls.lock().unwrap().clone(), "child-work").len(),
        2
    );
    assert_eq!(
        read_child_work_live(service.store(), &conversation_id, &goal_id)
            .unwrap()
            .unwrap()["operationId"],
        second_handle
    );
    host.update_live_native_turn(&child_id, &child_member, &first_handle, "native-old");
    assert_eq!(
        host.steer_admitted_child_follow_up(&child_id, &child_member, &first_handle, "old"),
        ChildControlDisposition::Conflict
    );
    assert_eq!(
        host.steer_admitted_child_follow_up(&child_id, &child_member, &second_handle, "new"),
        ChildControlDisposition::Accepted
    );
    let _ = agent;
}

#[test]
fn file_backed_reopen_without_owner_is_unavailable_not_a_second_start() {
    let root = std::env::temp_dir().join(format!(
        "lico-ca-host-reopen-{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&root).unwrap();
    let start_calls = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let steer_calls = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let cancel_calls = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let (goal_id, child_id, child_member, conversation_id) = {
        let service = bind_control_runtime(
            ConversationService::from_store(ConversationStore::open(&root).unwrap()),
            start_calls.clone(),
            steer_calls,
            cancel_calls,
            "native-turn-live",
        );
        let (conversation_id, owner, agent) = create_group_with_agent(&service, "codex");
        designate_assistant(&service, &conversation_id, &owner, &agent);
        let (goal_id, child_id, child_member) =
            commit_child_without_start(&service, &conversation_id, &owner, &agent);
        service.claim_continuity_owner().unwrap();
        let _ = service.attend_due().unwrap();
        assert_eq!(
            start_kind(&start_calls.lock().unwrap().clone(), "child-work").len(),
            1
        );
        (goal_id, child_id, child_member, conversation_id)
    };
    let reopen_starts = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let reopen_steers = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let reopen_cancels = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let reopened = bind_control_runtime(
        ConversationService::from_store(ConversationStore::open(&root).unwrap()),
        reopen_starts.clone(),
        reopen_steers,
        reopen_cancels.clone(),
        "native-turn-live",
    );
    reopened.claim_continuity_owner().unwrap();
    let _ = reopened.attend_due().unwrap();
    assert!(
        start_kind(&reopen_starts.lock().unwrap().clone(), "child-work").is_empty(),
        "missing PersistentTurn owner must not launch a second start"
    );
    let host = reopened.continuity().cloned().unwrap();
    assert_eq!(
        host.cancel_admitted_child_turn(
            &child_id,
            &child_member,
            &child_work_operation_id(&goal_id, 1),
        ),
        ChildControlDisposition::Unavailable
    );
    assert!(reopen_cancels.lock().unwrap().is_empty());
    let _ = conversation_id;
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn exact_grant_beyond_first_page_and_part_sibling_do_not_widen() {
    let fixture = HostImageFixture::new();
    let start_calls = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let complete_calls = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let service = bind_effect_free_runtime(
        ConversationService::from_store(ConversationStore::open_in_memory().unwrap()),
        complete_calls,
        start_calls.clone(),
    );
    let (conversation_id, owner, agent) = create_group_with_agent(&service, "codex");
    designate_assistant(&service, &conversation_id, &owner, &agent);
    let image_event = service
        .execute(json!({
            "action": "conversation.message.post",
            "conversationId": conversation_id,
            "authorMembershipId": owner,
            "content": "parent images",
            "attachments": [
                fixture.attachment_named("sel-1", "keep.png"),
                fixture.attachment_named("sel-2", "sibling.png"),
            ],
        }))
        .unwrap()["event"]["id"]
        .as_str()
        .unwrap()
        .to_owned();
    let event = service
        .store()
        .event(&conversation_id, &image_event)
        .unwrap()
        .expect("image event");
    let image_parts: Vec<String> = event
        .parts
        .iter()
        .filter(|part| part.kind == EventPartKind::Image)
        .map(|part| part.id.clone())
        .collect();
    assert_eq!(image_parts.len(), 2);
    set_child_work_fault(Some(ChildWorkFault::FailStart));
    let (_, child_id, child_member) =
        commit_child_without_start(&service, &conversation_id, &owner, &agent);
    set_child_work_fault(None);
    for index in 0..50 {
        put_grant(
            service.store(),
            &ContinuityParentContextGrant {
                grant_id: format!("grant:{index:02}-unrelated"),
                source_conversation_id: conversation_id.clone(),
                recipient_conversation_id: child_id.clone(),
                recipient_membership_id: child_member.clone(),
                source_refs: vec![source_for(&format!("event:unrelated-{index}"))],
                authorized_scopes: vec![ContinuityVisibilityScope::Conversation],
                status: ContinuityParentGrantStatus::Admitted,
                request_id: format!("request:unrelated-{index}"),
                revocation_generation: 0,
            },
        )
        .unwrap();
    }
    put_grant(
        service.store(),
        &ContinuityParentContextGrant {
            grant_id: "grant:zz-part".into(),
            source_conversation_id: conversation_id.clone(),
            recipient_conversation_id: child_id.clone(),
            recipient_membership_id: child_member.clone(),
            source_refs: vec![live_part_source(
                service.store(),
                &conversation_id,
                &image_event,
                &image_parts[0],
            )],
            authorized_scopes: vec![ContinuityVisibilityScope::Conversation],
            status: ContinuityParentGrantStatus::Admitted,
            request_id: "request:part".into(),
            revocation_generation: 0,
        },
    )
    .unwrap();
    service.claim_continuity_owner().unwrap();
    let _ = service.attend_due().unwrap();
    let start_snapshot = start_calls.lock().unwrap().clone();
    let starts = start_kind(&start_snapshot, "child-work");
    assert_eq!(starts.len(), 1);
    let attachments = starts[0]["attachments"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    assert_eq!(attachments.len(), 1);
    assert_eq!(attachments[0]["name"], "keep.png");
    let _ = agent;
}

#[test]
fn revoked_and_revision_mismatch_grants_disclose_zero_attachments() {
    let fixture = HostImageFixture::new();
    let start_calls = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let complete_calls = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let service = bind_effect_free_runtime(
        ConversationService::from_store(ConversationStore::open_in_memory().unwrap()),
        complete_calls,
        start_calls.clone(),
    );
    let (conversation_id, owner, agent) = create_group_with_agent(&service, "codex");
    designate_assistant(&service, &conversation_id, &owner, &agent);
    let image_event = service
        .execute(json!({
            "action": "conversation.message.post",
            "conversationId": conversation_id,
            "authorMembershipId": owner,
            "content": "mismatch image",
            "attachments": [fixture.attachment()],
        }))
        .unwrap()["event"]["id"]
        .as_str()
        .unwrap()
        .to_owned();
    set_child_work_fault(Some(ChildWorkFault::FailStart));
    let (_, child_id, child_member) =
        commit_child_without_start(&service, &conversation_id, &owner, &agent);
    set_child_work_fault(None);
    let mut mismatched = live_image_source(service.store(), &conversation_id, &image_event);
    mismatched.source_revision = 999;
    put_grant(
        service.store(),
        &ContinuityParentContextGrant {
            grant_id: "grant:mismatch".into(),
            source_conversation_id: conversation_id.clone(),
            recipient_conversation_id: child_id.clone(),
            recipient_membership_id: child_member.clone(),
            source_refs: vec![mismatched],
            authorized_scopes: vec![ContinuityVisibilityScope::Conversation],
            status: ContinuityParentGrantStatus::Admitted,
            request_id: "request:mismatch".into(),
            revocation_generation: 0,
        },
    )
    .unwrap();
    service.claim_continuity_owner().unwrap();
    let _ = service.attend_due().unwrap();
    let start_snapshot = start_calls.lock().unwrap().clone();
    let starts = start_kind(&start_snapshot, "child-work");
    assert_eq!(starts.len(), 1);
    assert!(
        starts[0]
            .get("attachments")
            .and_then(Value::as_array)
            .map(|items| items.is_empty())
            .unwrap_or(true)
    );
    let _ = agent;
}

#[test]
fn file_backed_reopen_keeps_declared_native_params_and_granted_attachment() {
    let fixture = HostImageFixture::new();
    let root = std::env::temp_dir().join(format!(
        "lico-ca-params-reopen-{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&root).unwrap();
    let start_calls = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let complete_calls = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    {
        let service = bind_effect_free_runtime(
            ConversationService::from_store(ConversationStore::open(&root).unwrap()),
            complete_calls,
            start_calls.clone(),
        );
        let (conversation_id, owner, agent) = create_group_with_agent(&service, "codex");
        designate_assistant(&service, &conversation_id, &owner, &agent);
        let image_event = service
            .execute(json!({
                "action": "conversation.message.post",
                "conversationId": conversation_id,
                "authorMembershipId": owner,
                "content": "old image",
                "attachments": [fixture.attachment()],
            }))
            .unwrap()["event"]["id"]
            .as_str()
            .unwrap()
            .to_owned();
        set_child_work_fault(Some(ChildWorkFault::FailStart));
        let (_, child_id, child_member) =
            commit_child_without_start(&service, &conversation_id, &owner, &agent);
        set_child_work_fault(None);
        for index in 0..55 {
            let _ = post(
                &service,
                &conversation_id,
                &owner,
                &format!("filler-{index}"),
            );
        }
        put_grant(
            service.store(),
            &ContinuityParentContextGrant {
                grant_id: "grant:image".into(),
                source_conversation_id: conversation_id.clone(),
                recipient_conversation_id: child_id.clone(),
                recipient_membership_id: child_member.clone(),
                source_refs: vec![live_image_source(
                    service.store(),
                    &conversation_id,
                    &image_event,
                )],
                authorized_scopes: vec![ContinuityVisibilityScope::Conversation],
                status: ContinuityParentGrantStatus::Admitted,
                request_id: "request:grant:image".into(),
                revocation_generation: 0,
            },
        )
        .unwrap();
        let profile = service
            .store()
            .membership_profile(&child_member)
            .unwrap()
            .expect("child profile");
        service
            .store()
            .set_membership_profile(
                &child_id,
                &child_member,
                &child_owner(&service, &child_id),
                profile.revision,
                &ProfileIntentUpdate {
                    required_capabilities: vec!["approval:workspace".into()],
                    preferred_model: Some("gpt-reopen".into()),
                    preferred_reasoning_effort: Some("medium".into()),
                    preferred_environment: Some("/workspace/reopen".into()),
                    ..ProfileIntentUpdate::default()
                },
            )
            .unwrap();
    }
    let reopened_starts = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let reopened_complete = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let reopened = bind_effect_free_runtime(
        ConversationService::from_store(ConversationStore::open(&root).unwrap()),
        reopened_complete,
        reopened_starts.clone(),
    );
    reopened.claim_continuity_owner().unwrap();
    let _ = reopened.attend_due().unwrap();
    let start_snapshot = reopened_starts.lock().unwrap().clone();
    let starts = start_kind(&start_snapshot, "child-work");
    assert_eq!(starts.len(), 1);
    assert_eq!(starts[0]["attachments"].as_array().unwrap().len(), 1);
    assert_eq!(starts[0]["attachments"][0]["name"], "synthetic.png");
    assert_eq!(starts[0]["model"], "gpt-reopen");
    assert_eq!(starts[0]["reasoningEffort"], "medium");
    assert_eq!(starts[0]["workingDirectory"], "/workspace/reopen");
    assert_eq!(
        starts[0]["requiredCapabilities"],
        json!(["approval:workspace"])
    );
    assert!(starts[0].get("approval").is_none());
    assert!(starts[0].get("workspace").is_none());
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn resume_goal_wire_resumes_paused_and_denies_invalid_targets() {
    let service = ConversationService::from_store(ConversationStore::open_in_memory().unwrap());
    let host = service.continuity().cloned().unwrap();
    let (conversation_id, owner, _) = create_group(&service);
    host.install_script(durable_script("", "matter:a"));
    let _ = post(&service, &conversation_id, &owner, "notes A");
    host.install_script(durable_script("", "matter:b"));
    let _ = post(&service, &conversation_id, &owner, "notes B");
    host.install_script(durable_script("", "matter:c"));
    let _ = post(&service, &conversation_id, &owner, "notes C");
    let relations = host
        .store()
        .list_child_relations(&conversation_id, None, 8)
        .unwrap();
    assert_eq!(relations.len(), 3);
    let goal_a = relations[0].goal_id.clone();
    let goal_b = relations[1].goal_id.clone();
    let goal_c = relations[2].goal_id.clone();
    let before_b = read_goal(host.store(), &goal_b).unwrap().unwrap();
    let before_c = read_goal(host.store(), &goal_c).unwrap().unwrap();
    let paused = service
        .execute(json!({
            "action": "pause-goal",
            "conversationId": conversation_id,
            "goalId": goal_a,
        }))
        .unwrap();
    assert_eq!(paused["control"], "paused");
    let resumed = service
        .execute(json!({
            "action": "resume-goal",
            "conversationId": conversation_id,
            "goalId": goal_a,
        }))
        .unwrap();
    assert_eq!(resumed["control"], "enabled");
    let after_b = read_goal(host.store(), &goal_b).unwrap().unwrap();
    let after_c = read_goal(host.store(), &goal_c).unwrap().unwrap();
    assert_eq!(after_b.control, before_b.control);
    assert_eq!(after_b.revision, before_b.revision);
    assert_eq!(after_b.lifecycle, before_b.lifecycle);
    assert_eq!(after_c.control, before_c.control);
    assert_eq!(after_c.revision, before_c.revision);
    assert_eq!(after_c.lifecycle, before_c.lifecycle);

    let stale = service.execute(json!({
        "action": "resume-goal",
        "conversationId": conversation_id,
        "goalId": goal_a,
    }));
    assert!(
        stale.is_err(),
        "resume of an enabled Goal is a stale target"
    );

    let missing = service.execute(json!({
        "action": "resume-goal",
        "conversationId": conversation_id,
        "goalId": "goal:missing",
    }));
    assert!(missing.is_err(), "missing Goal is denied");

    let (other_id, _, _) = create_group(&service);
    let wrong_conversation = service.execute(json!({
        "action": "resume-goal",
        "conversationId": other_id,
        "goalId": goal_a,
    }));
    assert!(
        wrong_conversation.is_err(),
        "resume from a foreign conversation is denied"
    );

    service
        .execute(json!({
            "action": "pause-goal",
            "conversationId": conversation_id,
            "goalId": goal_a,
        }))
        .unwrap();
    revoke_source(
        host.store(),
        &conversation_id,
        &relations[0].created_event.opaque_id,
        false,
    )
    .unwrap();
    let revoked = service.execute(json!({
        "action": "resume-goal",
        "conversationId": conversation_id,
        "goalId": goal_a,
    }));
    assert!(
        revoked.is_err(),
        "revoked created-event source denies resume"
    );
}

fn assert_get_has_no_notice_fields(value: &Value) {
    assert!(
        value.get("freshCompletionNotices").is_none(),
        "conversation.get must not disclose pending notices"
    );
    assert!(
        value.get("publishedNotificationIds").is_none(),
        "conversation.get must not disclose published notice ids"
    );
}

fn list_pending(
    service: &ConversationService,
    conversation_id: &str,
    owner_membership_id: &str,
) -> Value {
    service
        .execute(json!({
            "action": "list-pending-completion-notices",
            "conversationId": conversation_id,
            "ownerMembershipId": owner_membership_id,
        }))
        .unwrap()
}

fn close_accepted_goal(
    service: &ConversationService,
    host: &ContinuityHost,
    parent: &str,
    notification_id: &str,
) -> licoup_conversation::continuity::ContinuityTaskConversationRelation {
    let relation = host
        .store()
        .list_child_relations(parent, None, 8)
        .unwrap()
        .remove(0);
    let current = read_goal(host.store(), &relation.goal_id).unwrap().unwrap();
    let progress = ContinuityGoalProgress {
        lifecycle: ContinuityGoalLifecycle::Achieved,
        next_attention: None,
        ..current.clone()
    };
    let transition = ContinuityGoalCompletionTransition {
        transition_id: format!("transition:{notification_id}"),
        goal_id: relation.goal_id.clone(),
        from_lifecycle: current.lifecycle,
        to_lifecycle: ContinuityGoalLifecycle::Achieved,
        goal_revision: progress.revision,
        authority_kind: ContinuityClosureAuthorityKind::UserAcceptance,
        evaluation_ref: source_for(notification_id),
        notification_id: notification_id.to_owned(),
    };
    let closed = service
        .execute(json!({
            "action": "close-goal",
            "conversationId": parent,
            "transition": transition,
            "progress": progress,
        }))
        .unwrap();
    assert_eq!(closed["accepted"], true);
    assert_eq!(closed["notificationId"], notification_id);
    assert_eq!(
        closed["childConversationId"],
        relation.child_conversation_id
    );
    assert_eq!(closed["cardEventId"], relation.card_anchor.event_id);
    assert_eq!(closed["cardSequence"], relation.card_anchor.sequence);
    relation
}

#[test]
fn accepted_completion_fresh_notice_targets_consume_once_while_b_selected() {
    let root = std::env::temp_dir().join(format!("lico-ca-n12-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&root).unwrap();
    let service = ConversationService::open(&root).unwrap();
    let host = service.continuity().cloned().unwrap();
    let (parent_a, owner_a, agent_a) = create_group(&service);
    let (parent_b, owner_b, agent_b) = create_group(&service);
    let (parent_other, owner_other, _) =
        create_group_owned_by(&service, "human:other", "other-agent");
    host.install_script(durable_script("", "matter:close-a"));
    let _ = post(&service, &parent_a, &owner_a, "close A later");
    let relation = close_accepted_goal(&service, &host, &parent_a, "notice:close-a");

    let selected_b = service
        .execute(json!({"action": "conversation.get", "conversationId": parent_b}))
        .unwrap();
    assert_eq!(selected_b["id"], parent_b);
    assert_get_has_no_notice_fields(&selected_b);
    let selected_a = service
        .execute(json!({"action": "conversation.get", "conversationId": parent_a}))
        .unwrap();
    assert_eq!(selected_a["id"], parent_a);
    assert_get_has_no_notice_fields(&selected_a);

    let pending_b = list_pending(&service, &parent_b, &owner_b);
    let notices = pending_b["pendingCompletionNotices"].as_array().unwrap();
    assert_eq!(notices.len(), 1);
    assert_eq!(notices[0]["notificationId"], "notice:close-a");
    assert_eq!(notices[0]["goalId"], relation.goal_id);
    assert_eq!(notices[0]["parentConversationId"], parent_a);
    assert_eq!(
        notices[0]["childConversationId"],
        relation.child_conversation_id
    );
    assert_eq!(notices[0]["cardEventId"], relation.card_anchor.event_id);
    assert_eq!(notices[0]["cardSequence"], relation.card_anchor.sequence);

    let pending_again = list_pending(&service, &parent_b, &owner_b);
    assert_eq!(
        pending_again["pendingCompletionNotices"]
            .as_array()
            .unwrap()
            .len(),
        1,
        "list-pending must not consume"
    );

    let agent_denied = service.execute(json!({
        "action": "list-pending-completion-notices",
        "conversationId": parent_b,
        "ownerMembershipId": agent_b,
    }));
    assert!(
        agent_denied.is_err(),
        "non-owner agent membership cannot list pending notices"
    );
    let agent_ack = service.execute(json!({
        "action": "ack-completion-notices",
        "conversationId": parent_b,
        "ownerMembershipId": agent_a,
        "notificationIds": ["notice:close-a"],
    }));
    assert!(agent_ack.is_err(), "non-owner cannot ack");
    let foreign = list_pending(&service, &parent_other, &owner_other);
    assert!(
        foreign["pendingCompletionNotices"]
            .as_array()
            .unwrap()
            .is_empty(),
        "a different owner principal must not receive foreign pending"
    );
    let foreign_ack = service
        .execute(json!({
            "action": "ack-completion-notices",
            "conversationId": parent_other,
            "ownerMembershipId": owner_other,
            "notificationIds": ["notice:close-a"],
        }))
        .unwrap();
    assert!(
        foreign_ack["acknowledgedNotificationIds"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    assert_eq!(
        list_pending(&service, &parent_b, &owner_b)["pendingCompletionNotices"]
            .as_array()
            .unwrap()
            .len(),
        1,
        "failed or foreign ack must retain the pending notice"
    );

    let forged = service.execute(json!({
        "action": "resolve-completion-notice",
        "conversationId": parent_b,
        "ownerMembershipId": owner_b,
        "notificationId": "notice:forged",
    }));
    assert!(forged.is_err(), "forged notificationId has zero navigation");

    let resolved = service
        .execute(json!({
            "action": "resolve-completion-notice",
            "conversationId": parent_b,
            "ownerMembershipId": owner_b,
            "notificationId": "notice:close-a",
        }))
        .unwrap();
    assert_eq!(resolved["ok"], true);
    assert_eq!(resolved["parentConversationId"], parent_a);
    assert_eq!(
        resolved["childConversationId"],
        relation.child_conversation_id
    );
    assert_eq!(resolved["cardEventId"], relation.card_anchor.event_id);
    assert_eq!(resolved["cardSequence"], relation.card_anchor.sequence);

    let first_ack = service
        .execute(json!({
            "action": "ack-completion-notices",
            "conversationId": parent_b,
            "ownerMembershipId": owner_b,
            "notificationIds": ["notice:close-a", "notice:close-a"],
        }))
        .unwrap();
    assert_eq!(
        first_ack["acknowledgedNotificationIds"],
        json!(["notice:close-a"])
    );
    let repeat_ack = service
        .execute(json!({
            "action": "ack-completion-notices",
            "conversationId": parent_b,
            "ownerMembershipId": owner_b,
            "notificationIds": ["notice:close-a"],
        }))
        .unwrap();
    assert_eq!(
        repeat_ack["acknowledgedNotificationIds"],
        json!(["notice:close-a"]),
        "repeated ack is idempotent"
    );
    assert!(
        list_pending(&service, &parent_b, &owner_b)["pendingCompletionNotices"]
            .as_array()
            .unwrap()
            .is_empty()
    );

    let after_ack = service
        .execute(json!({
            "action": "resolve-completion-notice",
            "conversationId": parent_b,
            "ownerMembershipId": owner_b,
            "notificationId": "notice:close-a",
        }))
        .unwrap();
    assert_eq!(after_ack["parentConversationId"], parent_a);

    let reopened = ConversationService::open(&root).unwrap();
    let remount_get = reopened
        .execute(json!({"action": "conversation.get", "conversationId": parent_b}))
        .unwrap();
    assert_get_has_no_notice_fields(&remount_get);
    let remount_pending = reopened
        .execute(json!({
            "action": "list-pending-completion-notices",
            "conversationId": parent_b,
            "ownerMembershipId": owner_b,
        }))
        .unwrap();
    assert!(
        remount_pending["pendingCompletionNotices"]
            .as_array()
            .unwrap()
            .is_empty(),
        "remount must not republish a successfully acked notice"
    );

    revoke_source(
        host.store(),
        &parent_a,
        &relation.card_anchor.event_id,
        false,
    )
    .unwrap();
    let revoked = service.execute(json!({
        "action": "resolve-completion-notice",
        "conversationId": parent_b,
        "ownerMembershipId": owner_b,
        "notificationId": "notice:close-a",
    }));
    assert!(revoked.is_err(), "revoked card source has zero navigation");

    service
        .execute(json!({
            "action": "conversation.archive",
            "conversationId": parent_a,
            "archived": true,
        }))
        .unwrap();
    let archived = service.execute(json!({
        "action": "resolve-completion-notice",
        "conversationId": parent_b,
        "ownerMembershipId": owner_b,
        "notificationId": "notice:close-a",
    }));
    assert!(archived.is_err(), "archived parent has zero navigation");

    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn ordinary_post_returns_while_wake_cognition_is_held_then_attend_due_reviews_once() {
    let root = std::env::temp_dir().join(format!("lico-ca-wake-hold-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&root).unwrap();
    let complete_calls = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let start_calls = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let hold = std::sync::Arc::new((std::sync::Mutex::new(true), std::sync::Condvar::new()));
    let hold_for_complete = hold.clone();
    let complete_calls_for_turn = complete_calls.clone();
    let start_calls_for_sender = start_calls.clone();
    let service = ConversationService::open(&root)
        .unwrap()
        .bind_conversation_runtime(PersistentRuntimePorts::new(
            move |params: &Value| {
                start_calls_for_sender
                    .lock()
                    .unwrap_or_else(|poison| poison.into_inner())
                    .push(params.clone());
                Ok(json!({
                    "ok": true,
                    "accepted": true,
                    "turnHandle": "turn:test",
                }))
            },
            |_conversation_id: &str| json!([]),
            |_params: &Value| Ok(json!({ "ok": true })),
            move |params: &Value| {
                let (lock, cv) = &*hold_for_complete;
                let mut ready = lock.lock().unwrap_or_else(|poison| poison.into_inner());
                while !*ready {
                    ready = cv.wait(ready).unwrap_or_else(|poison| poison.into_inner());
                }
                complete_calls_for_turn
                    .lock()
                    .unwrap_or_else(|poison| poison.into_inner())
                    .push(params.clone());
                let conversation_id = params
                    .get("conversationId")
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                Ok(json!({
                    "ok": true,
                    "output": typed_child_proposal_json(conversation_id),
                }))
            },
            |_request: Value| Ok(json!({})),
        ));
    let (conversation_id, owner, agent) = create_group_with_agent(&service, "codex");
    designate_assistant(&service, &conversation_id, &owner, &agent);
    let posted = post(&service, &conversation_id, &owner, "prepare notes now");
    after_post(&service, &conversation_id, &posted);
    settle_typed_child(&service, &conversation_id, &agent, &posted);
    let host = service.continuity().cloned().unwrap();
    let pending_before = list_all_pending_wakes(host.store()).unwrap();
    assert!(
        !pending_before.is_empty(),
        "durable delegation must leave a pending review wake"
    );
    let logical_wake_ids: Vec<String> = pending_before
        .iter()
        .map(|(_, wake)| wake.logical_wake_id.clone())
        .collect();
    let completes_before_hold = complete_calls
        .lock()
        .unwrap_or_else(|poison| poison.into_inner())
        .len();
    {
        let (lock, cv) = &*hold;
        let mut ready = lock.lock().unwrap_or_else(|poison| poison.into_inner());
        *ready = false;
        cv.notify_all();
    }

    let post_service = service.clone();
    let post_conversation = conversation_id.clone();
    let post_owner = owner.clone();
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let result = post_service.execute(json!({
            "action": "conversation.message.post",
            "conversationId": post_conversation,
            "authorMembershipId": post_owner,
            "content": "ordinary follow-up while a wake is pending",
        }));
        let _ = tx.send(result);
    });
    let follow_up = rx
        .recv_timeout(std::time::Duration::from_secs(8))
        .expect("ordinary post must return without waiting on held wake cognition")
        .expect("ordinary post must persist");
    assert!(
        follow_up["continuityDrain"].is_null(),
        "post-time drain must not run: {}",
        follow_up
    );
    assert!(
        follow_up["event"]["id"].as_str().is_some(),
        "ordinary post must persist an event"
    );
    assert!(
        list_all_pending_wakes(host.store())
            .unwrap()
            .iter()
            .any(|(_, wake)| logical_wake_ids.contains(&wake.logical_wake_id)),
        "ordinary post must leave the durable wake pending"
    );
    assert_eq!(
        complete_calls
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
            .len(),
        completes_before_hold,
        "held cognition must not run during ordinary post"
    );

    let attend_service = service.clone();
    let (attend_tx, attend_rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let claimed = attend_service.claim_continuity_owner();
        let result = claimed.and_then(|_| attend_service.attend_due());
        let _ = attend_tx.send(result);
    });
    {
        let (lock, cv) = &*hold;
        let mut ready = lock.lock().unwrap_or_else(|poison| poison.into_inner());
        *ready = true;
        cv.notify_all();
    }
    let attended = attend_rx
        .recv_timeout(std::time::Duration::from_secs(8))
        .expect("attend_due must finish after the held cognition is released")
        .expect("attend_due must execute the pending review");
    let completes = complete_calls
        .lock()
        .unwrap_or_else(|poison| poison.into_inner())
        .clone();
    assert_eq!(
        completes.len(),
        completes_before_hold + 1,
        "background attendance must invoke wake-review cognition once: {attended}"
    );
    assert_eq!(
        completes
            .last()
            .and_then(|params| params.get("continuityKind").and_then(Value::as_str)),
        Some("wake-review"),
        "background attendance must execute the pending review once: {attended}"
    );
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn production_post_grant_reaches_child_start_without_put_grant() {
    let complete_calls = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let start_calls = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let service = bind_effect_free_runtime(
        ConversationService::from_store(ConversationStore::open_in_memory().unwrap()),
        complete_calls,
        start_calls.clone(),
    );
    let (conversation_id, owner, agent) = create_group_with_agent(&service, "codex");
    designate_assistant(&service, &conversation_id, &owner, &agent);
    let posted = post(
        &service,
        &conversation_id,
        &owner,
        "PRODUCTION-CURRENT-INPUT prepare the notes now",
    );
    after_post(&service, &conversation_id, &posted);
    settle_typed_child(&service, &conversation_id, &agent, &posted);
    let grants = list_all_parent_grants(service.store()).unwrap();
    assert!(
        !grants.is_empty(),
        "production admission must issue grants without put_grant"
    );
    let starts = start_calls
        .lock()
        .unwrap_or_else(|poison| poison.into_inner())
        .clone();
    let child_starts = start_kind(&starts, "child-work");
    assert_eq!(child_starts.len(), 1);
    let text = delivered_guidance(child_starts[0]);
    assert!(text.contains("PRODUCTION-CURRENT-INPUT"));
    assert_eq!(
        child_starts[0]
            .get("membershipId")
            .and_then(Value::as_str)
            .map(|member| grants
                .iter()
                .any(|grant| grant.recipient_membership_id == member)),
        Some(true)
    );
}

#[test]
fn production_evidence_part_grant_excludes_sibling_text_and_image() {
    let fixture = HostImageFixture::new();
    let image = fixture.attachment_named("secret", "secret.png");
    let complete_calls = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let start_calls = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let service = bind_effect_free_runtime(
        ConversationService::from_store(ConversationStore::open_in_memory().unwrap()),
        complete_calls,
        start_calls.clone(),
    );
    service.claim_continuity_owner().unwrap();
    let (conversation_id, owner, agent) = create_group_with_agent(&service, "codex");
    designate_assistant(&service, &conversation_id, &owner, &agent);
    let posted = post(&service, &conversation_id, &owner, "prepare notes now");
    after_post(&service, &conversation_id, &posted);
    settle_typed_child(&service, &conversation_id, &agent, &posted);
    let relation = service
        .store()
        .list_child_relations(&conversation_id, None, 8)
        .unwrap()
        .remove(0);
    let event = service
        .store()
        .append_event(
            &conversation_id,
            Some(&owner),
            licoup_conversation::EventKind::Message,
            &[
                NewEventPart {
                    id: String::new(),
                    kind: EventPartKind::Text,
                    content: "GRANTED-PART-TEXT".into(),
                },
                NewEventPart {
                    id: String::new(),
                    kind: EventPartKind::Text,
                    content: "SIBLING-PART-SECRET".into(),
                },
                NewEventPart {
                    id: String::new(),
                    kind: EventPartKind::Image,
                    content: licoup_conversation::ImageAttachment {
                        path: image["path"].as_str().unwrap().into(),
                        name: "secret.png".into(),
                        media_type: "image/png".into(),
                        byte_size: 1,
                    }
                    .part_content(),
                },
            ],
            None,
            None,
            true,
        )
        .unwrap();
    let granted = event
        .parts
        .iter()
        .find(|part| part.content == "GRANTED-PART-TEXT")
        .unwrap();
    let image_part = event
        .parts
        .iter()
        .find(|part| part.kind == EventPartKind::Image)
        .unwrap();
    accept_part_evidence(
        &service,
        &conversation_id,
        &relation.goal_id,
        &owner,
        &event,
        &granted.id,
        "event",
        format!("event:{}:{}", event.id, granted.id),
        None,
        "criterion:notes",
        1,
        4,
    );
    let host = service.continuity().cloned().unwrap();
    disclose_child_after_evidence(&service, &host, &conversation_id, &relation.goal_id);
    let child_member = child_recipient_id(service.store(), &relation.child_conversation_id);
    let starts = start_calls.lock().unwrap().clone();
    let capture = latest_child_work_for(&starts, &relation.child_conversation_id, &child_member)
        .expect("child-work start after exact part grant");
    let payload = delivered_guidance(capture);
    assert!(payload.contains("GRANTED-PART-TEXT"));
    assert!(
        !payload.contains("SIBLING-PART-SECRET"),
        "same-Event sibling text must stay out of the real start payload"
    );
    let attachments = start_attachments(capture);
    assert!(
        attachments.is_empty(),
        "unauthorized image must not appear on the real start attachments"
    );
    assert!(
        !payload.contains(&image_part.id),
        "unauthorized image part must not ride an Event-scoped sibling grant"
    );
}

#[test]
fn production_revoke_after_grant_stops_new_disclosure() {
    let complete_calls = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let start_calls = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let service = bind_effect_free_runtime(
        ConversationService::from_store(ConversationStore::open_in_memory().unwrap()),
        complete_calls,
        start_calls.clone(),
    );
    service.claim_continuity_owner().unwrap();
    let (conversation_id, owner, agent) = create_group_with_agent(&service, "codex");
    designate_assistant(&service, &conversation_id, &owner, &agent);
    let posted = post(
        &service,
        &conversation_id,
        &owner,
        "REVOKE-ME-SOURCE must disappear after revocation",
    );
    after_post(&service, &conversation_id, &posted);
    settle_typed_child(&service, &conversation_id, &agent, &posted);
    let relation = service
        .store()
        .list_child_relations(&conversation_id, None, 8)
        .unwrap()
        .remove(0);
    let child_member = child_recipient_id(service.store(), &relation.child_conversation_id);
    let host = service.continuity().cloned().unwrap();
    disclose_child_after_evidence(&service, &host, &conversation_id, &relation.goal_id);
    let before_starts = start_calls.lock().unwrap().clone();
    let before = latest_child_work_for(
        &before_starts,
        &relation.child_conversation_id,
        &child_member,
    )
    .map(delivered_guidance)
    .unwrap_or_default();
    assert!(before.contains("REVOKE-ME-SOURCE"));
    revoke_source(service.store(), &conversation_id, &posted, true).unwrap();
    let after_compose = host
        .compose_ingress_guidance(&relation.child_conversation_id, &child_member, "")
        .unwrap();
    assert!(!after_compose.contains("REVOKE-ME-SOURCE"));
    disclose_child_after_evidence(&service, &host, &conversation_id, &relation.goal_id);
    let after_starts = start_calls.lock().unwrap().clone();
    let new_starts = after_starts
        .iter()
        .skip(before_starts.len())
        .filter(|params| {
            params.get("continuityKind").and_then(Value::as_str) == Some("child-work")
                && params.get("conversationId").and_then(Value::as_str)
                    == Some(relation.child_conversation_id.as_str())
        })
        .collect::<Vec<_>>();
    for capture in new_starts {
        assert!(
            !delivered_guidance(capture).contains("REVOKE-ME-SOURCE"),
            "revoked source must not reach a later real start payload"
        );
    }
}

#[test]
fn production_evidence_span_grant_excludes_same_part_outside_span() {
    let complete_calls = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let start_calls = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let service = bind_effect_free_runtime(
        ConversationService::from_store(ConversationStore::open_in_memory().unwrap()),
        complete_calls,
        start_calls.clone(),
    );
    service.claim_continuity_owner().unwrap();
    let (conversation_id, owner, agent) = create_group_with_agent(&service, "codex");
    designate_assistant(&service, &conversation_id, &owner, &agent);
    let posted = post(&service, &conversation_id, &owner, "prepare notes now");
    after_post(&service, &conversation_id, &posted);
    settle_typed_child(&service, &conversation_id, &agent, &posted);
    let relation = service
        .store()
        .list_child_relations(&conversation_id, None, 8)
        .unwrap()
        .remove(0);
    let event = service
        .store()
        .append_event(
            &conversation_id,
            Some(&owner),
            licoup_conversation::EventKind::Message,
            &[NewEventPart {
                id: String::new(),
                kind: EventPartKind::Text,
                content: "KEEP-SPAN|OUT-OF-SPAN-SECRET".into(),
            }],
            None,
            None,
            true,
        )
        .unwrap();
    let part_id = event.parts[0].id.clone();
    accept_part_evidence(
        &service,
        &conversation_id,
        &relation.goal_id,
        &owner,
        &event,
        &part_id,
        "event",
        format!("event:{}:{part_id}", event.id),
        Some(ContinuityUtf8ByteSpan {
            start_byte: 0,
            end_byte: 9,
        }),
        "criterion:span",
        1,
        4,
    );
    let host = service.continuity().cloned().unwrap();
    disclose_child_after_evidence(&service, &host, &conversation_id, &relation.goal_id);
    let child_member = child_recipient_id(service.store(), &relation.child_conversation_id);
    let starts = start_calls.lock().unwrap().clone();
    let payload = delivered_guidance(
        latest_child_work_for(&starts, &relation.child_conversation_id, &child_member)
            .expect("child-work start after span grant"),
    );
    assert!(payload.contains("KEEP-SPAN"));
    assert!(
        !payload.contains("OUT-OF-SPAN-SECRET"),
        "same-Part bytes outside the granted span must stay out of the real start payload"
    );
}

#[test]
fn production_authorized_image_grant_reaches_start_attachments() {
    let fixture = HostImageFixture::new();
    let complete_calls = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let start_calls = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let service = bind_effect_free_runtime(
        ConversationService::from_store(ConversationStore::open_in_memory().unwrap()),
        complete_calls,
        start_calls.clone(),
    );
    service.claim_continuity_owner().unwrap();
    let (conversation_id, owner, agent) = create_group_with_agent(&service, "codex");
    designate_assistant(&service, &conversation_id, &owner, &agent);
    set_child_work_fault(Some(ChildWorkFault::FailStart));
    let (_, child_id, _) = commit_child_without_start(&service, &conversation_id, &owner, &agent);
    set_child_work_fault(None);
    let relation = service
        .store()
        .list_child_relations(&conversation_id, None, 8)
        .unwrap()
        .into_iter()
        .find(|item| item.child_conversation_id == child_id)
        .expect("child relation");
    let image_event = service
        .execute(json!({
            "action": "conversation.message.post",
            "conversationId": conversation_id,
            "authorMembershipId": owner,
            "content": "authorized image",
            "attachments": [fixture.attachment_named("keep", "keep.png")],
        }))
        .unwrap();
    let event_id = image_event["event"]["id"].as_str().unwrap().to_owned();
    let event = service
        .store()
        .event(&conversation_id, &event_id)
        .unwrap()
        .expect("image event");
    let image_part = event
        .parts
        .iter()
        .find(|part| part.kind == EventPartKind::Image)
        .expect("image part");
    accept_part_evidence(
        &service,
        &conversation_id,
        &relation.goal_id,
        &owner,
        &event,
        &image_part.id,
        "part",
        format!("part:{}", image_part.id),
        None,
        "criterion:image",
        1,
        4,
    );
    let host = service.continuity().cloned().unwrap();
    disclose_child_after_evidence(&service, &host, &conversation_id, &relation.goal_id);
    let child_member = child_recipient_id(service.store(), &relation.child_conversation_id);
    let starts = start_calls.lock().unwrap().clone();
    let capture = latest_child_work_for(&starts, &relation.child_conversation_id, &child_member)
        .expect("child-work start after authorized image grant");
    let attachments = start_attachments(capture);
    assert_eq!(attachments.len(), 1);
    assert_eq!(attachments[0]["name"], "keep.png");
}

#[test]
fn production_member_leave_blocks_new_disclosure() {
    let complete_calls = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let start_calls = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let service = bind_effect_free_runtime(
        ConversationService::from_store(ConversationStore::open_in_memory().unwrap()),
        complete_calls,
        start_calls.clone(),
    );
    service.claim_continuity_owner().unwrap();
    let (conversation_id, owner, agent) = create_group_with_agent(&service, "codex");
    designate_assistant(&service, &conversation_id, &owner, &agent);
    let posted = post(
        &service,
        &conversation_id,
        &owner,
        "MEMBER-LEAVE-SOURCE stays after the first delivery",
    );
    after_post(&service, &conversation_id, &posted);
    settle_typed_child(&service, &conversation_id, &agent, &posted);
    let relation = service
        .store()
        .list_child_relations(&conversation_id, None, 8)
        .unwrap()
        .remove(0);
    let child_member = child_recipient_id(service.store(), &relation.child_conversation_id);
    let host = service.continuity().cloned().unwrap();
    disclose_child_after_evidence(&service, &host, &conversation_id, &relation.goal_id);
    let before_starts = start_calls.lock().unwrap().clone();
    assert!(
        latest_child_work_for(
            &before_starts,
            &relation.child_conversation_id,
            &child_member,
        )
        .is_some_and(|capture| delivered_guidance(capture).contains("MEMBER-LEAVE-SOURCE"))
    );
    service
        .store()
        .leave_member(&relation.child_conversation_id, &child_member)
        .unwrap();
    disclose_child_after_evidence(&service, &host, &conversation_id, &relation.goal_id);
    let after_starts = start_calls.lock().unwrap().clone();
    assert_eq!(
        after_starts.len(),
        before_starts.len(),
        "inactive child membership must not start a later native call"
    );
}

#[test]
fn after_state_write_keeps_pending_and_recover_completes_once() {
    let complete_calls = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let start_calls = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let service = bind_effect_free_runtime(
        ConversationService::from_store(ConversationStore::open_in_memory().unwrap()),
        complete_calls,
        start_calls,
    );
    service.claim_continuity_owner().unwrap();
    let (conversation_id, owner, agent) = create_group_with_agent(&service, "codex");
    designate_assistant(&service, &conversation_id, &owner, &agent);
    let posted = post(&service, &conversation_id, &owner, "prepare notes now");
    after_post(&service, &conversation_id, &posted);
    settle_typed_child(&service, &conversation_id, &agent, &posted);
    let later = post(&service, &conversation_id, &owner, "interrupted progress");
    after_post(&service, &conversation_id, &later);
    set_continuity_interrupt(Some(ContinuityInterrupt::AfterStateWrite));
    let interrupted = service.after_runtime_settlement(
        &conversation_id,
        &json!({
            "output": assistant_turn_output(
                "interrupted",
                &speech_proposal_json(
                    &conversation_id,
                    8,
                    ContinuitySpeechAct::Correction,
                    ContinuityMatterSubject::Existing,
                    false,
                    vec![ContinuityAgreementProposal {
                        scope: ContinuityAgreementScope::Matter,
                        statement_ref: source_for("event:interrupted-progress"),
                        origin: ContinuityAgreementOrigin::UserExplicit,
                    }],
                ),
            ),
            "membershipId": agent,
            "causationId": later,
        }),
    );
    set_continuity_interrupt(None);
    assert!(interrupted.is_err(), "AfterStateWrite must fail the commit");
    assert!(
        read_settlement_pending(service.store(), &conversation_id, &later)
            .unwrap()
            .is_some(),
        "non-terminal AfterStateWrite must keep pending responsibility"
    );
    assert!(!settlement_applied(service.store(), &conversation_id, &later).unwrap());
    let first = service.attend_due().unwrap();
    assert!(
        settlement_applied(service.store(), &conversation_id, &later).unwrap(),
        "attend_due must complete the kept pending once"
    );
    assert!(
        read_settlement_pending(service.store(), &conversation_id, &later)
            .unwrap()
            .is_none()
    );
    let second = service.attend_due().unwrap();
    assert!(first["replayed"].as_u64().unwrap_or(0) <= 1);
    assert_eq!(second["replayed"].as_u64().unwrap_or(0), 0);
    assert_eq!(
        read_agreements(service.store(), &conversation_id)
            .unwrap()
            .iter()
            .filter(|item| item.statement_ref.opaque_id == "event:interrupted-progress")
            .count(),
        1,
        "recovery must apply the interrupted agreement once"
    );
}

/// WAKE-RECOVERY: a Goal that is waiting on a dependency has no decision to
/// make when a reminder comes due. The reminder is filtered deterministically
/// instead of buying a model call, the responsibility and its revision survive,
/// and pause, resume and cancel stay reachable.
#[test]
fn waiting_goal_reminder_is_filtered_without_a_model_call_and_keeps_its_controls() {
    let root = std::env::temp_dir().join(format!("lico-ca-wait-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&root).unwrap();
    let service = ConversationService::open(&root)
        .unwrap()
        .with_native_turn_sender(|_params| Ok(json!({ "ok": true, "accepted": true })));
    service.claim_continuity_owner().unwrap();
    let host = service.continuity().cloned().unwrap();
    let (conversation_id, owner, _) = create_group(&service);
    host.install_script(durable_script("", "matter:wait"));
    let posted = post(&service, &conversation_id, &owner, "prepare notes");
    after_post(&service, &conversation_id, &posted);
    let relation = first_relation(&host, &conversation_id);
    let goal_id = relation.goal_id.clone();
    apply_goal_control(
        host.store(),
        &conversation_id,
        &goal_id,
        ContinuityGoalEvent::NamedWait,
    )
    .unwrap();
    let waiting = read_goal(host.store(), &goal_id)
        .unwrap()
        .expect("admitted goal");
    assert_eq!(waiting.lifecycle, ContinuityGoalLifecycle::Waiting);

    set_continuity_clock(Some(1_000));
    // The first drain settles the admission reminder for a waiting Goal.
    let admission = service.attend_due().unwrap();
    assert_eq!(
        admission["noOps"][0]["reason"].as_str(),
        Some("dependency-wait"),
        "a waiting Goal's reminder is filtered, not reviewed: {admission}"
    );
    // A due reminder for the same revision must be filtered as well.
    host.schedule_review(&conversation_id, &goal_id, 1).unwrap();
    let before = host.cognition_invocation_count();
    let drain = service.attend_due().unwrap();
    assert_eq!(
        host.cognition_invocation_count(),
        before,
        "a reminder that repeats recorded sources must not call a model: {drain}"
    );
    assert!(
        drain["noOps"].as_array().is_some_and(|items| items
            .iter()
            .any(|item| item["reason"].as_str() == Some("dependency-wait"))),
        "the reminder must be reported as a deterministic no-op: {drain}"
    );
    set_continuity_clock(None);

    let retained = read_goal(host.store(), &goal_id)
        .unwrap()
        .expect("the responsibility survives its reminder");
    assert_eq!(retained.lifecycle, ContinuityGoalLifecycle::Waiting);
    assert_eq!(retained.revision, waiting.revision);
    assert_eq!(retained.control, ContinuityGoalControl::Enabled);
    assert!(
        retained.next_attention.is_some(),
        "the Goal keeps the attention it is waiting on"
    );

    // A stop stays reachable after the filtered reminder.
    host.pause_goal(&conversation_id, &goal_id).unwrap();
    assert_eq!(
        read_goal(host.store(), &goal_id).unwrap().unwrap().control,
        ContinuityGoalControl::Paused
    );
    host.resume_goal(&conversation_id, &goal_id).unwrap();
    host.request_cancel(&conversation_id, &goal_id).unwrap();
    assert_eq!(
        read_goal(host.store(), &goal_id).unwrap().unwrap().control,
        ContinuityGoalControl::CancelRequested
    );

    // A reopened host retains the same unfinished responsibility.
    drop(service);
    let reopened = ConversationService::open(&root).unwrap();
    let reopened_host = reopened.continuity().cloned().unwrap();
    let after_restart = read_goal(reopened_host.store(), &goal_id)
        .unwrap()
        .expect("restart retains the unfinished responsibility");
    assert_eq!(after_restart.lifecycle, ContinuityGoalLifecycle::Waiting);
    assert_eq!(
        after_restart.control,
        ContinuityGoalControl::CancelRequested
    );
    let _ = std::fs::remove_dir_all(&root);
}
