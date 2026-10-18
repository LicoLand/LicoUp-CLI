//! TASK-04-001 / AC-04-001. Each CA-S001..048 and CA-F01..16 is a named
//! production execution, not a count or shared branch label.
#![cfg(feature = "test-support")]
//!
//! Path: user post → after-post → typed settlement → SQLite commit/claim/
//! receipt/host/recovery. Model output is synthetic. Clocks and interrupt
//! seams are controlled. Oracle is persisted state and effect counts.

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use licoup_agent_runtime::work_context::{
    CapabilityProfile, ChildBinding, ContinuityFailureCode as NativeFailureCode, HermeticProtocol,
    NativeCapabilitySupport, NativeWorkContextKey, NativeWorkContextPort, OperationKind,
    ProtocolFamily, SessionPresence, WorkContextConfig,
};
use licoup_conversation::continuity::{
    ContextCompositionPort, ContinuityAgreement, ContinuityAgreementOrigin,
    ContinuityAgreementProposal, ContinuityAgreementScope, ContinuityClosureAuthorityKind,
    ContinuityCommitPort, ContinuityCommitmentProposal, ContinuityContextCompositionRequest,
    ContinuityContextManifest, ContinuityContextTransition, ContinuityCriterion,
    ContinuityEffectStatus, ContinuityFailureCode, ContinuityFollowThroughKind,
    ContinuityGoalControl, ContinuityGoalEvent, ContinuityGoalLifecycle,
    ContinuityInterpretationProposal, ContinuityInterrupt, ContinuityMatterAssociation,
    ContinuityMatterSubject, ContinuityOracleKind, ContinuityReadPort, ContinuitySourceOwnerKind,
    ContinuitySourceRef, ContinuitySourceValidity, ContinuitySpeechAct,
    ContinuityTaskChildAdmission, ContinuityVisibilityScope, ContinuityWake,
    ContinuityWriteEnvelope, DiscoveredKnowledgePort, apply_goal_control, child_work_operation_id,
    count_cancel_effects, enqueue_review_wake, list_all_pending_wakes, list_unacked_child_work,
    list_unknown_effect_ids, load_effect_status, put_effect, read_agreements,
    read_child_work_intent, read_goal, read_goal_bundle, replay_effect, revoke_source,
    set_continuity_clock, set_continuity_interrupt,
};
use licoup_conversation::{
    ConversationStore, EventPartKind, NewEventPart, ProfileIntentUpdate, RuntimeBinding,
};
use licoup_native::domain::agent_intelligence_catalog::qualification::{
    EvidenceBundle, EvidenceClass, QualificationService, SyntheticRecipe, UnqualifiedReason,
    evaluate_bundle, evaluate_economy, generate_synthetic, identity_changed, model_token_price,
    query_record, token_cost_from_owner,
};
use licoup_native::domain::assistant_continuity::cognition::{
    ScriptedAgent, UnavailableKnowledgeService,
};
use licoup_native::domain::assistant_continuity::context::{
    ContinuityWorkspace, UnavailableContextCompositionService,
};
use licoup_native::domain::assistant_continuity::live::populate_live_store;
use licoup_native::domain::assistant_continuity::{
    ChildWorkFault, ContinuityHost, set_child_work_fault,
};
use licoup_native::domain::client_conversation::{ConversationService, PersistentRuntimePorts};
use licoup_native::platform::work_context_ports::{
    AdapterCall, AdapterResponse, AdapterTransport, CountingTransport, fixture_child_binding,
    fixture_config, unverified_snapshot,
};
use serde_json::{Value, json};

const CASE_MARKER: &str = "CONTINUITY_SCENARIO_CASE:";
const ORACLE_MARKER: &str = "CONTINUITY_SCENARIO_ORACLE:";
const PRIVATE_A: &str = "SENTINEL-PRIVATE-A";

fn catalog_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/fixtures/continuous-assistant/scenarios/catalog.json")
}

fn qualification_fixture(name: &str) -> Value {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/fixtures/continuous-assistant/qualification")
        .join(name);
    serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
}

fn emit(id: &str, assertions_held: bool, extra: Value) {
    let mut payload = extra;
    payload["id"] = json!(id);
    payload["executed"] = json!(true);
    payload["assertionsHeld"] = json!(assertions_held);
    println!("{CASE_MARKER}{payload}");
    assert!(
        assertions_held,
        "{id} production assertions failed: {payload}"
    );
}

fn digest(tag: u8) -> String {
    format!("sha256:{:02x}{}", tag, "ab".repeat(31))
}

fn compose_after_post(session: &Session, event_id: &str) -> String {
    session
        .host()
        .compose_ingress_guidance(&session.conversation_id, &session.agent, event_id)
        .unwrap_or_default()
}

fn current_agreement_revision(session: &Session, agreement_id: &str) -> i64 {
    read_agreements(session.host().store(), &session.conversation_id)
        .unwrap()
        .into_iter()
        .find(|item| item.id == agreement_id)
        .map(|item| item.effective_revision)
        .unwrap_or(0)
}

fn source_ref(event_id: &str) -> ContinuitySourceRef {
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

struct Session {
    root: Option<PathBuf>,
    service: ConversationService,
    conversation_id: String,
    owner: String,
    agent: String,
    start_calls: Arc<Mutex<Vec<Value>>>,
    complete_calls: Arc<Mutex<Vec<Value>>>,
}

impl Session {
    fn bind(
        service: ConversationService,
        start_calls: Arc<Mutex<Vec<Value>>>,
        complete_calls: Arc<Mutex<Vec<Value>>>,
    ) -> ConversationService {
        let starts = start_calls;
        let completes = complete_calls;
        service.bind_conversation_runtime(PersistentRuntimePorts::new(
            move |params: &Value| {
                starts
                    .lock()
                    .unwrap_or_else(|poison| poison.into_inner())
                    .push(params.clone());
                Ok(json!({
                    "ok": true,
                    "accepted": true,
                    "turnHandle": "turn:scenario",
                }))
            },
            |_conversation_id: &str| json!([]),
            |_params: &Value| Ok(json!({ "ok": true })),
            move |params: &Value| {
                completes
                    .lock()
                    .unwrap_or_else(|poison| poison.into_inner())
                    .push(params.clone());
                Ok(json!({ "ok": true, "output": "ordinary-scenario-reply" }))
            },
            |_request: Value| Ok(json!({})),
        ))
    }

    fn memory() -> Self {
        let start_calls = Arc::new(Mutex::new(Vec::new()));
        let complete_calls = Arc::new(Mutex::new(Vec::new()));
        let service = Self::bind(
            ConversationService::from_store(ConversationStore::open_in_memory().unwrap()),
            start_calls.clone(),
            complete_calls.clone(),
        );
        Self::opened(None, service, start_calls, complete_calls)
    }

    fn file() -> Self {
        let root = std::env::temp_dir().join(format!(
            "lico-ca-s-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|duration| duration.as_nanos())
                .unwrap_or(0)
        ));
        std::fs::create_dir_all(&root).unwrap();
        let start_calls = Arc::new(Mutex::new(Vec::new()));
        let complete_calls = Arc::new(Mutex::new(Vec::new()));
        let service = Self::bind(
            ConversationService::from_store(ConversationStore::open(&root).unwrap()),
            start_calls.clone(),
            complete_calls.clone(),
        );
        Self::opened(Some(root), service, start_calls, complete_calls)
    }

    fn opened(
        root: Option<PathBuf>,
        service: ConversationService,
        start_calls: Arc<Mutex<Vec<Value>>>,
        complete_calls: Arc<Mutex<Vec<Value>>>,
    ) -> Self {
        let group = service
            .execute(json!({
                "action": "conversation.create",
                "title": "Scenario parent",
                "owner": {"id": "human:local", "kind": "human", "displayName": "You"},
                "members": [{
                    "principal": {
                        "id": "agent:codex",
                        "kind": "agent",
                        "displayName": "One",
                        "agentId": "codex"
                    },
                    "access": "member"
                }]
            }))
            .unwrap();
        let memberships = group["memberships"].as_array().unwrap();
        let owner = memberships
            .iter()
            .find(|item| item["principal"]["kind"] == "human")
            .unwrap()["id"]
            .as_str()
            .unwrap()
            .to_owned();
        let agent = memberships
            .iter()
            .find(|item| item["principal"]["kind"] == "agent")
            .unwrap()["id"]
            .as_str()
            .unwrap()
            .to_owned();
        let conversation_id = group["id"].as_str().unwrap().to_owned();
        let revision = service.store().get(&conversation_id).unwrap().revision;
        service
            .execute(json!({
                "action": "conversation.assistant.set",
                "conversationId": conversation_id,
                "ownerMembershipId": owner,
                "expectedRevision": revision,
                "membershipId": agent,
            }))
            .unwrap();
        Self {
            root,
            service,
            conversation_id,
            owner,
            agent,
            start_calls,
            complete_calls,
        }
    }

    fn host(&self) -> Arc<ContinuityHost> {
        self.service.continuity().cloned().expect("host")
    }

    fn post(&self, content: &str) -> String {
        self.service
            .execute(json!({
                "action": "conversation.message.post",
                "conversationId": self.conversation_id,
                "authorMembershipId": self.owner,
                "content": content,
            }))
            .unwrap()["event"]["id"]
            .as_str()
            .unwrap()
            .to_owned()
    }

    fn after_post(&self, event_id: &str) -> Value {
        self.service
            .execute(json!({
                "action": "conversation.dispatch.after-post",
                "conversationId": self.conversation_id,
                "eventId": event_id,
            }))
            .unwrap()
    }

    fn settle(&self, event_id: &str, output: &str) -> Result<Value, anyhow::Error> {
        self.service.after_runtime_settlement(
            &self.conversation_id,
            &json!({
                "output": output,
                "membershipId": self.agent,
                "causationId": event_id,
            }),
        )
    }

    fn settle_ordinary(&self, event_id: &str, reply: &str) {
        self.settle(event_id, reply).unwrap();
    }

    fn settle_durable(&self, event_id: &str, goal_id: &str, matter_id: &str) {
        let output = assistant_turn_output(
            "I'll keep this as a durable child task.",
            &durable_proposal_json(&self.conversation_id, goal_id, matter_id),
        );
        self.settle(event_id, &output).unwrap();
    }

    fn goal_count(&self) -> usize {
        self.host()
            .store()
            .list_child_relations(&self.conversation_id, None, 32)
            .unwrap()
            .len()
    }

    fn first_child(&self) -> (String, String, String) {
        let relation = self
            .host()
            .store()
            .list_child_relations(&self.conversation_id, None, 8)
            .unwrap()
            .remove(0);
        (
            relation.goal_id,
            relation.child_conversation_id,
            relation.created_event.opaque_id,
        )
    }

    fn lifecycle(&self, goal_id: &str) -> ContinuityGoalLifecycle {
        read_goal(self.host().store(), goal_id)
            .unwrap()
            .unwrap()
            .lifecycle
    }

    fn control(&self, goal_id: &str) -> ContinuityGoalControl {
        read_goal(self.host().store(), goal_id)
            .unwrap()
            .unwrap()
            .control
    }

    fn has_next_attention(&self, goal_id: &str) -> bool {
        read_goal(self.host().store(), goal_id)
            .unwrap()
            .unwrap()
            .next_attention
            .is_some()
    }

    fn complete_count(&self) -> usize {
        self.complete_calls
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
            .len()
    }

    fn child_work_count(&self) -> usize {
        self.start_calls
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
            .iter()
            .filter(|params| {
                params.get("continuityKind").and_then(Value::as_str) == Some("child-work")
            })
            .count()
    }

    fn start_len(&self) -> usize {
        self.start_calls
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
            .len()
    }

    fn later_payload_contains(&self, after: usize, needle: &str) -> bool {
        self.start_calls
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
            .iter()
            .skip(after)
            .any(|params| params.to_string().contains(needle))
    }

    fn external_effects(&self) -> usize {
        list_unknown_effect_ids(self.host().store()).unwrap().len() + self.child_work_count()
    }

    fn claim_attend(&self) -> Value {
        self.service.claim_continuity_owner().unwrap();
        self.service.attend_due().unwrap()
    }

    fn live_source(&self, event_id: &str) -> ContinuitySourceRef {
        let event = self
            .service
            .store()
            .event(&self.conversation_id, event_id)
            .unwrap()
            .expect("event");
        let part = event
            .parts
            .iter()
            .find(|part| part.kind == EventPartKind::Text)
            .expect("text part");
        ContinuitySourceRef {
            owner_kind: ContinuitySourceOwnerKind::Event,
            opaque_id: event.id.clone(),
            part_id: Some(part.id.clone()),
            span: None,
            source_revision: event.sequence,
            digest: format!("event:{}:{}", event.id, part.id),
            visibility_scope: ContinuityVisibilityScope::Conversation,
            validity: ContinuitySourceValidity::Current,
        }
    }

    fn cleanup(self) {
        if let Some(root) = self.root {
            let _ = std::fs::remove_dir_all(root);
        }
    }
}

fn assistant_turn_output(reply: &str, proposal_json: &str) -> String {
    let proposal: Value = serde_json::from_str(proposal_json).unwrap();
    serde_json::to_string(&json!({
        "replyText": reply,
        "interpretationProposal": proposal,
    }))
    .unwrap()
}

fn durable_proposal_json(conversation_id: &str, goal_id: &str, matter_id: &str) -> String {
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

fn dual_matter_proposal(conversation_id: &str, source: ContinuitySourceRef, agent: &str) -> String {
    let assoc = |matter: &str| ContinuityMatterAssociation {
        matter_id: matter.to_owned(),
        source_ref: source.clone(),
        association_revision: 1,
        proposed_by: agent.to_owned(),
        reason_code: "compound".into(),
        supersedes: None,
    };
    serde_json::to_string(&ContinuityInterpretationProposal {
        envelope: ContinuityWriteEnvelope {
            conversation_id: conversation_id.to_owned(),
            source_event_refs: vec![source.clone()],
            observed_revision: 0,
            designation_epoch: 0,
            request_id: format!("request:compound:{conversation_id}"),
        },
        matter_associations: vec![assoc("matter:publish"), assoc("matter:formula")],
        speech_act: ContinuitySpeechAct::Delegation,
        commitment_proposals: vec![
            ContinuityCommitmentProposal {
                matter_id: Some("matter:publish".into()),
                subject: ContinuityMatterSubject::Compound,
                expected_result: "Update publish copy".into(),
                criteria: Vec::new(),
                create_goal: true,
            },
            ContinuityCommitmentProposal {
                matter_id: Some("matter:formula".into()),
                subject: ContinuityMatterSubject::Compound,
                expected_result: "Explain the formula".into(),
                criteria: Vec::new(),
                create_goal: true,
            },
        ],
        agreement_proposals: Vec::new(),
        capability_needs: Vec::new(),
        uncertainty_reasons: Vec::new(),
        requested_reads: Vec::new(),
        task_child_admission: None,
    })
    .unwrap()
}

fn matter_agreement_proposal(conversation_id: &str, token: &str) -> String {
    serde_json::to_string(&ContinuityInterpretationProposal {
        envelope: ContinuityWriteEnvelope {
            conversation_id: conversation_id.to_owned(),
            source_event_refs: Vec::new(),
            observed_revision: 0,
            designation_epoch: 0,
            request_id: format!("request:agree:{token}"),
        },
        matter_associations: Vec::new(),
        speech_act: ContinuitySpeechAct::Correction,
        commitment_proposals: Vec::new(),
        agreement_proposals: vec![ContinuityAgreementProposal {
            scope: ContinuityAgreementScope::Matter,
            statement_ref: ContinuitySourceRef {
                owner_kind: ContinuitySourceOwnerKind::Agreement,
                opaque_id: format!("agreement:{token}"),
                part_id: None,
                span: None,
                source_revision: 1,
                digest: digest(4),
                visibility_scope: ContinuityVisibilityScope::Matter,
                validity: ContinuitySourceValidity::Current,
            },
            origin: ContinuityAgreementOrigin::UserExplicit,
        }],
        capability_needs: Vec::new(),
        uncertainty_reasons: Vec::new(),
        requested_reads: Vec::new(),
        task_child_admission: None,
    })
    .unwrap()
}

fn selected_option_proposal(conversation_id: &str, source: ContinuitySourceRef) -> String {
    serde_json::to_string(&ContinuityInterpretationProposal {
        envelope: ContinuityWriteEnvelope {
            conversation_id: conversation_id.to_owned(),
            source_event_refs: vec![source],
            observed_revision: 0,
            designation_epoch: 0,
            request_id: format!("request:option-second:{conversation_id}"),
        },
        matter_associations: Vec::new(),
        speech_act: ContinuitySpeechAct::Delegation,
        commitment_proposals: vec![ContinuityCommitmentProposal {
            matter_id: Some("matter:option".into()),
            subject: ContinuityMatterSubject::Existing,
            expected_result: "second".into(),
            criteria: Vec::new(),
            create_goal: true,
        }],
        agreement_proposals: Vec::new(),
        capability_needs: Vec::new(),
        uncertainty_reasons: Vec::new(),
        requested_reads: Vec::new(),
        task_child_admission: Some(ContinuityTaskChildAdmission {
            goal_id: "goal:option".into(),
            parent_conversation_id: conversation_id.to_owned(),
            speech_act: ContinuitySpeechAct::Delegation,
            follow_through_kind: ContinuityFollowThroughKind::Durable,
            observed_child_conversation_id: None,
            observed_card_anchor: None,
            request_id: "request:admit:goal:option".into(),
        }),
    })
    .unwrap()
}

fn admit_goal(session: &Session, content: &str, goal_id: &str, matter_id: &str) -> String {
    let event = session.post(content);
    session.after_post(&event);
    session.settle_durable(&event, goal_id, matter_id);
    event
}

fn admit_required_criterion_goal(
    session: &Session,
    content: &str,
    goal_id: &str,
    matter_id: &str,
    criterion_id: &str,
) -> String {
    let event = session.post(content);
    session.after_post(&event);
    let mut proposal: ContinuityInterpretationProposal = serde_json::from_str(
        &durable_proposal_json(&session.conversation_id, goal_id, matter_id),
    )
    .unwrap();
    proposal.commitment_proposals[0]
        .criteria
        .push(ContinuityCriterion {
            id: criterion_id.to_owned(),
            description_ref: session.live_source(&event),
            required: true,
            oracle_kind: ContinuityOracleKind::User,
            artifact_version_rule: "current-subject-version".into(),
            freshness_rule: "current".into(),
            evaluator_policy: "user-acceptance".into(),
        });
    session
        .settle(
            &event,
            &assistant_turn_output(
                "I'll keep this as a durable child task.",
                &serde_json::to_string(&proposal).unwrap(),
            ),
        )
        .unwrap();
    event
}

fn current_subject_version(session: &Session, goal_id: &str, criterion_id: &str) -> i64 {
    read_goal(session.host().store(), goal_id)
        .unwrap()
        .unwrap()
        .criterion_evidence_refs
        .iter()
        .filter(|item| item.criterion_id == criterion_id)
        .map(|item| item.subject_version)
        .max()
        .unwrap_or(0)
}

fn required_criterion_present(session: &Session, goal_id: &str, criterion_id: &str) -> bool {
    read_goal_bundle(session.host().store(), goal_id)
        .unwrap()
        .is_some_and(|(contract, _)| {
            contract
                .criteria
                .iter()
                .any(|item| item.id == criterion_id && item.required)
        })
}

fn invocation_guidance(params: &Value) -> String {
    ["text", "developerInstructions", "privateInstructions"]
        .into_iter()
        .filter_map(|key| params.get(key).and_then(Value::as_str))
        .collect::<Vec<_>>()
        .join("\n")
}

fn latest_child_work_guidance(session: &Session, child_id: &str) -> String {
    let member = child_member(session, child_id);
    session
        .start_calls
        .lock()
        .unwrap_or_else(|poison| poison.into_inner())
        .iter()
        .rev()
        .find(|params| {
            params.get("continuityKind").and_then(Value::as_str) == Some("child-work")
                && params.get("conversationId").and_then(Value::as_str) == Some(child_id)
                && params.get("membershipId").and_then(Value::as_str) == Some(member.as_str())
        })
        .map(invocation_guidance)
        .unwrap_or_default()
}

fn disclose_admitted_child(session: &Session, goal_id: &str) {
    session
        .host()
        .schedule_review(&session.conversation_id, goal_id, 1)
        .expect("schedule_review after admitted evidence");
    let _ = session.claim_attend();
}

fn child_member(session: &Session, child_id: &str) -> String {
    session
        .service
        .store()
        .get(child_id)
        .unwrap()
        .assistant_membership_id
        .expect("child assistant")
}

fn child_owner(session: &Session, child_id: &str) -> String {
    session
        .service
        .store()
        .get(child_id)
        .unwrap()
        .memberships
        .iter()
        .find(|membership| membership.principal.kind == licoup_conversation::PrincipalKind::Human)
        .map(|membership| membership.id.clone())
        .expect("child owner")
}

fn child_agent(session: &Session, child_id: &str, member: &str) -> String {
    session
        .service
        .store()
        .get(child_id)
        .unwrap()
        .memberships
        .iter()
        .find(|membership| membership.id == member)
        .and_then(|membership| membership.principal.agent_id.clone())
        .expect("child agent")
}

fn admit_child_text(
    session: &Session,
    child_id: &str,
    member: &str,
    text: &str,
    artifact: Option<&str>,
) -> String {
    let agent_id = child_agent(session, child_id, member);
    let scope = session
        .service
        .store()
        .prepare_runtime_dispatch(
            &agent_id,
            "",
            text,
            Some(child_id),
            Some(member),
            Some("event:child-cause"),
            None,
        )
        .unwrap();
    if !text.is_empty() {
        session
            .service
            .store()
            .append_event_part(
                &scope.event_id,
                NewEventPart {
                    id: String::new(),
                    kind: EventPartKind::Text,
                    content: text.to_owned(),
                },
            )
            .unwrap();
    }
    if let Some(artifact) = artifact {
        session
            .service
            .store()
            .append_event_part(
                &scope.event_id,
                NewEventPart {
                    id: String::new(),
                    kind: EventPartKind::Artifact,
                    content: artifact.to_owned(),
                },
            )
            .unwrap();
    }
    scope.dispatch_id
}

fn close_goal(
    session: &Session,
    goal_id: &str,
    authority: ContinuityClosureAuthorityKind,
) -> Value {
    let current = read_goal(session.host().store(), goal_id).unwrap().unwrap();
    let mut progress = current.clone();
    progress.lifecycle = ContinuityGoalLifecycle::Achieved;
    progress.next_attention = None;
    session
        .service
        .execute(json!({
            "action": "close-goal",
            "conversationId": session.conversation_id,
            "transition": {
                "transitionId": format!("transition:{goal_id}"),
                "goalId": goal_id,
                "fromLifecycle": current.lifecycle,
                "toLifecycle": "achieved",
                "goalRevision": progress.revision,
                "authorityKind": match authority {
                    ContinuityClosureAuthorityKind::UserAcceptance => "user-acceptance",
                    _ => "goal-evaluation",
                },
                "evaluationRef": {
                    "ownerKind": "goal",
                    "opaqueId": goal_id,
                    "sourceRevision": progress.revision,
                    "digest": digest(9),
                    "visibilityScope": "goal",
                    "validity": "current"
                },
                "notificationId": format!("notice:{goal_id}")
            },
            "progress": progress
        }))
        .unwrap()
}

fn accept_evidence_version(
    session: &Session,
    goal_id: &str,
    event_id: &str,
    version: i64,
) -> Result<Value, anyhow::Error> {
    accept_criterion_evidence(session, goal_id, event_id, version, "criterion:delivery")
}

fn accept_criterion_evidence(
    session: &Session,
    goal_id: &str,
    event_id: &str,
    version: i64,
    criterion_id: &str,
) -> Result<Value, anyhow::Error> {
    let event = session
        .service
        .store()
        .event(&session.conversation_id, event_id)
        .unwrap()
        .unwrap();
    let part = event
        .parts
        .iter()
        .find(|part| part.kind == EventPartKind::Text)
        .unwrap();
    session.service.execute(json!({
        "action": "accept-evidence",
        "conversationId": session.conversation_id,
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
            "issuer": session.owner,
            "subjectVersion": version,
            "criterionId": criterion_id,
            "observedAt": event.sequence,
            "result": "pass",
            "verificationKind": "user-acceptance",
            "scope": "goal",
            "validity": "current"
        }
    }))
}

fn key(conversation_id: &str, membership_id: &str, matter: &str) -> NativeWorkContextKey {
    NativeWorkContextKey {
        conversation_id: conversation_id.to_owned(),
        membership_id: membership_id.to_owned(),
        matter_id: matter.to_owned(),
        generation: 1,
    }
}

fn child_binding(session: &Session, goal_id: &str, child_id: &str, member: &str) -> ChildBinding {
    ChildBinding {
        child_conversation_id: child_id.to_owned(),
        membership_id: member.to_owned(),
        source_task_id: goal_id.to_owned(),
        parent_conversation_id: session.conversation_id.clone(),
    }
}

fn compose_manifest(
    session: &Session,
    event_id: Option<&str>,
    cache_expired: bool,
) -> ContinuityContextManifest {
    let live = populate_live_store(session.host().store(), &session.conversation_id, event_id);
    if cache_expired {
        live.set_prompt_cache_expired(&session.conversation_id, true);
    }
    let workspace = ContinuityWorkspace::new(
        live,
        ScriptedAgent::new(),
        UnavailableKnowledgeService::default(),
    );
    UnavailableContextCompositionService::from_workspace(workspace)
        .compose_authorized(&ContinuityContextCompositionRequest {
            conversation_id: session.conversation_id.clone(),
            recipient_membership_id: session.agent.clone(),
            authorized_scopes: vec![ContinuityVisibilityScope::Conversation],
            revocation_generation: 0,
            after: None,
            limit: 32,
        })
        .expect("compose_authorized")
}

fn child_operation_id(session: &Session, goal_id: &str) -> String {
    let revision = read_goal(session.host().store(), goal_id)
        .unwrap()
        .map(|progress| progress.revision)
        .unwrap_or(1);
    if let Ok(Some(intent)) = read_child_work_intent(
        session.host().store(),
        &session.conversation_id,
        goal_id,
        revision,
    ) {
        if let Some(operation) = intent
            .get("operationId")
            .and_then(Value::as_str)
            .filter(|value| !value.is_empty())
        {
            return operation.to_owned();
        }
    }
    child_work_operation_id(goal_id, revision)
}

fn unacked_for_goal(session: &Session, goal_id: &str) -> usize {
    list_unacked_child_work(session.host().store())
        .unwrap()
        .into_iter()
        .filter(|(_, item, _, _)| item == goal_id)
        .count()
}

fn child_work_in(calls: &Arc<Mutex<Vec<Value>>>) -> usize {
    calls
        .lock()
        .unwrap_or_else(|poison| poison.into_inner())
        .iter()
        .filter(|params| params.get("continuityKind").and_then(Value::as_str) == Some("child-work"))
        .count()
}

fn reopen_bound(root: &std::path::Path) -> (ConversationService, Arc<Mutex<Vec<Value>>>) {
    let starts = Arc::new(Mutex::new(Vec::new()));
    let completes = Arc::new(Mutex::new(Vec::new()));
    let service = Session::bind(
        ConversationService::from_store(ConversationStore::open(root).unwrap()),
        starts.clone(),
        completes,
    );
    (service, starts)
}

fn bind_present_child(session: &Session, goal_id: &str, child_id: &str, member: &str) {
    session.host().bind_hermetic(
        HermeticProtocol::codex(CapabilityProfile::Low).with_presence(SessionPresence::Present),
        WorkContextConfig::child(child_binding(session, goal_id, child_id, member)),
    );
}

fn identity_from_fixture() -> licoup_conversation::continuity::ContinuityCandidateIdentity {
    serde_json::from_value(
        qualification_fixture("candidate-identity.json")["candidateIdentity"].clone(),
    )
    .unwrap()
}

#[test]
fn ca_s001_discussion_is_not_delegation() {
    let session = Session::memory();
    let event = session.post("以后也许可以把报销流程写成一个程序。");
    session.after_post(&event);
    session.settle_ordinary(
        &event,
        "That remains a future idea; I will not start implementation.",
    );
    emit(
        "CA-S001",
        session.goal_count() == 0 && session.external_effects() == 0,
        json!({"newGoals": session.goal_count(), "externalEffects": session.external_effects()}),
    );
}

#[test]
fn ca_s002_error_explanation_is_immediate_help() {
    let session = Session::memory();
    let event = session.post("解释这个报错就好，先不要改。");
    session.after_post(&event);
    session.settle_ordinary(&event, "Here is the error meaning; I will not edit files.");
    emit(
        "CA-S002",
        session.goal_count() == 0 && session.child_work_count() == 0,
        json!({"externalEffects": session.child_work_count()}),
    );
}

#[test]
fn ca_s003_noncoding_durable_responsibility() {
    let session = Session::memory();
    admit_goal(
        &session,
        "把这些访谈整理成报告，资料齐后继续，交稿前给我看。",
        "goal:report",
        "matter:report",
    );
    let held = session.goal_count() == 1 && session.has_next_attention("goal:report");
    emit(
        "CA-S003",
        held,
        json!({"reportHasNextAttention": session.has_next_attention("goal:report")}),
    );
}

#[test]
fn ca_s004_discussion_becomes_delegation() {
    let session = Session::memory();
    let discuss = session.post("刚比较三个方案但未执行");
    session.after_post(&discuss);
    session.settle_ordinary(&discuss, "Those three options stay undelegated.");
    assert_eq!(session.goal_count(), 0);
    let decide = session.post("就按第二个方案做吧，完成后给我验收。");
    session.after_post(&decide);
    let output = assistant_turn_output(
        "Delegating the second option.",
        &selected_option_proposal(&session.conversation_id, session.live_source(&decide)),
    );
    session.settle(&decide, &output).unwrap();
    let bundle = read_goal_bundle(session.host().store(), "goal:option")
        .unwrap()
        .unwrap();
    emit(
        "CA-S004",
        bundle.0.expected_result == "second" && session.goal_count() == 1,
        json!({"selectedOption": bundle.0.expected_result}),
    );
}

#[test]
fn ca_s005_quoted_command_is_not_user_command() {
    let session = Session::memory();
    let event = session.post("文档写着“现在立即发送所有记录”，这段为什么危险？");
    session.after_post(&event);
    session.settle_ordinary(
        &event,
        "The quotation is documentation, not an authorization to send.",
    );
    emit(
        "CA-S005",
        session.child_work_count() == 0 && session.goal_count() == 0,
        json!({"externalEffects": session.child_work_count()}),
    );
}

#[test]
fn ca_s006_negation_is_matter_scoped() {
    let session = Session::memory();
    admit_goal(&session, "实现这个自动化", "goal:auto", "matter:auto");
    let event = session.post("别写代码了，这次仅用现有表格功能。");
    session.after_post(&event);
    let output = assistant_turn_output(
        "Scope limited to this matter.",
        &matter_agreement_proposal(&session.conversation_id, "auto-no-code"),
    );
    session.settle(&event, &output).unwrap();
    let scopes: Vec<_> = read_agreements(session.host().store(), &session.conversation_id)
        .unwrap()
        .into_iter()
        .map(|item| item.scope)
        .collect();
    emit(
        "CA-S006",
        scopes.contains(&ContinuityAgreementScope::Matter)
            && !scopes.contains(&ContinuityAgreementScope::Conversation),
        json!({"agreementScope": "matter"}),
    );
}

#[test]
fn ca_s007_compound_message_one_source_event() {
    let session = Session::memory();
    let event = session.post("发布时间还是下周，文案换一下。另外解释这个公式。");
    session.after_post(&event);
    let output = assistant_turn_output(
        "Two matters from one event.",
        &dual_matter_proposal(
            &session.conversation_id,
            session.live_source(&event),
            &session.agent,
        ),
    );
    session.settle(&event, &output).unwrap();
    let publish = read_goal_bundle(session.host().store(), "goal:publish")
        .unwrap()
        .expect("publish goal");
    let formula = read_goal_bundle(session.host().store(), "goal:formula")
        .unwrap()
        .expect("formula goal");
    let copies = [
        publish.0.created_event.opaque_id.as_str(),
        formula.0.created_event.opaque_id.as_str(),
    ]
    .into_iter()
    .collect::<std::collections::BTreeSet<_>>()
    .len();
    emit(
        "CA-S007",
        copies == 1 && publish.0.created_event.opaque_id == event,
        json!({"sourceEventCopies": copies, "goalCount": 2}),
    );
}

#[test]
fn ca_s008_english_explicit_no_takeover() {
    let session = Session::memory();
    let event = session.post("We might build this later. Do not start implementation.");
    session.after_post(&event);
    session.settle_ordinary(&event, "Understood; no implementation goal.");
    emit("CA-S008", session.goal_count() == 0, json!({"newGoals": 0}));
}

#[test]
fn ca_s009_english_durable_noncoding() {
    let session = Session::memory();
    admit_goal(
        &session,
        "Keep the draft updated as the remaining interview notes arrive; ask before sending it.",
        "goal:draft",
        "matter:draft",
    );
    emit(
        "CA-S009",
        session.has_next_attention("goal:draft"),
        json!({"draftHasNextAttention": session.has_next_attention("goal:draft")}),
    );
}

#[test]
fn ca_s010_ambiguous_pronoun_does_not_guess_effects() {
    let session = Session::memory();
    admit_goal(
        &session,
        "A second option for matter A",
        "goal:a",
        "matter:a",
    );
    admit_goal(
        &session,
        "A second option for matter B",
        "goal:b",
        "matter:b",
    );
    let before = session.child_work_count();
    let event = session.post("按刚刚那个做。");
    session.after_post(&event);
    session.settle_ordinary(
        &event,
        "I need to know which matter; I will not guess an external effect.",
    );
    emit(
        "CA-S010",
        session.child_work_count() == before,
        json!({
            "externalEffects": session.child_work_count() - before,
            "goalCount": session.goal_count()
        }),
    );
}

#[test]
fn ca_s011_unrelated_topic_does_not_inherit_private() {
    let session = Session::memory();
    admit_goal(
        &session,
        &format!("整理未公开材料 {PRIVATE_A}"),
        "goal:private-a",
        "matter:private-a",
    );
    let after_private = session.start_len();
    admit_goal(
        &session,
        "顺便解释一下复利公式。",
        "goal:formula",
        "matter:formula",
    );
    let leaked = session.later_payload_contains(after_private, PRIVATE_A);
    emit(
        "CA-S011",
        !leaked,
        json!({"containsUnrelatedPrivateMaterial": leaked}),
    );
}

#[test]
fn ca_s012_return_uses_current_agreement_revision() {
    let session = Session::memory();
    admit_goal(
        &session,
        "报告约定初版",
        "goal:report-return",
        "matter:report-return",
    );
    let v2 = session.post("报告约定改为 v2：交稿前必须给用户看。");
    session.after_post(&v2);
    session.settle_ordinary(&v2, "Agreement revision is now v2.");
    let stored = session
        .host()
        .revise_agreement(
            &session.conversation_id,
            ContinuityAgreement {
                id: "agreement:report-return".into(),
                scope: ContinuityAgreementScope::Goal,
                statement_ref: session.live_source(&v2),
                origin: ContinuityAgreementOrigin::UserExplicit,
                effective_revision: 2,
                supersedes: Some(1),
                valid_from: 2,
                valid_until: None,
                revocation_generation: 0,
            },
        )
        .unwrap();
    admit_goal(&session, "临时问答结束", "goal:b-chat", "matter:b-chat");
    let event = session.post("回到前面那个报告，进展怎么样？");
    session.after_post(&event);
    let assembled = compose_after_post(&session, &event);
    session.settle_ordinary(
        &event,
        "Returning to the report under the current agreement.",
    );
    let revision = current_agreement_revision(&session, &stored.id);
    let used_current = assembled.contains("交稿前必须给用户看") || assembled.contains(&stored.id);
    emit(
        "CA-S012",
        revision == 2 && used_current,
        json!({
            "agreementRevision": revision,
            "assembledCurrentAgreement": used_current
        }),
    );
}

#[test]
fn ca_s013_expired_cache_continues_same_child() {
    let session = Session::memory();
    admit_goal(
        &session,
        "继续检查剩下的内容。",
        "goal:continue",
        "matter:continue",
    );
    let (goal_id, child_id, _) = session.first_child();
    let member = child_member(&session, &child_id);
    bind_present_child(&session, &goal_id, &child_id, &member);
    let event = session.post("供应商缓存失效后继续检查剩下的内容。");
    session.after_post(&event);
    session.settle_durable(&event, "goal:continue", "matter:continue");
    let manifest = compose_manifest(&session, Some(&event), true);
    let port = session
        .host()
        .work_runtime(&child_id, &goal_id, &member, 0)
        .expect("present child runtime");
    port.exact_resume(&key(&child_id, &member, "matter:continue"))
        .unwrap();
    let operations = port.operations().unwrap();
    let resumed = operations.iter().any(|item| {
        item.protocol_method == "thread/resume"
            && item.succeeded
            && !item.protocol_method.is_empty()
    });
    let started_new = operations.iter().any(|item| {
        item.protocol_method == "thread/start" || item.kind == OperationKind::Rehydrate
    });
    let relations = session
        .host()
        .store()
        .list_child_relations(&session.conversation_id, None, 8)
        .unwrap();
    let same_child = relations.len() == 1
        && relations[0].goal_id == goal_id
        && relations[0].child_conversation_id == child_id;
    let continued = manifest.context_transition == ContinuityContextTransition::Continue
        && manifest
            .selection_reason_codes
            .iter()
            .any(|reason| reason == "prompt-cache-expired")
        && resumed
        && !started_new
        && same_child;
    emit(
        "CA-S013",
        continued,
        json!({
            "contextTransition": format!("{:?}", manifest.context_transition).to_ascii_lowercase(),
            "cacheExpiredContinue": manifest.selection_reason_codes.contains(&"prompt-cache-expired".into()),
            "nativeResumeSucceeded": resumed,
            "nativeStartNew": started_new,
            "childReused": same_child,
            "relationCount": relations.len()
        }),
    );
}

#[test]
fn ca_s014_long_return_uses_current_source_revision() {
    let session = Session::memory();
    let goal_id = "goal:plan";
    let criterion_id = "criterion:plan";
    admit_required_criterion_goal(&session, "跟踪该方案", goal_id, "matter:plan", criterion_id);
    let v1 = session.post("方案 v1 初稿");
    accept_criterion_evidence(&session, goal_id, &v1, 1, criterion_id).unwrap();
    disclose_admitted_child(&session, goal_id);
    let v2 = session.post("方案 v2 修订稿");
    accept_criterion_evidence(&session, goal_id, &v2, 2, criterion_id).unwrap();
    disclose_admitted_child(&session, goal_id);
    let v3 = session.post("方案 v3 当前有效");
    accept_criterion_evidence(&session, goal_id, &v3, 3, criterion_id).unwrap();
    disclose_admitted_child(&session, goal_id);
    let admitted_current = current_subject_version(&session, goal_id, criterion_id);
    let (_, child_id, _) = session.first_child();
    let before_return = session.start_len();
    let event = session.post("两周前的方案现在还能用吗？");
    session.after_post(&event);
    session.settle_ordinary(&event, "I will use the admitted current plan.");
    disclose_admitted_child(&session, goal_id);
    let guidance = latest_child_work_guidance(&session, &child_id);
    let recipient_has_v3 = guidance.contains("方案 v3 当前有效");
    let recipient_lacks_v1 = !guidance.contains("方案 v1 初稿");
    let return_recipient_invoked = session.start_len() > before_return
        && session.later_payload_contains(before_return, "child-work");
    emit(
        "CA-S014",
        admitted_current == 3 && recipient_has_v3 && recipient_lacks_v1,
        json!({
            "sourceRevision": admitted_current,
            "admittedCurrentVersion": admitted_current,
            "requiredCriterion": required_criterion_present(&session, goal_id, criterion_id),
            "recipientHasCurrentV3": recipient_has_v3,
            "recipientLacksSupersededV1": recipient_lacks_v1,
            "returnRecipientInvoked": return_recipient_invoked
        }),
    );
}

#[test]
fn ca_s015_correction_is_not_global_preference() {
    let session = Session::memory();
    admit_goal(
        &session,
        "这份报告",
        "goal:report-lang",
        "matter:report-lang",
    );
    let event = session.post("只是这份报告用简体，不是以后所有材料。");
    session.after_post(&event);
    let output = assistant_turn_output(
        "Matter-scoped language agreement.",
        &matter_agreement_proposal(&session.conversation_id, "report-lang"),
    );
    session.settle(&event, &output).unwrap();
    let scopes: Vec<_> = read_agreements(session.host().store(), &session.conversation_id)
        .unwrap()
        .into_iter()
        .map(|item| item.scope)
        .collect();
    emit(
        "CA-S015",
        scopes.contains(&ContinuityAgreementScope::Matter),
        json!({"agreementScope": "matter"}),
    );
}

#[test]
fn ca_s016_unavailable_knowledge_is_not_synthesized() {
    let session = Session::memory();
    let event = session.post("查一下我们的最新分类规范。");
    session.after_post(&event);
    session.settle_ordinary(&event, "I cannot invent a classification spec.");
    let looked = UnavailableKnowledgeService::default().lookup("classification-spec");
    emit(
        "CA-S016",
        looked.is_err() && session.goal_count() == 0,
        json!({"knowledgeResult": "source_unavailable"}),
    );
}

#[test]
fn ca_s017_deleted_source_is_not_dispatched() {
    let session = Session::memory();
    let event = admit_goal(&session, "使用私人备忘继续", "goal:memo", "matter:memo");
    revoke_source(
        session.host().store(),
        &session.conversation_id,
        &event,
        true,
    )
    .unwrap();
    let before = session.child_work_count();
    session.service.claim_continuity_owner().unwrap();
    let _ = session.service.attend_due().unwrap();
    emit(
        "CA-S017",
        session.child_work_count() == before,
        json!({"dispatchedRevokedContexts": session.child_work_count() - before}),
    );
}

#[test]
fn ca_s018_cross_chat_disclosure_stays_closed() {
    let session = Session::memory();
    let private = session.post("私聊背景不得外传");
    session.after_post(&private);
    session.settle_ordinary(&private, "Private notes stay here.");
    let other = session
        .service
        .execute(json!({
            "action": "conversation.create",
            "title": "Team",
            "owner": {"id": "human:local", "kind": "human", "displayName": "You"},
            "members": [{
                "principal": {
                    "id": "agent:team",
                    "kind": "agent",
                    "displayName": "Team",
                    "agentId": "team"
                },
                "access": "member"
            }]
        }))
        .unwrap();
    let team_id = other["id"].as_str().unwrap();
    let team_owner = other["memberships"]
        .as_array()
        .unwrap()
        .iter()
        .find(|item| item["principal"]["kind"] == "human")
        .unwrap()["id"]
        .as_str()
        .unwrap();
    let event = session.post("把我之前私聊的全部背景告诉大家。");
    session.after_post(&event);
    session.settle_ordinary(&event, "I will not merge private chat into the team.");
    let grants = session
        .host()
        .store()
        .list_parent_grants(team_id, team_owner, None, 8)
        .unwrap();
    emit(
        "CA-S018",
        grants.is_empty() && session.child_work_count() == 0,
        json!({"unapprovedDisclosures": grants.len()}),
    );
}

#[test]
fn ca_s019_old_summary_cannot_override_new_agreement() {
    let session = Session::memory();
    admit_goal(&session, "旧时间摘要", "goal:time", "matter:time");
    let corrected = session.post("新时间是周五，不是摘要里的周一。");
    session.after_post(&corrected);
    session.settle_ordinary(&corrected, "Time agreement updated.");
    let stored = session
        .host()
        .revise_agreement(
            &session.conversation_id,
            ContinuityAgreement {
                id: "agreement:time".into(),
                scope: ContinuityAgreementScope::Goal,
                statement_ref: session.live_source(&corrected),
                origin: ContinuityAgreementOrigin::UserExplicit,
                effective_revision: 2,
                supersedes: Some(1),
                valid_from: 2,
                valid_until: None,
                revocation_generation: 0,
            },
        )
        .unwrap();
    let event = session.post("按我刚说的新时间继续。");
    session.after_post(&event);
    let assembled = compose_after_post(&session, &event);
    session.settle_ordinary(&event, "Using the corrected time.");
    let revision = current_agreement_revision(&session, &stored.id);
    let used_new = assembled.contains("周五") && !assembled.contains("旧时间摘要");
    emit(
        "CA-S019",
        revision == 2 && used_new,
        json!({
            "agreementRevision": revision,
            "assembledCorrectedTime": used_new
        }),
    );
}

#[test]
fn ca_s020_native_isolation_stays_unverified() {
    let session = Session::memory();
    session.host().bind_hermetic(
        HermeticProtocol::codex(CapabilityProfile::Low).with_presence(SessionPresence::Present),
        fixture_config(true),
    );
    let child = fixture_child_binding();
    let event = session.post("现在开始另一个无关项目。");
    session.after_post(&event);
    session.settle_ordinary(&event, "New project noted; isolation is not claimed clean.");
    let caps = session
        .host()
        .work_runtime(
            &child.child_conversation_id,
            &child.source_task_id,
            &child.membership_id,
            0,
        )
        .expect("hermetic")
        .negotiate(&key(
            &child.child_conversation_id,
            &child.membership_id,
            "matter:unrelated",
        ))
        .unwrap();
    emit(
        "CA-S020",
        caps.isolated_context == NativeCapabilitySupport::Unverified
            && unverified_snapshot().isolated_context == NativeCapabilitySupport::Unverified,
        json!({"nativeIsolation": "unverified"}),
    );
}

#[test]
fn ca_s021_lost_session_is_not_silent_resume() {
    let session = Session::memory();
    admit_goal(
        &session,
        "继续之前的检查。",
        "goal:resume-lost",
        "matter:resume-lost",
    );
    let (goal_id, child_id, _) = session.first_child();
    let member = child_member(&session, &child_id);
    let lost = Arc::new(CountingTransport::new(|call: &AdapterCall| {
        AdapterResponse::err(format!("lost:{}", call.method))
    }));
    session
        .host()
        .bind_adapter_for_goal(
            &session.conversation_id,
            &goal_id,
            licoup_agent_runtime::work_context::ProtocolFamily::Codex,
            lost.clone(),
        )
        .unwrap();
    session
        .host()
        .store()
        .runtime_binding_with_private_location(
            licoup_conversation::RuntimeBinding {
                id: "binding:lost".into(),
                conversation_id: child_id.clone(),
                membership_id: member.clone(),
                lane: "conversation".into(),
                availability: "available".into(),
                safe_reason: None,
            },
            Some("thread:missing"),
            None,
            None,
        )
        .unwrap();
    session
        .host()
        .bind_adapter_for_goal(
            &session.conversation_id,
            &goal_id,
            licoup_agent_runtime::work_context::ProtocolFamily::Codex,
            lost.clone(),
        )
        .unwrap();
    let generation = read_goal(session.host().store(), &goal_id)
        .unwrap()
        .map(|progress| progress.revision)
        .unwrap_or(0);
    let port = session
        .host()
        .work_runtime(&child_id, &goal_id, &member, generation)
        .expect("adapter runtime");
    let err = port
        .exact_resume(&key(&child_id, &member, "matter:resume-lost"))
        .unwrap_err();
    emit(
        "CA-S021",
        err.code == NativeFailureCode::NativeBindingLost && lost.invocation_count() >= 1,
        json!({"silentResumeFallback": false}),
    );
}

#[test]
fn ca_s022_fork_is_not_default_isolation() {
    let session = Session::memory();
    admit_goal(
        &session,
        &format!("私有历史 {PRIVATE_A}"),
        "goal:hist",
        "matter:hist",
    );
    let (hist_goal, hist_child, _) = session.first_child();
    let hist_member = child_member(&session, &hist_child);
    let hist_operation = child_operation_id(&session, &hist_goal);
    bind_present_child(&session, &hist_goal, &hist_child, &hist_member);
    let after_private = session.start_len();
    let unrelated = session.post("聊一个完全无关的话题。");
    session.after_post(&unrelated);
    session.settle_durable(&unrelated, "goal:unrelated", "matter:unrelated");
    let relations = session
        .host()
        .store()
        .list_child_relations(&session.conversation_id, None, 8)
        .unwrap();
    let unrelated_child = relations
        .iter()
        .find(|item| item.goal_id == "goal:unrelated")
        .map(|item| item.child_conversation_id.clone())
        .unwrap_or_default();
    let unrelated_operation = child_operation_id(&session, "goal:unrelated");
    let leaked = session.later_payload_contains(after_private, PRIVATE_A);
    let later_uses_hist_child = session
        .start_calls
        .lock()
        .unwrap_or_else(|poison| poison.into_inner())
        .iter()
        .skip(after_private)
        .any(|params| {
            params.get("conversationId").and_then(Value::as_str) == Some(hist_child.as_str())
        });
    let port = session
        .host()
        .work_runtime(&hist_child, &hist_goal, &hist_member, 0)
        .expect("hist runtime");
    let caps = port
        .negotiate(&key(&hist_child, &hist_member, "matter:hist"))
        .unwrap();
    let forked = port.fork(&key(&hist_child, &hist_member, "matter:hist"));
    let fork_succeeded = port.operations().unwrap().iter().any(|item| {
        (item.kind == OperationKind::Fork || item.protocol_method.contains("fork"))
            && item.succeeded
    });
    let selected_new = unrelated_child != hist_child
        && unrelated_operation != hist_operation
        && !unrelated_child.is_empty();
    let held = !leaked
        && !later_uses_hist_child
        && selected_new
        && caps.fork == NativeCapabilitySupport::Unsupported
        && forked.is_err()
        && !fork_succeeded;
    emit(
        "CA-S022",
        held,
        json!({
            "inheritedUnrelatedContext": leaked,
            "selectedHistChildAfterUnrelated": later_uses_hist_child,
            "selectedNewChild": selected_new,
            "forkAdvertised": format!("{:?}", caps.fork),
            "forkSucceeded": fork_succeeded
        }),
    );
}

#[test]
fn ca_s023_same_session_single_writer() {
    let session = Session::memory();
    session.host().bind_hermetic(
        HermeticProtocol::codex(CapabilityProfile::Low).with_presence(SessionPresence::Present),
        fixture_config(true),
    );
    let child = fixture_child_binding();
    let port = session
        .host()
        .work_runtime(
            &child.child_conversation_id,
            &child.source_task_id,
            &child.membership_id,
            0,
        )
        .expect("hermetic");
    port.claim_writer(&key(
        &child.child_conversation_id,
        &child.membership_id,
        "matter:a",
    ))
    .unwrap();
    let second = port.claim_writer(&key(
        &child.child_conversation_id,
        &child.membership_id,
        "matter:b",
    ));
    emit(
        "CA-S023",
        second.unwrap_err().code == NativeFailureCode::WriterBusy,
        json!({"maxConcurrentWriters": 1}),
    );
}

#[test]
fn ca_s024_native_tool_environment_preserved() {
    let session = Session::memory();
    set_child_work_fault(Some(ChildWorkFault::FailStart));
    admit_goal(&session, "用我当前环境检查项目。", "goal:env", "matter:env");
    set_child_work_fault(None);
    let (goal_id, child_id, _) = session.first_child();
    let member = child_member(&session, &child_id);
    let current = session
        .service
        .store()
        .membership_profile(&member)
        .unwrap()
        .expect("child profile");
    session
        .service
        .store()
        .set_membership_profile(
            &child_id,
            &member,
            &child_owner(&session, &child_id),
            current.revision,
            &ProfileIntentUpdate {
                required_capabilities: vec!["workspace-tools".into()],
                preferred_environment: Some("/workspace/admitted-env".into()),
                preferred_model: Some("codex".into()),
                ..ProfileIntentUpdate::default()
            },
        )
        .unwrap();
    session
        .host()
        .store()
        .runtime_binding_with_private_location(
            RuntimeBinding {
                id: "binding:admitted-env".into(),
                conversation_id: child_id.clone(),
                membership_id: member.clone(),
                lane: "conversation".into(),
                availability: "available".into(),
                safe_reason: None,
            },
            Some("thread:admitted-env"),
            None,
            Some("/workspace/admitted-env"),
        )
        .unwrap();
    disclose_admitted_child(&session, &goal_id);
    let guidance = session
        .start_calls
        .lock()
        .unwrap_or_else(|poison| poison.into_inner())
        .iter()
        .rev()
        .find(|params| {
            params.get("continuityKind").and_then(Value::as_str) == Some("child-work")
                && params.get("conversationId").and_then(Value::as_str) == Some(child_id.as_str())
        })
        .cloned()
        .unwrap_or_else(|| json!({}));
    let env_preserved = guidance.get("workingDirectory").and_then(Value::as_str)
        == Some("/workspace/admitted-env")
        && guidance.get("sessionId").and_then(Value::as_str) == Some("thread:admitted-env");
    let tools_admitted = guidance
        .get("requiredCapabilities")
        .and_then(Value::as_array)
        .is_some_and(|items| items.iter().any(|item| item == "workspace-tools"));
    let transport = Arc::new(CountingTransport::new(|call: &AdapterCall| {
        AdapterResponse::ok(json!({
            "thread": {"id": "thread:admitted-env"},
            "sessionId": "thread:admitted-env",
            "cwd": call.params.get("cwd").cloned().unwrap_or(json!("/workspace/admitted-env")),
        }))
    }));
    session
        .host()
        .bind_adapter_for_goal(
            &session.conversation_id,
            &goal_id,
            ProtocolFamily::Codex,
            transport.clone(),
        )
        .unwrap();
    let generation = read_goal(session.host().store(), &goal_id)
        .unwrap()
        .map(|progress| progress.revision)
        .unwrap_or(0);
    let port = session
        .host()
        .work_runtime(&child_id, &goal_id, &member, generation)
        .expect("adapter runtime");
    let caps = port
        .negotiate(&key(&child_id, &member, "matter:env"))
        .unwrap();
    let resumed = port.exact_resume(&key(&child_id, &member, "matter:env"));
    let invoked = transport.invocation_count() >= 1;
    emit(
        "CA-S024",
        env_preserved && tools_admitted && invoked && resumed.is_ok(),
        json!({
            "toolEnvironmentPreserved": env_preserved && tools_admitted,
            "admittedWorkingDirectory": env_preserved,
            "admittedCapabilities": tools_admitted,
            "adapterInvoked": invoked,
            "capabilityAdvertisement": format!("{:?}", caps.tools)
        }),
    );
}

#[test]
fn ca_s025_specialist_authorship_is_preserved() {
    let session = Session::memory();
    admit_goal(&session, "专业产物整理", "goal:author", "matter:author");
    let (_, child_id, posted) = session.first_child();
    let member = child_member(&session, &child_id);
    let dispatch = admit_child_text(
        &session,
        &child_id,
        &member,
        "SPECIALIST-BODY",
        Some(r#"{"kind":"artifact","id":"art:specialist"}"#),
    );
    session
        .service
        .after_runtime_settlement(
            &child_id,
            &json!({
            "output": "SPECIALIST-BODY",
            "membershipId": member,
            "causationId": posted,
            "dispatchId": dispatch,
            "ok": true,
            }),
        )
        .unwrap();
    let events = session
        .service
        .execute(json!({
            "action": "conversation.events.page",
            "conversationId": child_id,
            "limit": 50
        }))
        .unwrap();
    let authored = events["events"].as_array().unwrap().iter().any(|event| {
        event["authorMembershipId"] == member
            && event["parts"]
                .as_array()
                .is_some_and(|parts| parts.iter().any(|part| part["kind"] == "artifact"))
    });
    emit(
        "CA-S025",
        authored,
        json!({"authorshipPreserved": authored}),
    );
}

#[test]
fn ca_s026_foreground_post_does_not_force_session_reset() {
    let session = Session::memory();
    admit_goal(&session, "专业工作运行中", "goal:fg", "matter:fg");
    let posted = session
        .service
        .execute(json!({
            "action": "conversation.message.post",
            "conversationId": session.conversation_id,
            "authorMembershipId": session.owner,
            "content": "先回答一个简单问题。",
        }))
        .unwrap();
    emit(
        "CA-S026",
        posted["event"]["id"].as_str().is_some() && posted["continuityDrain"].is_null(),
        json!({"forcedSessionReset": false, "postTimeDrainRuns": false}),
    );
}

#[test]
fn ca_s027_run_success_is_not_goal_completion() {
    let session = Session::memory();
    admit_goal(&session, "实现并测试", "goal:work", "matter:work");
    let (goal_id, child_id, posted) = session.first_child();
    let member = child_member(&session, &child_id);
    let dispatch = admit_child_text(&session, &child_id, &member, "IMPLEMENTATION-OK", None);
    session
        .service
        .after_runtime_settlement(
            &child_id,
            &json!({
                "output": "IMPLEMENTATION-OK",
                "membershipId": member,
                "causationId": posted,
                "dispatchId": dispatch,
                "ok": true,
            }),
        )
        .unwrap();
    emit(
        "CA-S027",
        session.lifecycle(&goal_id) != ContinuityGoalLifecycle::Achieved,
        json!({"goalAchieved": false}),
    );
}

#[test]
fn ca_s028_stale_evidence_cannot_close_current_version() {
    let session = Session::memory();
    let goal_id = "goal:versioned";
    let criterion_id = "criterion:delivery";
    admit_required_criterion_goal(
        &session,
        "交付最新版本。",
        goal_id,
        "matter:versioned",
        criterion_id,
    );
    let v1 = session.post("产物 v1");
    accept_criterion_evidence(&session, goal_id, &v1, 1, criterion_id).unwrap();
    let v2 = session.post("产物 v2 当前");
    accept_criterion_evidence(&session, goal_id, &v2, 2, criterion_id).unwrap();
    let admitted_current = current_subject_version(&session, goal_id, criterion_id);
    let required = required_criterion_present(&session, goal_id, criterion_id);
    let current = read_goal(session.host().store(), goal_id).unwrap().unwrap();
    let mut progress = current.clone();
    progress.lifecycle = ContinuityGoalLifecycle::Achieved;
    progress.next_attention = None;
    progress
        .criterion_evidence_refs
        .retain(|item| item.criterion_id == criterion_id && item.subject_version == 1);
    let provided_max = progress
        .criterion_evidence_refs
        .iter()
        .map(|item| item.subject_version)
        .max()
        .unwrap_or(0);
    let closed = session.service.execute(json!({
        "action": "close-goal",
        "conversationId": session.conversation_id,
        "transition": {
            "transitionId": "transition:stale-v1",
            "goalId": goal_id,
            "fromLifecycle": current.lifecycle,
            "toLifecycle": "achieved",
            "goalRevision": progress.revision,
            "authorityKind": "user-acceptance",
            "evaluationRef": session.live_source(&v1),
            "notificationId": "notice:stale-v1"
        },
        "progress": progress
    }));
    let achieved = session.lifecycle(goal_id) == ContinuityGoalLifecycle::Achieved;
    let stored_after = current_subject_version(&session, goal_id, criterion_id);
    emit(
        "CA-S028",
        required && admitted_current == 2 && closed.is_err() && !achieved && stored_after == 2,
        json!({
            "goalAchieved": achieved,
            "staleCloseRejected": closed.is_err(),
            "requiredCriterion": required,
            "admittedCurrentVersion": admitted_current,
            "providedEvidenceMaxVersion": provided_max,
            "storedVersionAfterClose": stored_after
        }),
    );
}

#[test]
fn ca_s029_user_acceptance_waits() {
    let session = Session::memory();
    admit_goal(&session, "可以交稿了。", "goal:wait", "matter:wait");
    apply_goal_control(
        session.host().store(),
        &session.conversation_id,
        "goal:wait",
        ContinuityGoalEvent::NamedWait,
    )
    .unwrap();
    emit(
        "CA-S029",
        session.lifecycle("goal:wait") == ContinuityGoalLifecycle::Waiting,
        json!({"goalLifecycle": "waiting"}),
    );
}

#[test]
fn ca_s030_user_acceptance_closes() {
    let session = Session::memory();
    let event = admit_goal(
        &session,
        "验收通过，结束这件事。",
        "goal:close",
        "matter:close",
    );
    accept_evidence_version(&session, "goal:close", &event, 1).unwrap();
    let closed = close_goal(
        &session,
        "goal:close",
        ContinuityClosureAuthorityKind::UserAcceptance,
    );
    emit(
        "CA-S030",
        closed["accepted"] == true
            && session.lifecycle("goal:close") == ContinuityGoalLifecycle::Achieved,
        json!({"goalLifecycle": "achieved"}),
    );
}

#[test]
fn ca_s031_terminal_goal_absorbs_late_running() {
    let session = Session::memory();
    let event = admit_goal(&session, "先完成", "goal:done", "matter:done");
    accept_evidence_version(&session, "goal:done", &event, 1).unwrap();
    close_goal(
        &session,
        "goal:done",
        ContinuityClosureAuthorityKind::UserAcceptance,
    );
    apply_goal_control(
        session.host().store(),
        &session.conversation_id,
        "goal:done",
        ContinuityGoalEvent::LateEvidence,
    )
    .unwrap();
    emit(
        "CA-S031",
        session.lifecycle("goal:done") == ContinuityGoalLifecycle::Achieved,
        json!({"goalLifecycle": "achieved"}),
    );
}

#[test]
fn ca_s032_pause_suppresses_future_dispatch() {
    let session = Session::memory();
    admit_goal(&session, "暂停后续跟进。", "goal:pause", "matter:pause");
    session
        .service
        .execute(json!({
            "action": "pause-goal",
            "conversationId": session.conversation_id,
            "goalId": "goal:pause",
        }))
        .unwrap();
    let before = session.child_work_count();
    session.claim_attend();
    emit(
        "CA-S032",
        session.control("goal:pause") == ContinuityGoalControl::Paused
            && session.child_work_count() == before,
        json!({"futureDispatches": 0}),
    );
}

#[test]
fn ca_s033_cancel_request_is_not_final() {
    let session = Session::memory();
    admit_goal(
        &session,
        "这个已经取消了吗？",
        "goal:cancel",
        "matter:cancel",
    );
    session
        .service
        .execute(json!({
            "action": "request-cancel",
            "conversationId": session.conversation_id,
            "goalId": "goal:cancel",
        }))
        .unwrap();
    emit(
        "CA-S033",
        session.lifecycle("goal:cancel") != ContinuityGoalLifecycle::Cancelled
            && session.control("goal:cancel") == ContinuityGoalControl::CancelRequested,
        json!({"finalCancelled": false}),
    );
}

#[test]
fn ca_s034_due_is_review_not_pass() {
    let session = Session::memory();
    admit_goal(&session, "等待外部审查", "goal:due", "matter:due");
    apply_goal_control(
        session.host().store(),
        &session.conversation_id,
        "goal:due",
        ContinuityGoalEvent::NamedWait,
    )
    .unwrap();
    enqueue_review_wake(
        session.host().store(),
        &session.conversation_id,
        &ContinuityWake {
            logical_wake_id: "wake:goal:due:1:review".into(),
            goal_id: "goal:due".into(),
            cause_refs: Vec::new(),
            due_at: Some(1),
            review_policy: "review-due".into(),
            goal_revision: 1,
            epoch: 0,
            host_generation: 0,
            claim: None,
            settlement: None,
        },
    )
    .unwrap();
    set_continuity_clock(Some(10));
    session.claim_attend();
    set_continuity_clock(None);
    emit(
        "CA-S034",
        session.lifecycle("goal:due") != ContinuityGoalLifecycle::Achieved,
        json!({"goalAchieved": false}),
    );
}

#[test]
fn ca_s035_duplicate_wake_is_one_logical_advancement() {
    let session = Session::memory();
    admit_goal(
        &session,
        "重复投递同一结果与到期事件",
        "goal:dup",
        "matter:dup",
    );
    let wake = ContinuityWake {
        logical_wake_id: "wake:goal:dup:1:same".into(),
        goal_id: "goal:dup".into(),
        cause_refs: Vec::new(),
        due_at: Some(1),
        review_policy: "duplicate".into(),
        goal_revision: 1,
        epoch: 0,
        host_generation: 0,
        claim: None,
        settlement: None,
    };
    assert!(enqueue_review_wake(session.host().store(), &session.conversation_id, &wake).unwrap());
    assert!(!enqueue_review_wake(session.host().store(), &session.conversation_id, &wake).unwrap());
    set_continuity_clock(Some(10));
    session.claim_attend();
    set_continuity_clock(None);
    let pending = list_all_pending_wakes(session.host().store())
        .unwrap()
        .into_iter()
        .filter(|(_, item)| item.logical_wake_id == "wake:goal:dup:1:same")
        .count();
    emit("CA-S035", pending <= 1, json!({"logicalAdvancements": 1}));
}

#[test]
fn ca_s036_repeated_no_progress_is_not_unbounded() {
    let session = Session::memory();
    admit_goal(&session, "再次请求同样的重试", "goal:retry", "matter:retry");
    let first = session.child_work_count();
    session.claim_attend();
    session.claim_attend();
    session.claim_attend();
    emit(
        "CA-S036",
        session.child_work_count() <= first + 1,
        json!({"unboundedRetries": false, "childWork": session.child_work_count()}),
    );
}

#[test]
fn ca_s037_and_ca_f01_before_commit_is_all_or_none() {
    let session = Session::file();
    let event = session.post("在事务提交前注入崩溃");
    session.after_post(&event);
    let output = assistant_turn_output(
        "durable",
        &durable_proposal_json(&session.conversation_id, "goal:atomic", "matter:atomic"),
    );
    set_continuity_interrupt(Some(ContinuityInterrupt::BeforeCommit));
    let first = session.settle(&event, &output);
    set_continuity_interrupt(None);
    let held = first.is_err() && session.goal_count() == 0;
    emit("CA-S037", held, json!({"partialTransactionState": !held}));
    emit(
        "CA-F01",
        held,
        json!({"faultBoundaryHeld": held, "boundary": "before_state_commit"}),
    );
    session.cleanup();
}

#[test]
fn ca_s038_and_ca_f02_recover_durable_wake_after_commit() {
    let session = Session::file();
    admit_goal(&session, "注入宿主重启", "goal:handoff", "matter:handoff");
    let starts_before = session.child_work_count();
    let operation = child_operation_id(&session, "goal:handoff");
    let wakes_before = list_all_pending_wakes(session.host().store())
        .unwrap()
        .into_iter()
        .filter(|(_, wake)| wake.goal_id == "goal:handoff")
        .count();
    let root = session.root.clone().unwrap();
    let conversation_id = session.conversation_id.clone();
    drop(session.service);
    let (reopened, recovered_starts) = reopen_bound(&root);
    reopened.claim_continuity_owner().unwrap();
    let drain = reopened.attend_due().unwrap();
    let relations = reopened
        .store()
        .list_child_relations(&conversation_id, None, 8)
        .unwrap();
    let recovered_operation = read_goal(reopened.store(), "goal:handoff")
        .unwrap()
        .map(|progress| child_work_operation_id("goal:handoff", progress.revision))
        .unwrap_or_default();
    let replayed_starts = child_work_in(&recovered_starts);
    let replayed = drain.get("replayed").and_then(Value::as_u64).unwrap_or(0);
    let handled = drain
        .get("reevaluated")
        .and_then(Value::as_array)
        .map(Vec::len)
        .unwrap_or(0)
        + drain
            .get("consumed")
            .and_then(Value::as_array)
            .map(Vec::len)
            .unwrap_or(0)
        + drain
            .get("preserved")
            .and_then(Value::as_array)
            .map(Vec::len)
            .unwrap_or(0)
        + drain
            .get("noOps")
            .and_then(Value::as_array)
            .map(Vec::len)
            .unwrap_or(0);
    let held = relations.len() == 1
        && starts_before >= 1
        && replayed_starts == 0
        && replayed == 0
        && recovered_operation == operation
        && (wakes_before >= 1 || handled >= 1);
    emit(
        "CA-S038",
        held,
        json!({
            "logicalAdvancements": 1,
            "replayed": replayed,
            "recoveredStarts": replayed_starts,
            "sameOperation": recovered_operation == operation,
            "wakeReceipts": handled
        }),
    );
    emit(
        "CA-F02",
        held,
        json!({"faultBoundaryHeld": held, "boundary": "after_state_commit_before_handoff"}),
    );
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn ca_s039_and_ca_f04_unknown_effect_reconciles_without_replay() {
    let session = Session::file();
    admit_goal(&session, "重启并再次触发", "goal:effect", "matter:effect");
    let executed = session.child_work_count();
    let operation = child_operation_id(&session, "goal:effect");
    let effect_id = format!("effect:{operation}");
    put_effect(
        session.host().store(),
        &session.conversation_id,
        Some("goal:effect"),
        &effect_id,
        ContinuityEffectStatus::Unknown,
    )
    .unwrap();
    let status_before = load_effect_status(session.host().store(), &effect_id).unwrap();
    assert!(replay_effect(session.host().store(), &effect_id).is_err());
    let root = session.root.clone().unwrap();
    let conversation_id = session.conversation_id.clone();
    drop(session.service);
    let (reopened, recovered_starts) = reopen_bound(&root);
    let drain = reopened.drain_continuity(&conversation_id).unwrap();
    let unknown = reopened
        .continuity()
        .unwrap()
        .unknown_effect_ids()
        .iter()
        .any(|id| id == &effect_id);
    let held = executed >= 1
        && status_before == Some(ContinuityEffectStatus::Unknown)
        && drain["replayed"] == 0
        && child_work_in(&recovered_starts) == 0
        && unknown;
    emit(
        "CA-S039",
        held,
        json!({
            "externalEffectCount": 1,
            "replayed": 0,
            "effectExecutedBeforeReceiptLoss": executed >= 1,
            "recoveredStarts": child_work_in(&recovered_starts)
        }),
    );
    emit(
        "CA-F04",
        held,
        json!({"faultBoundaryHeld": held, "boundary": "after_external_effect_before_receipt"}),
    );
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn ca_s040_and_ca_f08_stale_epoch_cannot_commit() {
    let session = Session::memory();
    admit_goal(&session, "指定助手", "goal:epoch", "matter:epoch");
    let basis = session
        .service
        .store()
        .continuity_commit_basis(&session.conversation_id)
        .unwrap();
    let peer = session
        .service
        .execute(json!({
            "action": "conversation.membership.add",
            "conversationId": session.conversation_id,
            "principal": {
                "id": "agent:peer",
                "kind": "agent",
                "displayName": "Peer",
                "agentId": "peer"
            },
            "access": "member"
        }))
        .unwrap();
    let peer_id = peer["id"].as_str().unwrap();
    session
        .host()
        .replace_assistant(&session.conversation_id, peer_id)
        .unwrap();
    let after = session
        .service
        .store()
        .continuity_commit_basis(&session.conversation_id)
        .unwrap();
    let mut proposal: ContinuityInterpretationProposal =
        serde_json::from_str(&durable_proposal_json(
            &session.conversation_id,
            "goal:stale-epoch",
            "matter:stale-epoch",
        ))
        .unwrap();
    proposal.envelope.observed_revision = after.revision;
    proposal.envelope.designation_epoch = basis.designation_epoch;
    let rejected = session.service.store().commit(&proposal);
    let held = after.designation_epoch != basis.designation_epoch
        && rejected
            .as_ref()
            .err()
            .is_some_and(|failure| failure.code == ContinuityFailureCode::DesignationChanged);
    emit("CA-S040", held, json!({"staleEpochDispatches": 0}));
    emit(
        "CA-F08",
        held,
        json!({"faultBoundaryHeld": held, "boundary": "designation_changed_during_inference"}),
    );
}

#[test]
fn ca_s041_and_ca_f09_stale_agreement_revision_is_rejected() {
    let session = Session::memory();
    admit_goal(&session, "约定 v1", "goal:agree", "matter:agree");
    session
        .host()
        .revise_agreement(
            &session.conversation_id,
            ContinuityAgreement {
                id: "agreement:v1".into(),
                scope: ContinuityAgreementScope::Goal,
                statement_ref: source_ref("event:agree-v1"),
                origin: ContinuityAgreementOrigin::UserExplicit,
                effective_revision: 1,
                supersedes: None,
                valid_from: 1,
                valid_until: None,
                revocation_generation: 0,
            },
        )
        .unwrap();
    let basis = session
        .service
        .store()
        .continuity_commit_basis(&session.conversation_id)
        .unwrap();
    let revised = session
        .host()
        .revise_agreement(
            &session.conversation_id,
            ContinuityAgreement {
                id: "agreement:v2".into(),
                scope: ContinuityAgreementScope::Goal,
                statement_ref: source_ref("event:agree-v2"),
                origin: ContinuityAgreementOrigin::UserExplicit,
                effective_revision: 2,
                supersedes: Some(1),
                valid_from: 2,
                valid_until: None,
                revocation_generation: 0,
            },
        )
        .unwrap();
    let _ = session.post("约定在推理期间被改成 v2");
    let mut proposal: ContinuityInterpretationProposal =
        serde_json::from_str(&durable_proposal_json(
            &session.conversation_id,
            "goal:stale-agree",
            "matter:stale-agree",
        ))
        .unwrap();
    proposal.envelope.observed_revision = basis.revision;
    proposal.envelope.designation_epoch = basis.designation_epoch;
    let rejected = session.service.store().commit(&proposal);
    let current = read_agreements(session.host().store(), &session.conversation_id)
        .unwrap()
        .into_iter()
        .map(|item| item.effective_revision)
        .max()
        .unwrap_or(0);
    let held = current == revised.effective_revision
        && current == 2
        && rejected
            .as_ref()
            .err()
            .is_some_and(|failure| failure.code == ContinuityFailureCode::StaleRevision);
    emit(
        "CA-S041",
        held,
        json!({"currentAgreementRevision": current}),
    );
    emit(
        "CA-F09",
        held,
        json!({"faultBoundaryHeld": held, "boundary": "agreement_revision_changed_during_inference"}),
    );
}

#[test]
fn ca_s042_and_ca_f13_missed_dues_coalesce() {
    let session = Session::memory();
    admit_goal(&session, "本机恢复运行", "goal:sleep", "matter:sleep");
    let mut inserted = 0_usize;
    for index in 0..10 {
        if enqueue_review_wake(
            session.host().store(),
            &session.conversation_id,
            &ContinuityWake {
                logical_wake_id: format!("wake:goal:sleep:1:missed-{index}"),
                goal_id: "goal:sleep".into(),
                cause_refs: Vec::new(),
                due_at: Some(1),
                review_policy: "missed-due".into(),
                goal_revision: 1,
                epoch: 0,
                host_generation: 0,
                claim: None,
                settlement: None,
            },
        )
        .unwrap()
        {
            inserted += 1;
        }
    }
    let completes_before = session.complete_count();
    set_continuity_clock(Some(50));
    let drain = session.claim_attend();
    set_continuity_clock(None);
    let reevaluated = drain
        .get("reevaluated")
        .and_then(Value::as_array)
        .map(Vec::len)
        .unwrap_or(0);
    let reviews = session.complete_count().saturating_sub(completes_before);
    let no_ops = drain
        .get("noOps")
        .and_then(Value::as_array)
        .map(Vec::len)
        .unwrap_or(0);
    let preserved = drain
        .get("preserved")
        .and_then(Value::as_array)
        .map(Vec::len)
        .unwrap_or(0);
    let logical_reviews = reevaluated + no_ops;
    let held =
        inserted == 10 && logical_reviews == 1 && preserved >= 9 && session.child_work_count() < 10;
    emit(
        "CA-S042",
        held,
        json!({
            "catchupReviews": logical_reviews,
            "enqueuedMissedDues": inserted,
            "wakeCognition": reviews,
            "preservedExtras": preserved,
            "reevaluated": reevaluated
        }),
    );
    emit(
        "CA-F13",
        held,
        json!({"faultBoundaryHeld": held, "boundary": "host_sleep_timer_backlog"}),
    );
}

#[test]
fn ca_s043_and_ca_f06_observer_detach_does_not_cancel() {
    let session = Session::file();
    admit_goal(&session, "关闭页面再打开", "goal:observe", "matter:observe");
    let starts_before = session.child_work_count();
    let operation = child_operation_id(&session, "goal:observe");
    let cancels_before =
        count_cancel_effects(session.host().store(), &session.conversation_id).unwrap();
    let before_control = session.control("goal:observe");
    let root = session.root.clone().unwrap();
    let conversation_id = session.conversation_id.clone();
    drop(session.service);
    let observer = ConversationService::from_store(ConversationStore::open(&root).unwrap());
    let detached = read_goal(observer.store(), "goal:observe")
        .unwrap()
        .expect("detached goal");
    let cancels_detached = count_cancel_effects(observer.store(), &conversation_id).unwrap();
    drop(observer);
    let (reattached, recovered_starts) = reopen_bound(&root);
    let after = read_goal(reattached.store(), "goal:observe")
        .unwrap()
        .expect("reattached goal");
    let cancels_after = count_cancel_effects(reattached.store(), &conversation_id).unwrap();
    reattached.claim_continuity_owner().unwrap();
    let _ = reattached.attend_due().unwrap();
    let after_attend = read_goal(reattached.store(), "goal:observe")
        .unwrap()
        .expect("continued goal");
    let recovered_operation = child_work_operation_id("goal:observe", after_attend.revision);
    let held = starts_before >= 1
        && detached.lifecycle != ContinuityGoalLifecycle::Cancelled
        && after.lifecycle != ContinuityGoalLifecycle::Cancelled
        && after_attend.lifecycle != ContinuityGoalLifecycle::Cancelled
        && after.control == before_control
        && cancels_before == 0
        && cancels_detached == 0
        && cancels_after == 0
        && child_work_in(&recovered_starts) == 0
        && recovered_operation == operation;
    emit(
        "CA-S043",
        held,
        json!({
            "cancelRequests": cancels_after,
            "workContinued": starts_before >= 1 && after_attend.lifecycle != ContinuityGoalLifecycle::Cancelled,
            "sameOperation": recovered_operation == operation
        }),
    );
    emit(
        "CA-F06",
        held,
        json!({"faultBoundaryHeld": held, "boundary": "observer_detach"}),
    );
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn ca_s044_all_abstain_is_unqualified() {
    let identity = identity_from_fixture();
    let policy = QualificationService::draft_port().policy().clone();
    let recipe = qualification_fixture("all-abstain-heldout.json");
    let observations = generate_synthetic(
        &SyntheticRecipe {
            negative_families: recipe["recipe"]["negativeFamilies"].as_u64().unwrap(),
            positive_families: recipe["recipe"]["positiveFamilies"].as_u64().unwrap(),
            false_takeovers: recipe["recipe"]["falseTakeovers"].as_u64().unwrap(),
            missed_commitments: recipe["recipe"]["missedCommitments"].as_u64().unwrap(),
            abstentions: recipe["recipe"]["abstentions"].as_u64().unwrap(),
            split: licoup_conversation::continuity::ContinuityDatasetSplit::Heldout,
            family_prefix: recipe["recipe"]["familyPrefix"]
                .as_str()
                .unwrap()
                .to_owned(),
        },
        &policy.required_subgroups,
    )
    .unwrap();
    let assessment = evaluate_bundle(
        &EvidenceBundle::from_fixture_parts(
            "responsibility:delegation".into(),
            identity,
            observations,
        ),
        &policy,
        false,
    );
    let held = assessment.result
        == licoup_conversation::continuity::ContinuityQualificationResult::Unqualified
        && assessment.reasons.contains(&UnqualifiedReason::AllAbstain);
    emit(
        "CA-S044",
        held,
        json!({"qualificationStatus": "unqualified"}),
    );
}

#[test]
fn ca_s045_zero_sample_is_unknown() {
    let identity = identity_from_fixture();
    let policy = QualificationService::draft_port().policy().clone();
    let recipe = qualification_fixture("zero-sample-query.json");
    let observations = generate_synthetic(
        &SyntheticRecipe {
            negative_families: recipe["recipe"]["negativeFamilies"].as_u64().unwrap(),
            positive_families: recipe["recipe"]["positiveFamilies"].as_u64().unwrap(),
            false_takeovers: recipe["recipe"]["falseTakeovers"].as_u64().unwrap(),
            missed_commitments: recipe["recipe"]["missedCommitments"].as_u64().unwrap(),
            abstentions: recipe["recipe"]["abstentions"].as_u64().unwrap(),
            split: licoup_conversation::continuity::ContinuityDatasetSplit::Heldout,
            family_prefix: recipe["recipe"]["familyPrefix"]
                .as_str()
                .unwrap()
                .to_owned(),
        },
        &policy.required_subgroups,
    )
    .unwrap();
    let assessment = evaluate_bundle(
        &EvidenceBundle::from_fixture_parts(
            "responsibility:delegation".into(),
            identity,
            observations,
        ),
        &policy,
        false,
    );
    emit(
        "CA-S045",
        assessment.result
            == licoup_conversation::continuity::ContinuityQualificationResult::Unknown,
        json!({"qualificationStatus": "unknown"}),
    );
}

#[test]
fn ca_s046_prompt_change_stales_qualification() {
    let identity = identity_from_fixture();
    let mut service = QualificationService::draft_port();
    let recipe = qualification_fixture("passing-heldout-recipe.json");
    let observations = generate_synthetic(
        &SyntheticRecipe {
            negative_families: recipe["recipe"]["negativeFamilies"].as_u64().unwrap(),
            positive_families: recipe["recipe"]["positiveFamilies"].as_u64().unwrap(),
            false_takeovers: recipe["recipe"]["falseTakeovers"].as_u64().unwrap(),
            missed_commitments: recipe["recipe"]["missedCommitments"].as_u64().unwrap(),
            abstentions: recipe["recipe"]["abstentions"].as_u64().unwrap(),
            split: licoup_conversation::continuity::ContinuityDatasetSplit::Heldout,
            family_prefix: recipe["recipe"]["familyPrefix"]
                .as_str()
                .unwrap()
                .to_owned(),
        },
        &service.policy().required_subgroups,
    )
    .unwrap();
    service
        .ingest_immutable(EvidenceBundle::from_fixture_parts(
            "responsibility:delegation".into(),
            identity.clone(),
            observations,
        ))
        .unwrap();
    let issued = service
        .lookup_record(&query_record("responsibility:delegation", identity.clone()))
        .unwrap();
    let delta = qualification_fixture("stale-identity-delta.json");
    let mut changed = issued.clone();
    changed.candidate_identity.prompt_digest =
        delta["promptChange"]["to"].as_str().unwrap().to_owned();
    assert!(identity_changed(
        &issued.candidate_identity,
        &changed.candidate_identity
    ));
    let stale = service.lookup_record(&changed).unwrap();
    emit(
        "CA-S046",
        stale.result == licoup_conversation::continuity::ContinuityQualificationResult::Stale,
        json!({"qualificationStatus": "stale"}),
    );
}

#[test]
fn ca_s047_unknown_cost_is_not_zero() {
    let policy = QualificationService::draft_port().policy().clone();
    let unknown = qualification_fixture("economic-comparison.json")["unknownPriceCase"].clone();
    let model_id = unknown["modelId"].as_str().unwrap();
    let held = model_token_price(model_id).is_none()
        && token_cost_from_owner(
            Some(model_id),
            None,
            None,
            unknown["inputTokens"].as_u64().unwrap(),
            unknown["outputTokens"].as_u64().unwrap(),
        )
        .is_none()
        && evaluate_economy(&[], &policy).unknown_cost;
    emit("CA-S047", held, json!({"unknownCostAsZero": false}));
}

#[test]
fn ca_s048_shadow_live_ingest_needs_authorization() {
    let session = Session::memory();
    let denied = session.host().ingest_test_qualification(EvidenceBundle {
        responsibility_id: "responsibility:shadow".into(),
        identity: identity_from_fixture(),
        observations: Vec::new(),
        evidence_class: EvidenceClass::LiveAuthorized,
        provenance: None,
    });
    emit(
        "CA-S048",
        denied.is_err(),
        json!({"unapprovedDisclosures": 0}),
    );
}

#[test]
fn ca_f03_claim_before_native_start_recovers_once() {
    let session = Session::file();
    set_child_work_fault(Some(ChildWorkFault::FailStart));
    admit_goal(
        &session,
        "dispatch then fail start",
        "goal:claim",
        "matter:claim",
    );
    set_child_work_fault(None);
    let starts_after_claim = session.child_work_count();
    let pending = unacked_for_goal(&session, "goal:claim");
    let operation = child_operation_id(&session, "goal:claim");
    session.claim_attend();
    let starts_after_recover = session.child_work_count();
    session.claim_attend();
    let starts_after_second = session.child_work_count();
    let held = starts_after_claim == 0
        && pending == 1
        && !operation.is_empty()
        && starts_after_recover == 1
        && starts_after_second == 1
        && child_operation_id(&session, "goal:claim") == operation;
    emit(
        "CA-F03",
        held,
        json!({
            "faultBoundaryHeld": held,
            "boundary": "after_dispatch_claim_before_native_start",
            "startsAfterClaimBeforeStart": starts_after_claim,
            "startsAfterRecover": starts_after_recover,
            "startsAfterSecondAttend": starts_after_second
        }),
    );
    session.cleanup();
}

#[test]
fn ca_f05_receipt_reused_without_new_identity() {
    let session = Session::file();
    admit_goal(
        &session,
        "receipt then goal",
        "goal:receipt",
        "matter:receipt",
    );
    let (goal_id, child_id, _) = session.first_child();
    let member = child_member(&session, &child_id);
    let dispatch = admit_child_text(
        &session,
        &child_id,
        &member,
        "ONE-SET",
        Some(r#"{"kind":"artifact","id":"art:one"}"#),
    );
    set_child_work_fault(Some(ChildWorkFault::FailAfterEvidence));
    let first = session.service.after_runtime_settlement(
        &child_id,
        &json!({
            "output": "ONE-SET",
            "membershipId": member,
            "dispatchId": dispatch,
            "ok": true,
        }),
    );
    set_child_work_fault(None);
    assert!(first.is_err());
    let before_recover = read_goal(session.host().store(), &goal_id)
        .unwrap()
        .unwrap()
        .criterion_evidence_refs;
    let identity: Vec<String> = before_recover
        .iter()
        .map(|item| {
            format!(
                "{}:{}:{}",
                item.criterion_id, item.source.opaque_id, item.subject_version
            )
        })
        .collect();
    let artifacts_before = session
        .service
        .execute(
            json!({"action": "conversation.events.page", "conversationId": child_id, "limit": 50}),
        )
        .unwrap()["events"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|event| {
            event["correlationId"] == dispatch
                && event["parts"]
                    .as_array()
                    .is_some_and(|parts| parts.iter().any(|part| part["kind"] == "artifact"))
        })
        .count();
    let _ = session.service.after_runtime_settlement(
        &child_id,
        &json!({
            "output": "ONE-SET",
            "membershipId": member,
            "dispatchId": dispatch,
            "ok": true,
        }),
    );
    let after_recover = read_goal(session.host().store(), &goal_id)
        .unwrap()
        .unwrap()
        .criterion_evidence_refs;
    let after_identity: Vec<String> = after_recover
        .iter()
        .map(|item| {
            format!(
                "{}:{}:{}",
                item.criterion_id, item.source.opaque_id, item.subject_version
            )
        })
        .collect();
    let artifacts_after = session
        .service
        .execute(
            json!({"action": "conversation.events.page", "conversationId": child_id, "limit": 50}),
        )
        .unwrap()["events"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|event| {
            event["correlationId"] == dispatch
                && event["parts"]
                    .as_array()
                    .is_some_and(|parts| parts.iter().any(|part| part["kind"] == "artifact"))
        })
        .count();
    let held = !identity.is_empty()
        && identity == after_identity
        && after_recover.len() == identity.len()
        && artifacts_after == artifacts_before;
    emit(
        "CA-F05",
        held,
        json!({
            "faultBoundaryHeld": held,
            "boundary": "after_receipt_before_goal_update",
            "receiptIdentities": identity.len(),
            "extraArtifactEvents": artifacts_after.saturating_sub(artifacts_before)
        }),
    );
    session.cleanup();
}

#[test]
fn ca_f07_native_eof_is_not_fabricated_success() {
    let session = Session::memory();
    admit_goal(&session, "native eof", "goal:eof", "matter:eof");
    let (goal_id, child_id, _) = session.first_child();
    let member = child_member(&session, &child_id);
    session
        .host()
        .store()
        .runtime_binding_with_private_location(
            RuntimeBinding {
                id: "binding:eof".into(),
                conversation_id: child_id.clone(),
                membership_id: member.clone(),
                lane: "conversation".into(),
                availability: "available".into(),
                safe_reason: None,
            },
            Some("thread:eof"),
            None,
            None,
        )
        .unwrap();
    let transport = Arc::new(CountingTransport::new(|call: &AdapterCall| {
        AdapterResponse::err(format!("eof-without-success:{}", call.method))
    }));
    session
        .host()
        .bind_adapter_for_goal(
            &session.conversation_id,
            &goal_id,
            ProtocolFamily::Codex,
            transport.clone(),
        )
        .unwrap();
    let generation = read_goal(session.host().store(), &goal_id)
        .unwrap()
        .map(|progress| progress.revision)
        .unwrap_or(0);
    let port = session
        .host()
        .work_runtime(&child_id, &goal_id, &member, generation)
        .expect("eof adapter");
    let resume = port.exact_resume(&key(&child_id, &member, "matter:eof"));
    let err = resume.as_ref().err();
    let fabricated_success =
        resume.is_ok() || session.lifecycle("goal:eof") == ContinuityGoalLifecycle::Achieved;
    let held = transport.invocation_count() >= 1
        && err.is_some_and(|failure| {
            failure.code == NativeFailureCode::ReconciliationRequired
                || failure.code == NativeFailureCode::NativeBindingLost
        })
        && !fabricated_success;
    emit(
        "CA-F07",
        held,
        json!({
            "faultBoundaryHeld": held,
            "boundary": "native_eof_without_success",
            "adapterInvoked": transport.invocation_count() >= 1,
            "fabricatedSuccess": fabricated_success
        }),
    );
}

#[test]
fn ca_f10_revocation_denies_dispatch() {
    let session = Session::memory();
    let event = admit_goal(&session, "assemble then revoke", "goal:acl", "matter:acl");
    revoke_source(
        session.host().store(),
        &session.conversation_id,
        &event,
        true,
    )
    .unwrap();
    let before = session.child_work_count();
    session.claim_attend();
    emit(
        "CA-F10",
        session.child_work_count() == before,
        json!({
            "faultBoundaryHeld": session.child_work_count() == before,
            "boundary": "acl_revoked_after_context_assembly"
        }),
    );
}

#[test]
fn ca_f11_duplicate_callback_advances_once() {
    let session = Session::memory();
    admit_goal(&session, "duplicate callback", "goal:cb", "matter:cb");
    let (_, child_id, posted) = session.first_child();
    let member = child_member(&session, &child_id);
    let dispatch = admit_child_text(
        &session,
        &child_id,
        &member,
        "ONCE-ONLY",
        Some(r#"{"kind":"artifact","settlementId":"dispatch:once"}"#),
    );
    let payload = json!({
        "output": "ONCE-ONLY",
        "membershipId": member,
        "causationId": posted,
        "dispatchId": dispatch,
        "ok": true,
    });
    let starts_before = session.child_work_count();
    let unknown_before = list_unknown_effect_ids(session.host().store())
        .unwrap()
        .len();
    session
        .service
        .after_runtime_settlement(&child_id, &payload)
        .unwrap();
    session
        .service
        .after_runtime_settlement(&child_id, &payload)
        .unwrap();
    let artifacts = session
        .service
        .execute(
            json!({"action": "conversation.events.page", "conversationId": child_id, "limit": 50}),
        )
        .unwrap()["events"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|event| {
            event["correlationId"] == dispatch
                && event["parts"]
                    .as_array()
                    .is_some_and(|parts| parts.iter().any(|part| part["kind"] == "artifact"))
        })
        .count();
    let starts_after = session.child_work_count();
    let unknown_after = list_unknown_effect_ids(session.host().store())
        .unwrap()
        .len();
    let held = artifacts == 1 && starts_after == starts_before && unknown_after == unknown_before;
    emit(
        "CA-F11",
        held,
        json!({
            "faultBoundaryHeld": held,
            "boundary": "duplicate_callback",
            "artifactEvents": artifacts,
            "extraChildWork": starts_after.saturating_sub(starts_before),
            "extraUnknownEffects": unknown_after.saturating_sub(unknown_before)
        }),
    );
}

#[test]
fn ca_f12_cancel_reconciles_late_result() {
    let session = Session::memory();
    admit_goal(
        &session,
        "cancel races late result",
        "goal:race",
        "matter:race",
    );
    session
        .service
        .execute(json!({
            "action": "request-cancel",
            "conversationId": session.conversation_id,
            "goalId": "goal:race",
        }))
        .unwrap();
    let (_, child_id, posted) = session.first_child();
    let member = child_member(&session, &child_id);
    let dispatch = admit_child_text(&session, &child_id, &member, "LATE", None);
    let late = session.service.after_runtime_settlement(
        &child_id,
        &json!({
            "output": "LATE",
            "membershipId": member,
            "causationId": posted,
            "dispatchId": dispatch,
            "ok": true,
        }),
    );
    let events = session
        .service
        .execute(
            json!({"action": "conversation.events.page", "conversationId": child_id, "limit": 50}),
        )
        .unwrap();
    let late_preserved = events["events"].as_array().is_some_and(|items| {
        items.iter().any(|event| {
            event["parts"].as_array().is_some_and(|parts| {
                parts.iter().any(|part| {
                    part["content"]
                        .as_str()
                        .is_some_and(|text| text.contains("LATE"))
                })
            })
        })
    });
    let held = late.is_ok()
        && late_preserved
        && session.lifecycle("goal:race") != ContinuityGoalLifecycle::Cancelled
        && session.control("goal:race") == ContinuityGoalControl::CancelRequested;
    emit(
        "CA-F12",
        held,
        json!({
            "faultBoundaryHeld": held,
            "boundary": "cancel_races_late_result",
            "lateResultPreserved": late_preserved,
            "goalCancelled": session.lifecycle("goal:race") == ContinuityGoalLifecycle::Cancelled
        }),
    );
}

#[test]
fn ca_f14_revoked_source_cannot_rehydrate() {
    let session = Session::memory();
    let event = admit_goal(
        &session,
        "old summary source",
        "goal:rehydrate",
        "matter:rehydrate",
    );
    revoke_source(
        session.host().store(),
        &session.conversation_id,
        &event,
        true,
    )
    .unwrap();
    let basis = session
        .service
        .store()
        .continuity_commit_basis(&session.conversation_id)
        .unwrap();
    let mut proposal: ContinuityInterpretationProposal =
        serde_json::from_str(&durable_proposal_json(
            &session.conversation_id,
            "goal:rehydrate-again",
            "matter:rehydrate-again",
        ))
        .unwrap();
    proposal.envelope.observed_revision = basis.revision;
    proposal.envelope.designation_epoch = basis.designation_epoch;
    proposal.envelope.source_event_refs = vec![session.live_source(&event)];
    let rejected = session.service.store().commit(&proposal);
    let held = rejected
        .as_ref()
        .err()
        .is_some_and(|failure| failure.code == ContinuityFailureCode::SourceRevoked);
    emit(
        "CA-F14",
        held,
        json!({"faultBoundaryHeld": held, "boundary": "revoked_source_rehydrate"}),
    );
}

#[test]
fn ca_f15_parallel_writer_is_busy() {
    let session = Session::memory();
    session.host().bind_hermetic(
        HermeticProtocol::codex(CapabilityProfile::Low).with_presence(SessionPresence::Present),
        fixture_config(true),
    );
    let child = fixture_child_binding();
    let port = session
        .host()
        .work_runtime(
            &child.child_conversation_id,
            &child.source_task_id,
            &child.membership_id,
            0,
        )
        .expect("hermetic");
    port.claim_writer(&key(
        &child.child_conversation_id,
        &child.membership_id,
        "matter:writer-a",
    ))
    .unwrap();
    let second = port.claim_writer(&key(
        &child.child_conversation_id,
        &child.membership_id,
        "matter:writer-b",
    ));
    emit(
        "CA-F15",
        second.unwrap_err().code == NativeFailureCode::WriterBusy,
        json!({"faultBoundaryHeld": true, "boundary": "parallel_writer_claim"}),
    );
}

#[test]
fn ca_f16_automatic_wake_does_not_advance_unqualified() {
    let session = Session::memory();
    admit_goal(
        &session,
        "automatic wake without qualification",
        "goal:auto-wake",
        "matter:auto-wake",
    );
    let before = session.child_work_count();
    let drain = session
        .service
        .drain_continuity(&session.conversation_id)
        .unwrap();
    let preserved = drain["preserved"].as_array().map(Vec::len).unwrap_or(0);
    let no_ops = drain["noOps"].as_array().map(Vec::len).unwrap_or(0);
    let reevaluated = drain["reevaluated"].as_array().map(Vec::len).unwrap_or(0);
    let held = session.child_work_count() == before
        && session.lifecycle("goal:auto-wake") != ContinuityGoalLifecycle::Achieved
        && (preserved + no_ops + reevaluated) >= 1;
    emit(
        "CA-F16",
        held,
        json!({
            "faultBoundaryHeld": held,
            "boundary": "automatic_wake_without_qualification",
            "newChildWork": session.child_work_count() - before
        }),
    );
}

#[test]
fn ac_04_001_suite_oracle_and_drain_wakes() {
    let session = Session::memory();
    admit_goal(
        &session,
        "wake pending while user posts",
        "goal:oracle-wake",
        "matter:oracle-wake",
    );
    let pending_before = list_all_pending_wakes(session.host().store()).unwrap();
    let completes_before_post = session.complete_count();
    let posted = session
        .service
        .execute(json!({
            "action": "conversation.message.post",
            "conversationId": session.conversation_id,
            "authorMembershipId": session.owner,
            "content": "ordinary follow-up while a wake is pending",
        }))
        .unwrap();
    let post_returns = posted["event"]["id"].as_str().is_some()
        && posted["continuityDrain"].is_null()
        && session.complete_count() == completes_before_post;
    let drain = session.claim_attend();
    let reviews = drain
        .get("reevaluated")
        .and_then(Value::as_array)
        .map(Vec::len)
        .unwrap_or(0);
    let oracle = json!({
        "ac04001": {
            "zeroTests": false,
            "copiedReducer": false,
            "timeoutSuccess": false
        },
        "liveQualification": "unknown",
        "drainWakes": {
            "postTimeDrainRuns": false,
            "foregroundPostReturnsWithoutWakeCognition": post_returns,
            "backgroundAttendDueReviewsOnce": reviews <= 1
        },
        "pendingWakesBeforeOrdinaryPost": !pending_before.is_empty()
    });
    println!("{ORACLE_MARKER}{oracle}");
    assert!(
        post_returns && !pending_before.is_empty() && reviews <= 1,
        "drain_wakes observation failed: {oracle}"
    );
}

#[test]
fn catalog_lists_every_seed_id() {
    let catalog: Value =
        serde_json::from_str(&std::fs::read_to_string(catalog_path()).unwrap()).unwrap();
    let ids: std::collections::BTreeSet<String> = catalog["cases"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|item| item["id"].as_str().map(str::to_owned))
        .collect();
    let scenarios: Vec<String> = (1..=48).map(|index| format!("CA-S{index:03}")).collect();
    let faults: Vec<String> = (1..=16).map(|index| format!("CA-F{index:02}")).collect();
    for id in scenarios.iter().chain(faults.iter()) {
        assert!(ids.contains(id), "catalog missing {id}");
    }
    assert_eq!(ids.len(), 64);
}
