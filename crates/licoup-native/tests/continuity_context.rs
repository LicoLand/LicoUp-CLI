//! TASK-02-002 focused acceptance. Synthetic ids only.

use licoup_conversation::continuity::{
    CONTINUITY_MAX_ORIENTATION_ITEMS, ContextCompositionPort, ContinuityAgreement,
    ContinuityAgreementOrigin, ContinuityAgreementProposal, ContinuityAgreementScope,
    ContinuityCommitBasis, ContinuityContextCompositionRequest, ContinuityContextTransition,
    ContinuityFailureCode, ContinuityFollowThroughKind, ContinuityInterpretationProposal,
    ContinuityMatterSubject, ContinuityParentContextGrant, ContinuityParentGrantStatus,
    ContinuitySourceOwnerKind, ContinuitySourceRef, ContinuitySourceValidity, ContinuitySpeechAct,
    ContinuityUtf8ByteSpan, ContinuityVisibilityScope, DiscoveredKnowledgePort, InterpretationPort,
};
use licoup_native::domain::assistant_continuity::cognition::{
    ContextRecord, InformationClass, KnowledgeDiscoveryDescriptor, ScriptedAgent, SemanticScript,
    SpanAxis, UnavailableKnowledgeService, interpretation_port,
};
use licoup_native::domain::assistant_continuity::context::{
    ContinuityWorkspace, FrozenContextStore, UnavailableContextCompositionService,
    context_composition_port,
};

fn digest(tag: u8) -> String {
    format!("sha256:{:02x}{}", tag, "ab".repeat(31))
}

fn source(
    owner: ContinuitySourceOwnerKind,
    opaque_id: &str,
    revision: i64,
    tag: u8,
    scope: ContinuityVisibilityScope,
) -> ContinuitySourceRef {
    ContinuitySourceRef {
        owner_kind: owner,
        opaque_id: opaque_id.into(),
        part_id: Some("part:text".into()),
        span: None,
        source_revision: revision,
        digest: digest(tag),
        visibility_scope: scope,
        validity: ContinuitySourceValidity::Current,
    }
}

fn spanned(base: ContinuitySourceRef, start: u64, end: u64) -> ContinuitySourceRef {
    let mut item = base;
    item.owner_kind = ContinuitySourceOwnerKind::Span;
    item.span = Some(ContinuityUtf8ByteSpan {
        start_byte: start,
        end_byte: end,
    });
    item
}

fn request(
    conversation: &str,
    member: &str,
    generation: i64,
) -> ContinuityContextCompositionRequest {
    ContinuityContextCompositionRequest {
        conversation_id: conversation.into(),
        recipient_membership_id: member.into(),
        authorized_scopes: vec![
            ContinuityVisibilityScope::Conversation,
            ContinuityVisibilityScope::Matter,
            ContinuityVisibilityScope::Goal,
        ],
        revocation_generation: generation,
        after: None,
        limit: 20,
    }
}

fn basis(conversation: &str, revision: i64) -> ContinuityCommitBasis {
    ContinuityCommitBasis {
        conversation_id: conversation.into(),
        revision,
        designation_epoch: 1,
        assistant_membership_id: Some("membership:assistant".into()),
    }
}

fn record(
    conversation: &str,
    matter: Option<&str>,
    class: InformationClass,
    source_ref: ContinuitySourceRef,
    recency: i64,
) -> ContextRecord {
    ContextRecord {
        conversation_id: conversation.into(),
        matter_id: matter.map(str::to_string),
        class,
        source: source_ref,
        agreement: None,
        membership_id: Some("membership:assistant".into()),
        recency,
        entities: Vec::new(),
        text_bytes: 24,
        explicit_refs: Vec::new(),
        conversation_level: matter.is_none(),
        is_current_input: false,
        is_malicious_data: false,
        is_summary: false,
        is_worker_or_turn_exit: false,
        is_mcp_return: false,
    }
}

fn seed_basis(store: &FrozenContextStore, conversation: &str, revision: i64) {
    store.set_commit_basis(basis(conversation, revision));
    store.set_recipient_revocation(conversation, "membership:assistant", 0);
}

fn axis(
    source_ref: ContinuitySourceRef,
    subject: ContinuityMatterSubject,
    speech_act: ContinuitySpeechAct,
    follow: ContinuityFollowThroughKind,
    create_goal: bool,
    matter_id: Option<&str>,
    reason: &str,
) -> SpanAxis {
    SpanAxis {
        source_ref,
        subject,
        speech_act,
        follow_through: follow,
        create_goal,
        matter_id: matter_id.map(str::to_string),
        expected_result: Some(reason.into()),
        capability_needs: Vec::new(),
        uncertainty_reasons: Vec::new(),
        requested_reads: Vec::new(),
        agreement_proposals: Vec::new(),
        abstain: false,
        reason_code: reason.into(),
    }
}

fn session(
    store: FrozenContextStore,
    agent: ScriptedAgent,
    knowledge: UnavailableKnowledgeService,
) -> (
    UnavailableContextCompositionService,
    licoup_native::domain::assistant_continuity::cognition::UnavailableInterpretationService,
    std::sync::Arc<ContinuityWorkspace>,
) {
    let workspace = ContinuityWorkspace::new(store, agent, knowledge);
    let composer = UnavailableContextCompositionService::from_workspace(workspace.clone());
    let interpreter = workspace.interpreter();
    (composer, interpreter, workspace)
}

fn goal_created(proposal: &ContinuityInterpretationProposal) -> bool {
    proposal
        .commitment_proposals
        .iter()
        .any(|item| item.create_goal)
}

#[test]
fn default_ports_do_not_invent_goals_or_restore_two_arg_compose() {
    let composer = context_composition_port();
    let interpreter = interpretation_port();
    let failure = composer
        .compose_authorized(&request("conversation:missing", "membership:assistant", 0))
        .expect_err("empty store cannot invent context");
    assert_eq!(failure.code, ContinuityFailureCode::SourceUnavailable);
    assert_eq!(
        failure.effect_class,
        licoup_conversation::continuity::ContinuityEffectClass::None
    );
    let manifest = licoup_conversation::continuity::ContinuityContextManifest {
        invocation_id: "invocation:missing".into(),
        sources: Vec::new(),
        agreement_revisions: Vec::new(),
        acl_generation: 0,
        revocation_generation: 0,
        recipient_binding: "membership:assistant".into(),
        token_estimate: 0,
        selection_reason_codes: Vec::new(),
        context_transition: ContinuityContextTransition::New,
    };
    let interpret_err = interpreter
        .interpret(&manifest)
        .expect_err("empty interpreter cannot invent a proposal");
    assert_eq!(interpret_err.code, ContinuityFailureCode::SourceUnavailable);
}

#[test]
fn mixed_hypothetical_and_durable_delegation_keeps_independent_axes() {
    let store = FrozenContextStore::new();
    seed_basis(&store, "conversation:parent", 4);
    let event = source(
        ContinuitySourceOwnerKind::Event,
        "event:mixed",
        4,
        1,
        ContinuityVisibilityScope::Conversation,
    );
    let hypo = spanned(event.clone(), 0, 18);
    let durable = spanned(event.clone(), 18, 40);
    let mut input = record(
        "conversation:parent",
        None,
        InformationClass::ConversationFact,
        event.clone(),
        40,
    );
    input.is_current_input = true;
    input.explicit_refs = vec![hypo.clone(), durable.clone()];
    input.entities = vec!["implement".into(), "fn".into()];
    input.text_bytes = 400;
    store.insert_record(input);

    let mut agent = ScriptedAgent::new();
    let mut hypo_axis = axis(
        hypo.clone(),
        ContinuityMatterSubject::New,
        ContinuitySpeechAct::Hypothetical,
        ContinuityFollowThroughKind::None,
        false,
        Some("matter:maybe-app"),
        "development-possibility",
    );
    hypo_axis.capability_needs = vec!["discussion".into()];
    let mut durable_axis = axis(
        durable.clone(),
        ContinuityMatterSubject::New,
        ContinuitySpeechAct::Delegation,
        ContinuityFollowThroughKind::Durable,
        true,
        Some("matter:briefing"),
        "noncoding-follow-through",
    );
    durable_axis.capability_needs = vec!["writing".into()];
    durable_axis.expected_result = Some("Prepare the briefing notes".into());
    agent.insert(SemanticScript {
        event_opaque_id: "event:mixed".into(),
        axes: vec![hypo_axis, durable_axis],
        fused: true,
        model_confidence: Some(97),
        escalate: false,
    });

    let (composer, interpreter, workspace) =
        session(store, agent, UnavailableKnowledgeService::default());
    let manifest = composer
        .compose_authorized(&request("conversation:parent", "membership:assistant", 0))
        .unwrap();
    assert_eq!(manifest.recipient_binding, "membership:assistant");
    assert_eq!(manifest.revocation_generation, 0);
    let proposal = interpreter.interpret(&manifest).unwrap();
    assert_eq!(workspace.total_cognition(), 1);
    assert_eq!(
        proposal.envelope.source_event_refs[0].opaque_id,
        "event:mixed"
    );
    assert_eq!(proposal.speech_act, ContinuitySpeechAct::Delegation);
    assert_eq!(proposal.matter_associations.len(), 2);
    assert_eq!(proposal.commitment_proposals.len(), 2);
    let hypo_commit = proposal
        .commitment_proposals
        .iter()
        .find(|item| item.matter_id.as_deref() == Some("matter:maybe-app"))
        .unwrap();
    let durable_commit = proposal
        .commitment_proposals
        .iter()
        .find(|item| item.matter_id.as_deref() == Some("matter:briefing"))
        .unwrap();
    assert!(!hypo_commit.create_goal);
    assert_eq!(hypo_commit.subject, ContinuityMatterSubject::New);
    assert!(durable_commit.create_goal);
    assert_eq!(durable_commit.expected_result, "Prepare the briefing notes");
    let admission = proposal.task_child_admission.as_ref().unwrap();
    assert_eq!(
        admission.follow_through_kind,
        ContinuityFollowThroughKind::Durable
    );
    assert_eq!(admission.speech_act, ContinuitySpeechAct::Delegation);
    assert!(
        proposal
            .uncertainty_reasons
            .iter()
            .any(|reason| reason == "model-confidence-is-not-permission")
    );
}

#[test]
fn keyword_length_and_count_do_not_infer_delegation() {
    let store = FrozenContextStore::new();
    seed_basis(&store, "conversation:parent", 3);
    let event = source(
        ContinuitySourceOwnerKind::Event,
        "event:code-words",
        3,
        2,
        ContinuityVisibilityScope::Conversation,
    );
    let mut input = record(
        "conversation:parent",
        None,
        InformationClass::ConversationFact,
        event,
        10,
    );
    input.is_current_input = true;
    input.entities = vec![
        "TODO".into(),
        "implement".into(),
        "fn".into(),
        "main".into(),
    ];
    input.text_bytes = 8000;
    store.insert_record(input);
    let (composer, interpreter, _) = session(
        store,
        ScriptedAgent::new(),
        UnavailableKnowledgeService::default(),
    );
    let manifest = composer
        .compose_authorized(&request("conversation:parent", "membership:assistant", 0))
        .unwrap();
    let proposal = interpreter.interpret(&manifest).unwrap();
    assert!(!goal_created(&proposal));
    assert!(proposal.task_child_admission.is_none());
    assert_eq!(
        proposal.envelope.source_event_refs[0].opaque_id,
        "event:code-words"
    );
    assert!(
        proposal
            .uncertainty_reasons
            .iter()
            .any(|reason| reason == "no-qualified-interpreter")
    );
}

#[test]
fn matter_b_excludes_a_and_return_uses_latest_agreements() {
    let store = FrozenContextStore::new();
    seed_basis(&store, "conversation:work", 8);
    let input_b = source(
        ContinuitySourceOwnerKind::Event,
        "event:ask-b",
        8,
        3,
        ContinuityVisibilityScope::Conversation,
    );
    let fact_a = source(
        ContinuitySourceOwnerKind::Event,
        "event:a-secret",
        5,
        4,
        ContinuityVisibilityScope::Matter,
    );
    let agree_v1 = source(
        ContinuitySourceOwnerKind::Agreement,
        "agreement:tone",
        1,
        5,
        ContinuityVisibilityScope::Matter,
    );
    let agree_v2 = source(
        ContinuitySourceOwnerKind::Agreement,
        "agreement:tone",
        2,
        6,
        ContinuityVisibilityScope::Matter,
    );
    let mut current = record(
        "conversation:work",
        Some("matter:b"),
        InformationClass::ConversationFact,
        input_b,
        80,
    );
    current.is_current_input = true;
    current.conversation_level = false;
    store.insert_record(current);
    store.insert_record(record(
        "conversation:work",
        Some("matter:a"),
        InformationClass::ConversationFact,
        fact_a,
        20,
    ));
    let mut a1 = record(
        "conversation:work",
        Some("matter:a"),
        InformationClass::Agreement,
        agree_v1.clone(),
        21,
    );
    a1.agreement = Some(ContinuityAgreement {
        id: "agreement:tone".into(),
        scope: ContinuityAgreementScope::Matter,
        statement_ref: agree_v1,
        origin: ContinuityAgreementOrigin::UserExplicit,
        effective_revision: 1,
        supersedes: None,
        valid_from: 1,
        valid_until: Some(8),
        revocation_generation: 0,
    });
    store.insert_record(a1);
    let mut a2 = record(
        "conversation:work",
        Some("matter:a"),
        InformationClass::Agreement,
        agree_v2.clone(),
        81,
    );
    a2.agreement = Some(ContinuityAgreement {
        id: "agreement:tone".into(),
        scope: ContinuityAgreementScope::Matter,
        statement_ref: agree_v2.clone(),
        origin: ContinuityAgreementOrigin::UserExplicit,
        effective_revision: 2,
        supersedes: Some(1),
        valid_from: 8,
        valid_until: None,
        revocation_generation: 0,
    });
    store.insert_record(a2);
    store.set_attention("conversation:work", "matter:a");

    let (composer, _, _) = session(
        store.clone(),
        ScriptedAgent::new(),
        UnavailableKnowledgeService::default(),
    );
    let manifest_b = composer
        .compose_authorized(&request("conversation:work", "membership:assistant", 0))
        .unwrap();
    assert_eq!(
        manifest_b.context_transition,
        ContinuityContextTransition::New
    );
    assert!(
        !manifest_b
            .sources
            .iter()
            .any(|item| item.opaque_id == "event:a-secret" || item.opaque_id == "agreement:tone")
    );
    assert!(manifest_b.agreement_revisions.is_empty());

    store.set_attention("conversation:work", "matter:a");
    let mut return_a = record(
        "conversation:work",
        Some("matter:a"),
        InformationClass::ConversationFact,
        source(
            ContinuitySourceOwnerKind::Event,
            "event:return-a",
            9,
            7,
            ContinuityVisibilityScope::Conversation,
        ),
        90,
    );
    return_a.is_current_input = true;
    store.insert_record(return_a);
    store.set_commit_basis(basis("conversation:work", 9));
    let manifest_a = composer
        .compose_authorized(&request("conversation:work", "membership:assistant", 0))
        .unwrap();
    assert_eq!(manifest_a.agreement_revisions, vec![2]);
    assert!(
        manifest_a
            .sources
            .iter()
            .any(|item| item.opaque_id == "agreement:tone" && item.source_revision == 2)
    );
    assert!(
        !manifest_a
            .sources
            .iter()
            .any(|item| item.opaque_id == "event:ask-b")
    );
}

#[test]
fn child_assembles_granted_parent_refs_and_rejects_sibling() {
    let store = FrozenContextStore::new();
    seed_basis(&store, "conversation:child-a", 2);
    store.set_recipient_revocation("conversation:child-a", "membership:child-coordinator", 0);
    let parent_event = source(
        ContinuitySourceOwnerKind::Event,
        "event:parent-one",
        2,
        8,
        ContinuityVisibilityScope::Conversation,
    );
    let sibling = source(
        ContinuitySourceOwnerKind::Event,
        "event:sibling-b",
        2,
        9,
        ContinuityVisibilityScope::Conversation,
    );
    let child_input = source(
        ContinuitySourceOwnerKind::Event,
        "event:child-a-now",
        2,
        10,
        ContinuityVisibilityScope::Conversation,
    );
    let mut current = record(
        "conversation:child-a",
        Some("matter:task"),
        InformationClass::ConversationFact,
        child_input.clone(),
        5,
    );
    current.is_current_input = true;
    current.explicit_refs = vec![parent_event.clone(), sibling.clone()];
    store.insert_record(current);
    store.insert_record(record(
        "conversation:parent",
        None,
        InformationClass::ConversationFact,
        parent_event.clone(),
        1,
    ));
    store.insert_record(record(
        "conversation:child-b",
        Some("matter:other"),
        InformationClass::ConversationFact,
        sibling.clone(),
        2,
    ));
    store.insert_grant(ContinuityParentContextGrant {
        grant_id: "grant:parent-to-child-a".into(),
        source_conversation_id: "conversation:parent".into(),
        recipient_conversation_id: "conversation:child-a".into(),
        recipient_membership_id: "membership:child-coordinator".into(),
        source_refs: vec![parent_event.clone()],
        authorized_scopes: vec![ContinuityVisibilityScope::Conversation],
        status: ContinuityParentGrantStatus::Admitted,
        request_id: "request:grant-1".into(),
        revocation_generation: 0,
    });

    let (composer, _, _) = session(
        store,
        ScriptedAgent::new(),
        UnavailableKnowledgeService::default(),
    );
    let manifest = composer
        .compose_authorized(&request(
            "conversation:child-a",
            "membership:child-coordinator",
            0,
        ))
        .unwrap();
    assert!(
        manifest
            .sources
            .iter()
            .any(|item| item.opaque_id == "event:parent-one")
    );
    assert!(
        !manifest
            .sources
            .iter()
            .any(|item| item.opaque_id == "event:sibling-b")
    );
}

#[test]
fn granted_parent_ref_keeps_original_identity() {
    let store = FrozenContextStore::new();
    seed_basis(&store, "conversation:child-a", 2);
    store.set_recipient_revocation("conversation:child-a", "membership:child-coordinator", 0);
    let parent_event = source(
        ContinuitySourceOwnerKind::Event,
        "event:parent-one",
        2,
        11,
        ContinuityVisibilityScope::Conversation,
    );
    let child_input = source(
        ContinuitySourceOwnerKind::Event,
        "event:child-a-now",
        2,
        12,
        ContinuityVisibilityScope::Conversation,
    );
    let mut current = record(
        "conversation:child-a",
        Some("matter:task"),
        InformationClass::ConversationFact,
        child_input,
        5,
    );
    current.is_current_input = true;
    current.explicit_refs = vec![parent_event.clone()];
    store.insert_record(current);
    store.insert_record(record(
        "conversation:parent",
        None,
        InformationClass::ConversationFact,
        parent_event.clone(),
        1,
    ));
    store.insert_grant(ContinuityParentContextGrant {
        grant_id: "grant:parent-to-child-a".into(),
        source_conversation_id: "conversation:parent".into(),
        recipient_conversation_id: "conversation:child-a".into(),
        recipient_membership_id: "membership:child-coordinator".into(),
        source_refs: vec![parent_event.clone()],
        authorized_scopes: vec![ContinuityVisibilityScope::Conversation],
        status: ContinuityParentGrantStatus::Admitted,
        request_id: "request:grant-1".into(),
        revocation_generation: 0,
    });
    let (composer, _, _) = session(
        store,
        ScriptedAgent::new(),
        UnavailableKnowledgeService::default(),
    );
    let manifest = composer
        .compose_authorized(&request(
            "conversation:child-a",
            "membership:child-coordinator",
            0,
        ))
        .unwrap();
    let parent = manifest
        .sources
        .iter()
        .find(|item| item.opaque_id == "event:parent-one")
        .unwrap();
    assert_eq!(parent.source_revision, 2);
    assert_eq!(parent.digest, parent_event.digest);
    assert_eq!(parent.owner_kind, ContinuitySourceOwnerKind::Event);
    composer.recheck_dispatch(&manifest).unwrap();
}

#[test]
fn revocation_and_scope_shrink_invalidate_manifest() {
    let store = FrozenContextStore::new();
    seed_basis(&store, "conversation:child-a", 2);
    store.set_recipient_revocation("conversation:child-a", "membership:child-coordinator", 0);
    let parent_base = source(
        ContinuitySourceOwnerKind::Event,
        "event:parent-span",
        2,
        13,
        ContinuityVisibilityScope::Conversation,
    );
    let parent_event = spanned(parent_base.clone(), 0, 20);
    let child_input = source(
        ContinuitySourceOwnerKind::Event,
        "event:child-now",
        2,
        14,
        ContinuityVisibilityScope::Conversation,
    );
    let mut current = record(
        "conversation:child-a",
        None,
        InformationClass::ConversationFact,
        child_input,
        3,
    );
    current.is_current_input = true;
    current.explicit_refs = vec![parent_event.clone()];
    store.insert_record(current);
    store.insert_record(record(
        "conversation:parent",
        None,
        InformationClass::ConversationFact,
        parent_base,
        1,
    ));
    store.insert_grant(ContinuityParentContextGrant {
        grant_id: "grant:span".into(),
        source_conversation_id: "conversation:parent".into(),
        recipient_conversation_id: "conversation:child-a".into(),
        recipient_membership_id: "membership:child-coordinator".into(),
        source_refs: vec![parent_event.clone()],
        authorized_scopes: vec![ContinuityVisibilityScope::Conversation],
        status: ContinuityParentGrantStatus::Admitted,
        request_id: "request:grant-span".into(),
        revocation_generation: 0,
    });
    let (composer, _, _) = session(
        store.clone(),
        ScriptedAgent::new(),
        UnavailableKnowledgeService::default(),
    );
    let manifest = composer
        .compose_authorized(&request(
            "conversation:child-a",
            "membership:child-coordinator",
            0,
        ))
        .unwrap();
    store.shrink_grant_span(
        "grant:span",
        ContinuityUtf8ByteSpan {
            start_byte: 0,
            end_byte: 4,
        },
    );
    assert_eq!(
        composer.recheck_dispatch(&manifest).unwrap_err().code,
        ContinuityFailureCode::ScopeDenied
    );
    store.bump_grant_generation("grant:span", 3);
    store.set_recipient_revocation("conversation:child-a", "membership:child-coordinator", 3);
    assert_eq!(
        composer.recheck_dispatch(&manifest).unwrap_err().code,
        ContinuityFailureCode::StaleRevision
    );
}

#[test]
fn fresh_parent_agreement_correction_replaces_prior_revision() {
    let store = FrozenContextStore::new();
    seed_basis(&store, "conversation:child-a", 3);
    store.set_recipient_revocation("conversation:child-a", "membership:child-coordinator", 0);
    let v1 = source(
        ContinuitySourceOwnerKind::Agreement,
        "agreement:parent-tone",
        1,
        15,
        ContinuityVisibilityScope::Conversation,
    );
    let v2 = source(
        ContinuitySourceOwnerKind::Agreement,
        "agreement:parent-tone",
        2,
        16,
        ContinuityVisibilityScope::Conversation,
    );
    let child_input = source(
        ContinuitySourceOwnerKind::Event,
        "event:child-now",
        3,
        17,
        ContinuityVisibilityScope::Conversation,
    );
    let mut current = record(
        "conversation:child-a",
        None,
        InformationClass::ConversationFact,
        child_input,
        9,
    );
    current.is_current_input = true;
    store.insert_record(current);
    let mut first = record(
        "conversation:parent",
        None,
        InformationClass::Agreement,
        v1.clone(),
        1,
    );
    first.agreement = Some(ContinuityAgreement {
        id: "agreement:parent-tone".into(),
        scope: ContinuityAgreementScope::Conversation,
        statement_ref: v1.clone(),
        origin: ContinuityAgreementOrigin::UserExplicit,
        effective_revision: 1,
        supersedes: None,
        valid_from: 1,
        valid_until: Some(3),
        revocation_generation: 0,
    });
    let mut second = record(
        "conversation:parent",
        None,
        InformationClass::Agreement,
        v2.clone(),
        3,
    );
    second.agreement = Some(ContinuityAgreement {
        id: "agreement:parent-tone".into(),
        scope: ContinuityAgreementScope::Conversation,
        statement_ref: v2.clone(),
        origin: ContinuityAgreementOrigin::UserExplicit,
        effective_revision: 2,
        supersedes: Some(1),
        valid_from: 3,
        valid_until: None,
        revocation_generation: 0,
    });
    store.insert_record(first);
    store.insert_record(second);
    store.insert_grant(ContinuityParentContextGrant {
        grant_id: "grant:agree".into(),
        source_conversation_id: "conversation:parent".into(),
        recipient_conversation_id: "conversation:child-a".into(),
        recipient_membership_id: "membership:child-coordinator".into(),
        source_refs: vec![v2.clone()],
        authorized_scopes: vec![ContinuityVisibilityScope::Conversation],
        status: ContinuityParentGrantStatus::Admitted,
        request_id: "request:grant-agree".into(),
        revocation_generation: 0,
    });
    let (composer, _, _) = session(
        store,
        ScriptedAgent::new(),
        UnavailableKnowledgeService::default(),
    );
    let manifest = composer
        .compose_authorized(&request(
            "conversation:child-a",
            "membership:child-coordinator",
            0,
        ))
        .unwrap();
    assert_eq!(manifest.agreement_revisions, vec![2]);
    assert!(
        manifest
            .sources
            .iter()
            .any(|item| item.opaque_id == "agreement:parent-tone" && item.source_revision == 2)
    );
    assert!(
        !manifest
            .sources
            .iter()
            .any(|item| item.opaque_id == "agreement:parent-tone" && item.source_revision == 1)
    );
}

#[test]
fn information_classes_and_knowledge_discovery_stay_separate() {
    let store = FrozenContextStore::new();
    seed_basis(&store, "conversation:work", 4);
    let input = source(
        ContinuitySourceOwnerKind::Event,
        "event:now",
        4,
        18,
        ContinuityVisibilityScope::Conversation,
    );
    let note = source(
        ContinuitySourceOwnerKind::Artifact,
        "note:scratch",
        1,
        19,
        ContinuityVisibilityScope::Matter,
    );
    let fact = source(
        ContinuitySourceOwnerKind::Event,
        "event:fact",
        2,
        20,
        ContinuityVisibilityScope::Conversation,
    );
    let mut current = record(
        "conversation:work",
        Some("matter:a"),
        InformationClass::ConversationFact,
        input,
        10,
    );
    current.is_current_input = true;
    store.insert_record(current);
    store.insert_record(record(
        "conversation:work",
        Some("matter:a"),
        InformationClass::WorkingNote,
        note,
        8,
    ));
    let mut conversation_fact = record(
        "conversation:work",
        None,
        InformationClass::ConversationFact,
        fact,
        7,
    );
    conversation_fact.conversation_level = true;
    store.insert_record(conversation_fact);
    let knowledge = UnavailableKnowledgeService::from_descriptors(vec![
        KnowledgeDiscoveryDescriptor {
            capability: "style-guide".into(),
            advertised_version: "1".into(),
            input_contract_digest: digest(21),
            disclosure_class: "read-only".into(),
            available: true,
        },
        KnowledgeDiscoveryDescriptor {
            capability: "offline-lexicon".into(),
            advertised_version: "1".into(),
            input_contract_digest: digest(22),
            disclosure_class: "read-only".into(),
            available: false,
        },
    ]);
    let mut agent = ScriptedAgent::new();
    let mut live = axis(
        source(
            ContinuitySourceOwnerKind::Event,
            "event:now",
            4,
            18,
            ContinuityVisibilityScope::Conversation,
        ),
        ContinuityMatterSubject::Existing,
        ContinuitySpeechAct::Question,
        ContinuityFollowThroughKind::None,
        false,
        Some("matter:a"),
        "ask",
    );
    live.capability_needs = vec![
        "knowledge:style-guide".into(),
        "knowledge:offline-lexicon".into(),
    ];
    agent.insert(SemanticScript {
        event_opaque_id: "event:now".into(),
        axes: vec![live],
        fused: true,
        model_confidence: None,
        escalate: false,
    });
    let (composer, interpreter, _) = session(store, agent, knowledge.clone());
    let manifest = composer
        .compose_authorized(&request("conversation:work", "membership:assistant", 0))
        .unwrap();
    assert!(
        manifest
            .sources
            .iter()
            .any(|item| item.opaque_id == "event:fact")
    );
    let proposal = interpreter.interpret(&manifest).unwrap();
    assert!(!goal_created(&proposal));
    assert!(
        proposal
            .uncertainty_reasons
            .iter()
            .any(|reason| reason == "knowledge-unavailable:offline-lexicon")
    );
    let found = knowledge.lookup("style-guide").unwrap();
    assert_eq!(found.owner_kind, ContinuitySourceOwnerKind::Knowledge);
    assert_eq!(found.opaque_id, "style-guide");
    assert_eq!(
        knowledge.lookup("offline-lexicon").unwrap_err().code,
        ContinuityFailureCode::SourceUnavailable
    );
    assert_eq!(
        knowledge.lookup("missing").unwrap_err().code,
        ContinuityFailureCode::SourceUnavailable
    );
}

#[test]
fn fused_path_does_not_duplicate_cognition_and_two_step_refines_once() {
    let store = FrozenContextStore::new();
    seed_basis(&store, "conversation:work", 5);
    let event = source(
        ContinuitySourceOwnerKind::Event,
        "event:fused",
        5,
        23,
        ContinuityVisibilityScope::Conversation,
    );
    let extra = source(
        ContinuitySourceOwnerKind::Event,
        "event:needed",
        4,
        24,
        ContinuityVisibilityScope::Conversation,
    );
    let mut current = record(
        "conversation:work",
        Some("matter:a"),
        InformationClass::ConversationFact,
        event.clone(),
        11,
    );
    current.is_current_input = true;
    store.insert_record(current);
    store.insert_record(record(
        "conversation:work",
        Some("matter:a"),
        InformationClass::ConversationFact,
        extra.clone(),
        4,
    ));
    let mut fused_agent = ScriptedAgent::new();
    fused_agent.insert(SemanticScript {
        event_opaque_id: "event:fused".into(),
        axes: vec![axis(
            event.clone(),
            ContinuityMatterSubject::Existing,
            ContinuitySpeechAct::Question,
            ContinuityFollowThroughKind::None,
            false,
            Some("matter:a"),
            "fused",
        )],
        fused: true,
        model_confidence: None,
        escalate: false,
    });
    let (composer, interpreter, workspace) = session(
        store.clone(),
        fused_agent,
        UnavailableKnowledgeService::default(),
    );
    let manifest = composer
        .compose_authorized(&request("conversation:work", "membership:assistant", 0))
        .unwrap();
    let first = interpreter.interpret(&manifest).unwrap();
    let again = interpreter.interpret(&manifest).unwrap();
    assert_eq!(first, again);
    assert_eq!(workspace.total_cognition(), 1);

    let store_two = FrozenContextStore::new();
    seed_basis(&store_two, "conversation:work", 5);
    let mut current_two = record(
        "conversation:work",
        Some("matter:a"),
        InformationClass::ConversationFact,
        event.clone(),
        11,
    );
    current_two.is_current_input = true;
    store_two.insert_record(current_two);
    store_two.insert_record(record(
        "conversation:work",
        Some("matter:a"),
        InformationClass::ConversationFact,
        extra.clone(),
        4,
    ));
    let mut two_step = ScriptedAgent::new();
    let mut needs_read = axis(
        event,
        ContinuityMatterSubject::Existing,
        ContinuitySpeechAct::Question,
        ContinuityFollowThroughKind::None,
        false,
        Some("matter:a"),
        "two-step",
    );
    needs_read.requested_reads = vec![extra];
    two_step.insert(SemanticScript {
        event_opaque_id: "event:fused".into(),
        axes: vec![needs_read],
        fused: false,
        model_confidence: None,
        escalate: true,
    });
    let (composer, interpreter, workspace) =
        session(store_two, two_step, UnavailableKnowledgeService::default());
    let orientation = composer
        .compose_authorized(&request("conversation:work", "membership:assistant", 0))
        .unwrap();
    let waiting = interpreter.interpret(&orientation).unwrap();
    assert!(!waiting.requested_reads.is_empty());
    assert!(
        waiting
            .uncertainty_reasons
            .iter()
            .any(|reason| reason == "escalation-requested")
    );
    let refined = composer
        .refine_authorized(
            &request("conversation:work", "membership:assistant", 0),
            &waiting,
        )
        .unwrap();
    let done = interpreter.interpret(&refined).unwrap();
    assert!(done.requested_reads.is_empty());
    assert_eq!(workspace.total_cognition(), 2);
}

#[test]
fn worker_turn_exit_and_mcp_return_are_not_goal_acceptance() {
    let store = FrozenContextStore::new();
    seed_basis(&store, "conversation:parent", 6);
    let callback = source(
        ContinuitySourceOwnerKind::Event,
        "event:worker-exit",
        6,
        25,
        ContinuityVisibilityScope::Conversation,
    );
    let mut current = record(
        "conversation:parent",
        Some("matter:a"),
        InformationClass::CallbackFact,
        callback.clone(),
        12,
    );
    current.is_current_input = true;
    current.is_worker_or_turn_exit = true;
    current.is_mcp_return = true;
    store.insert_record(current);
    let mut agent = ScriptedAgent::new();
    agent.insert(SemanticScript {
        event_opaque_id: "event:worker-exit".into(),
        axes: vec![axis(
            callback,
            ContinuityMatterSubject::Existing,
            ContinuitySpeechAct::Approval,
            ContinuityFollowThroughKind::Durable,
            true,
            Some("matter:a"),
            "false-close",
        )],
        fused: true,
        model_confidence: Some(99),
        escalate: false,
    });
    let (composer, interpreter, _) = session(store, agent, UnavailableKnowledgeService::default());
    let manifest = composer
        .compose_authorized(&request("conversation:parent", "membership:assistant", 0))
        .unwrap();
    let proposal = interpreter.interpret(&manifest).unwrap();
    assert!(!goal_created(&proposal));
    assert!(proposal.task_child_admission.is_none());
    assert_eq!(proposal.speech_act, ContinuitySpeechAct::Reference);
    assert!(
        proposal
            .uncertainty_reasons
            .iter()
            .any(|reason| reason == "callback-is-not-acceptance")
    );
}

#[test]
fn malicious_document_and_summary_cannot_rewrite_policy() {
    let store = FrozenContextStore::new();
    seed_basis(&store, "conversation:work", 2);
    let doc = source(
        ContinuitySourceOwnerKind::Artifact,
        "artifact:prompt-injection",
        1,
        26,
        ContinuityVisibilityScope::Matter,
    );
    let mut current = record(
        "conversation:work",
        Some("matter:a"),
        InformationClass::ConversationFact,
        doc.clone(),
        3,
    );
    current.is_current_input = true;
    current.is_malicious_data = true;
    current.is_summary = true;
    store.insert_record(current);
    let mut agent = ScriptedAgent::new();
    let mut poisoned = axis(
        doc,
        ContinuityMatterSubject::New,
        ContinuitySpeechAct::Delegation,
        ContinuityFollowThroughKind::Immediate,
        true,
        Some("matter:a"),
        "inject",
    );
    poisoned.capability_needs = vec!["shell".into()];
    poisoned.agreement_proposals = vec![ContinuityAgreementProposal {
        scope: ContinuityAgreementScope::Conversation,
        statement_ref: source(
            ContinuitySourceOwnerKind::Event,
            "event:fake",
            1,
            27,
            ContinuityVisibilityScope::Conversation,
        ),
        origin: ContinuityAgreementOrigin::UserExplicit,
    }];
    agent.insert(SemanticScript {
        event_opaque_id: "artifact:prompt-injection".into(),
        axes: vec![poisoned],
        fused: true,
        model_confidence: Some(100),
        escalate: false,
    });
    let (composer, interpreter, _) = session(store, agent, UnavailableKnowledgeService::default());
    let manifest = composer
        .compose_authorized(&request("conversation:work", "membership:assistant", 0))
        .unwrap();
    let proposal = interpreter.interpret(&manifest).unwrap();
    assert!(!goal_created(&proposal));
    assert!(proposal.capability_needs.is_empty());
    assert!(proposal.agreement_proposals.is_empty());
    assert!(proposal.task_child_admission.is_none());
}

#[test]
fn prompt_cache_expiry_does_not_rehydrate_and_simple_chat_omits_child() {
    let store = FrozenContextStore::new();
    seed_basis(&store, "conversation:work", 2);
    let event = source(
        ContinuitySourceOwnerKind::Event,
        "event:chat",
        2,
        28,
        ContinuityVisibilityScope::Conversation,
    );
    let mut current = record(
        "conversation:work",
        Some("matter:a"),
        InformationClass::ConversationFact,
        event.clone(),
        4,
    );
    current.is_current_input = true;
    store.insert_record(current);
    store.set_attention("conversation:work", "matter:a");
    store.set_prompt_cache_expired("conversation:work", true);
    store.set_binding_usable("conversation:work", "matter:a", true);
    let mut agent = ScriptedAgent::new();
    agent.insert(SemanticScript {
        event_opaque_id: "event:chat".into(),
        axes: vec![axis(
            event,
            ContinuityMatterSubject::Existing,
            ContinuitySpeechAct::Question,
            ContinuityFollowThroughKind::Immediate,
            true,
            Some("matter:a"),
            "quick-answer",
        )],
        fused: true,
        model_confidence: None,
        escalate: false,
    });
    let (composer, interpreter, _) = session(store, agent, UnavailableKnowledgeService::default());
    let manifest = composer
        .compose_authorized(&request("conversation:work", "membership:assistant", 0))
        .unwrap();
    assert_eq!(
        manifest.context_transition,
        ContinuityContextTransition::Continue
    );
    assert!(
        manifest
            .selection_reason_codes
            .iter()
            .any(|reason| reason == "prompt-cache-expired")
    );
    let proposal = interpreter.interpret(&manifest).unwrap();
    assert!(goal_created(&proposal));
    assert!(proposal.task_child_admission.is_none());
}

#[test]
fn exact_refs_outrank_lexical_hints() {
    let store = FrozenContextStore::new();
    seed_basis(&store, "conversation:work", 2);
    let input = source(
        ContinuitySourceOwnerKind::Event,
        "event:now",
        2,
        29,
        ContinuityVisibilityScope::Conversation,
    );
    let exact = source(
        ContinuitySourceOwnerKind::Event,
        "event:exact",
        1,
        30,
        ContinuityVisibilityScope::Conversation,
    );
    let lexical = source(
        ContinuitySourceOwnerKind::Event,
        "event:hint",
        1,
        31,
        ContinuityVisibilityScope::Conversation,
    );
    let mut current = record(
        "conversation:work",
        Some("matter:a"),
        InformationClass::ConversationFact,
        input,
        10,
    );
    current.is_current_input = true;
    current.entities = vec!["table".into()];
    current.explicit_refs = vec![exact.clone()];
    store.insert_record(current);
    store.insert_record(record(
        "conversation:work",
        Some("matter:a"),
        InformationClass::ConversationFact,
        exact,
        1,
    ));
    let mut hint = record(
        "conversation:work",
        Some("matter:a"),
        InformationClass::ConversationFact,
        lexical,
        9,
    );
    hint.entities = vec!["table".into()];
    store.insert_record(hint);
    let (composer, _, _) = session(
        store,
        ScriptedAgent::new(),
        UnavailableKnowledgeService::default(),
    );
    let manifest = composer
        .compose_authorized(&request("conversation:work", "membership:assistant", 0))
        .unwrap();
    assert_eq!(manifest.selection_reason_codes[0], "current-input");
    assert!(
        manifest
            .selection_reason_codes
            .iter()
            .any(|reason| reason == "exact-ref")
    );
    let exact_pos = manifest
        .sources
        .iter()
        .position(|item| item.opaque_id == "event:exact")
        .unwrap();
    let hint_pos = manifest
        .sources
        .iter()
        .position(|item| item.opaque_id == "event:hint")
        .unwrap();
    assert!(exact_pos < hint_pos);
}

#[test]
fn sibling_requested_read_is_denied_and_lost_binding_rehydrates() {
    let store = FrozenContextStore::new();
    seed_basis(&store, "conversation:child-a", 2);
    store.set_recipient_revocation("conversation:child-a", "membership:child-coordinator", 0);
    let sibling = source(
        ContinuitySourceOwnerKind::Event,
        "event:sibling-b",
        2,
        32,
        ContinuityVisibilityScope::Conversation,
    );
    let child_input = source(
        ContinuitySourceOwnerKind::Event,
        "event:child-now",
        2,
        33,
        ContinuityVisibilityScope::Conversation,
    );
    let mut current = record(
        "conversation:child-a",
        Some("matter:task"),
        InformationClass::ConversationFact,
        child_input.clone(),
        5,
    );
    current.is_current_input = true;
    store.insert_record(current);
    store.insert_record(record(
        "conversation:child-b",
        Some("matter:other"),
        InformationClass::ConversationFact,
        sibling.clone(),
        2,
    ));
    let mut agent = ScriptedAgent::new();
    let mut needs_sibling = axis(
        child_input,
        ContinuityMatterSubject::Existing,
        ContinuitySpeechAct::Reference,
        ContinuityFollowThroughKind::None,
        false,
        Some("matter:task"),
        "bad-read",
    );
    needs_sibling.requested_reads = vec![sibling];
    agent.insert(SemanticScript {
        event_opaque_id: "event:child-now".into(),
        axes: vec![needs_sibling],
        fused: false,
        model_confidence: None,
        escalate: false,
    });
    let (composer, interpreter, _) =
        session(store.clone(), agent, UnavailableKnowledgeService::default());
    let orientation = composer
        .compose_authorized(&request(
            "conversation:child-a",
            "membership:child-coordinator",
            0,
        ))
        .unwrap();
    let waiting = interpreter.interpret(&orientation).unwrap();
    assert_eq!(
        composer
            .refine_authorized(
                &request("conversation:child-a", "membership:child-coordinator", 0,),
                &waiting,
            )
            .unwrap_err()
            .code,
        ContinuityFailureCode::ScopeDenied
    );

    store.set_attention("conversation:child-a", "matter:task");
    store.set_binding_usable("conversation:child-a", "matter:task", false);
    let lost = composer
        .compose_authorized(&request(
            "conversation:child-a",
            "membership:child-coordinator",
            0,
        ))
        .unwrap();
    assert_eq!(
        lost.context_transition,
        ContinuityContextTransition::Rehydrate
    );
}

#[test]
fn requested_scopes_and_full_source_identity_bound_reads() {
    let store = FrozenContextStore::new();
    seed_basis(&store, "conversation:scope", 2);
    store.set_recipient_revocation("conversation:scope", "membership:assistant", 0);

    let current_source = source(
        ContinuitySourceOwnerKind::Event,
        "event:scope-now",
        2,
        34,
        ContinuityVisibilityScope::Conversation,
    );
    let parent_source = source(
        ContinuitySourceOwnerKind::Event,
        "event:matter-only",
        2,
        35,
        ContinuityVisibilityScope::Matter,
    );
    let local_matter_source = source(
        ContinuitySourceOwnerKind::Event,
        "event:local-matter-only",
        2,
        81,
        ContinuityVisibilityScope::Matter,
    );
    let mut current = record(
        "conversation:scope",
        None,
        InformationClass::ConversationFact,
        current_source,
        2,
    );
    current.is_current_input = true;
    current.explicit_refs = vec![parent_source.clone()];
    store.insert_record(current);
    store.insert_record(record(
        "conversation:parent",
        None,
        InformationClass::ConversationFact,
        parent_source.clone(),
        1,
    ));
    store.insert_record(record(
        "conversation:scope",
        None,
        InformationClass::ConversationFact,
        local_matter_source,
        1,
    ));
    store.insert_grant(ContinuityParentContextGrant {
        grant_id: "grant:matter-only".into(),
        source_conversation_id: "conversation:parent".into(),
        recipient_conversation_id: "conversation:scope".into(),
        recipient_membership_id: "membership:assistant".into(),
        source_refs: vec![parent_source],
        authorized_scopes: vec![ContinuityVisibilityScope::Matter],
        status: ContinuityParentGrantStatus::Admitted,
        request_id: "request:matter-only".into(),
        revocation_generation: 0,
    });

    let mut restricted = request("conversation:scope", "membership:assistant", 0);
    restricted.authorized_scopes = vec![ContinuityVisibilityScope::Conversation];
    let (composer, _, _) = session(
        store,
        ScriptedAgent::new(),
        UnavailableKnowledgeService::default(),
    );
    let manifest = composer.compose_authorized(&restricted).unwrap();
    assert!(
        manifest
            .sources
            .iter()
            .any(|item| item.opaque_id == "event:matter-only")
    );
    assert!(
        !manifest
            .sources
            .iter()
            .any(|item| item.opaque_id == "event:local-matter-only")
    );

    let store = FrozenContextStore::new();
    seed_basis(&store, "conversation:identity", 2);
    let current_source = source(
        ContinuitySourceOwnerKind::Event,
        "event:identity-now",
        2,
        36,
        ContinuityVisibilityScope::Conversation,
    );
    let stored_ref = source(
        ContinuitySourceOwnerKind::Event,
        "event:identity-target",
        2,
        37,
        ContinuityVisibilityScope::Conversation,
    );
    let mut forged_ref = stored_ref.clone();
    forged_ref.digest = digest(38);
    let mut current = record(
        "conversation:identity",
        None,
        InformationClass::ConversationFact,
        current_source,
        2,
    );
    current.is_current_input = true;
    current.explicit_refs = vec![forged_ref];
    store.insert_record(current);
    store.insert_record(record(
        "conversation:identity",
        None,
        InformationClass::ConversationFact,
        stored_ref,
        1,
    ));
    let (composer, _, _) = session(
        store,
        ScriptedAgent::new(),
        UnavailableKnowledgeService::default(),
    );
    assert_eq!(
        composer
            .compose_authorized(&request("conversation:identity", "membership:assistant", 0,))
            .unwrap_err()
            .code,
        ContinuityFailureCode::SourceUnavailable
    );
}

#[test]
fn dispatch_rechecks_basis_designation_and_acl_generation() {
    let fixture = || {
        let store = FrozenContextStore::new();
        seed_basis(&store, "conversation:dispatch", 2);
        let mut current = record(
            "conversation:dispatch",
            None,
            InformationClass::ConversationFact,
            source(
                ContinuitySourceOwnerKind::Event,
                "event:dispatch-now",
                2,
                39,
                ContinuityVisibilityScope::Conversation,
            ),
            2,
        );
        current.is_current_input = true;
        store.insert_record(current);
        let (composer, _, _) = session(
            store.clone(),
            ScriptedAgent::new(),
            UnavailableKnowledgeService::default(),
        );
        let manifest = composer
            .compose_authorized(&request("conversation:dispatch", "membership:assistant", 0))
            .unwrap();
        (store, composer, manifest)
    };

    let (store, composer, manifest) = fixture();
    store.set_commit_basis(basis("conversation:dispatch", 3));
    assert_eq!(
        composer.recheck_dispatch(&manifest).unwrap_err().code,
        ContinuityFailureCode::StaleRevision
    );

    let (store, composer, manifest) = fixture();
    let mut designation_changed = basis("conversation:dispatch", 2);
    designation_changed.designation_epoch = 2;
    store.set_commit_basis(designation_changed);
    assert_eq!(
        composer.recheck_dispatch(&manifest).unwrap_err().code,
        ContinuityFailureCode::DesignationChanged
    );

    let (store, composer, manifest) = fixture();
    store.set_acl_generation("conversation:dispatch", 2);
    assert_eq!(
        composer.recheck_dispatch(&manifest).unwrap_err().code,
        ContinuityFailureCode::StaleRevision
    );
}

#[test]
fn orientation_budget_and_token_estimate_are_bounded() {
    let store = FrozenContextStore::new();
    seed_basis(&store, "conversation:budget", 40);
    let mut current = record(
        "conversation:budget",
        None,
        InformationClass::ConversationFact,
        source(
            ContinuitySourceOwnerKind::Event,
            "event:budget-now",
            40,
            80,
            ContinuityVisibilityScope::Conversation,
        ),
        40,
    );
    current.is_current_input = true;
    current.text_bytes = u64::MAX;
    store.insert_record(current);

    for revision in 1_i64..=40 {
        let agreement_id = format!("agreement:budget-{revision}");
        let statement_ref = source(
            ContinuitySourceOwnerKind::Agreement,
            &agreement_id,
            revision,
            revision as u8,
            ContinuityVisibilityScope::Conversation,
        );
        let mut agreement_record = record(
            "conversation:budget",
            None,
            InformationClass::Agreement,
            statement_ref.clone(),
            revision,
        );
        agreement_record.text_bytes = u64::MAX;
        agreement_record.agreement = Some(ContinuityAgreement {
            id: agreement_id,
            scope: ContinuityAgreementScope::Conversation,
            statement_ref,
            origin: ContinuityAgreementOrigin::UserExplicit,
            effective_revision: revision,
            supersedes: None,
            valid_from: revision,
            valid_until: None,
            revocation_generation: 0,
        });
        store.insert_record(agreement_record);
    }

    let (composer, _, _) = session(
        store,
        ScriptedAgent::new(),
        UnavailableKnowledgeService::default(),
    );
    let manifest = composer
        .compose_authorized(&request("conversation:budget", "membership:assistant", 0))
        .unwrap();
    assert_eq!(manifest.sources.len(), CONTINUITY_MAX_ORIENTATION_ITEMS);
    assert!(
        manifest
            .sources
            .iter()
            .any(|item| item.opaque_id == "event:budget-now")
    );
    assert_eq!(manifest.token_estimate, u64::MAX);
}

#[test]
fn granted_parent_current_input_survives_orientation_budget() {
    let store = FrozenContextStore::new();
    seed_basis(&store, "conversation:child-feed", 2);
    let parent_live = source(
        ContinuitySourceOwnerKind::Event,
        "event:parent-live",
        2,
        82,
        ContinuityVisibilityScope::Conversation,
    );
    let child_input = source(
        ContinuitySourceOwnerKind::Event,
        "event:child-feed-now",
        2,
        83,
        ContinuityVisibilityScope::Conversation,
    );
    let mut current = record(
        "conversation:child-feed",
        None,
        InformationClass::ConversationFact,
        child_input,
        3,
    );
    current.is_current_input = true;
    store.insert_record(current);
    let mut parent_current = record(
        "conversation:parent-feed",
        None,
        InformationClass::ConversationFact,
        parent_live.clone(),
        2,
    );
    parent_current.is_current_input = true;
    store.insert_record(parent_current);
    store.insert_grant(ContinuityParentContextGrant {
        grant_id: "grant:parent-live".into(),
        source_conversation_id: "conversation:parent-feed".into(),
        recipient_conversation_id: "conversation:child-feed".into(),
        recipient_membership_id: "membership:assistant".into(),
        source_refs: vec![parent_live],
        authorized_scopes: vec![ContinuityVisibilityScope::Conversation],
        status: ContinuityParentGrantStatus::Admitted,
        request_id: "request:parent-live".into(),
        revocation_generation: 0,
    });
    let (composer, _, _) = session(
        store,
        ScriptedAgent::new(),
        UnavailableKnowledgeService::default(),
    );
    let manifest = composer
        .compose_authorized(&request(
            "conversation:child-feed",
            "membership:assistant",
            0,
        ))
        .unwrap();
    assert!(
        manifest
            .sources
            .iter()
            .any(|item| item.opaque_id == "event:parent-live")
    );
}

#[test]
fn revoked_stored_source_cannot_be_projected_through_span_request() {
    let store = FrozenContextStore::new();
    seed_basis(&store, "conversation:child-revoke", 2);
    let child_input = source(
        ContinuitySourceOwnerKind::Event,
        "event:child-revoke-now",
        2,
        84,
        ContinuityVisibilityScope::Conversation,
    );
    let granted_span = spanned(
        source(
            ContinuitySourceOwnerKind::Event,
            "event:revoked-parent",
            2,
            85,
            ContinuityVisibilityScope::Conversation,
        ),
        0,
        20,
    );
    let mut revoked_base = source(
        ContinuitySourceOwnerKind::Event,
        "event:revoked-parent",
        2,
        85,
        ContinuityVisibilityScope::Conversation,
    );
    revoked_base.validity = ContinuitySourceValidity::Revoked;
    let mut current = record(
        "conversation:child-revoke",
        None,
        InformationClass::ConversationFact,
        child_input,
        3,
    );
    current.is_current_input = true;
    current.explicit_refs = vec![spanned(granted_span.clone(), 0, 8)];
    store.insert_record(current);
    store.insert_record(record(
        "conversation:parent-revoke",
        None,
        InformationClass::ConversationFact,
        revoked_base,
        1,
    ));
    store.insert_grant(ContinuityParentContextGrant {
        grant_id: "grant:revoked-span".into(),
        source_conversation_id: "conversation:parent-revoke".into(),
        recipient_conversation_id: "conversation:child-revoke".into(),
        recipient_membership_id: "membership:assistant".into(),
        source_refs: vec![granted_span],
        authorized_scopes: vec![ContinuityVisibilityScope::Conversation],
        status: ContinuityParentGrantStatus::Admitted,
        request_id: "request:revoked-span".into(),
        revocation_generation: 0,
    });
    let (composer, _, _) = session(
        store,
        ScriptedAgent::new(),
        UnavailableKnowledgeService::default(),
    );
    assert_eq!(
        composer
            .compose_authorized(&request(
                "conversation:child-revoke",
                "membership:assistant",
                0,
            ))
            .unwrap_err()
            .code,
        ContinuityFailureCode::SourceRevoked
    );
}
