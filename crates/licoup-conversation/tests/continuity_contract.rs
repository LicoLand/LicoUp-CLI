use licoup_conversation::continuity::{
    ContextCompositionPort, ContinuityCommitBasis, ContinuityCommitPort,
    ContinuityContextCompositionRequest, ContinuityFailure, ContinuityFailureCode,
    ContinuityGoalCompletionTransition, ContinuityGoalProgress, ContinuityMatter,
    ContinuityParentCardAnchor, ContinuityParentContextGrant, ContinuityParentGrantBasis,
    ContinuityReadPort, ContinuitySourceRef, ContinuityTaskChildAdmission,
    ContinuityTaskConversationRelation, ContinuityVisibilityScope, ContinuityWriteEnvelope,
    UnavailableContextComposition, UnavailableContinuityCommit, UnavailableContinuityRead,
    admit_card_identity_stable, admit_completion_transition, admit_composition_request,
    admit_goal_progress, admit_idempotency, admit_parent_context_grant, admit_sibling_card_order,
    admit_source_ref, admit_task_child_admission, admit_task_relation, admit_utf8_span,
    admit_versions, parse_continuity_value,
};
use licoup_conversation::{ConversationStore, Principal, PrincipalKind};
use serde_json::{Value, json};
use std::fs;
use std::path::{Path, PathBuf};

fn fixtures_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/fixtures/continuous-assistant/contracts")
}

fn read_fixture(relative: &str) -> Value {
    let path = fixtures_root().join(relative);
    serde_json::from_str(&fs::read_to_string(path).unwrap()).unwrap()
}

fn round_trip<T>(payload: &Value) -> Result<(), ContinuityFailure>
where
    T: for<'de> serde::Deserialize<'de> + serde::Serialize + PartialEq + std::fmt::Debug,
{
    let parsed: T = parse_continuity_value(payload)?;
    let encoded = serde_json::to_value(&parsed).expect("continuity serialize");
    let again: T = parse_continuity_value(&encoded)?;
    assert_eq!(parsed, again);
    assert_eq!(encoded, *payload);
    Ok(())
}

fn parse_named(type_name: &str, payload: &Value) -> Result<(), ContinuityFailure> {
    match type_name {
        "SourceRef" => round_trip::<ContinuitySourceRef>(payload),
        "Matter" => round_trip::<ContinuityMatter>(payload),
        "Agreement" => round_trip::<licoup_conversation::continuity::ContinuityAgreement>(payload),
        "GoalContract" => {
            round_trip::<licoup_conversation::continuity::ContinuityGoalContract>(payload)
        }
        "GoalProgress" => {
            round_trip::<licoup_conversation::continuity::ContinuityGoalProgress>(payload)
        }
        "InterpretationProposal" => {
            round_trip::<licoup_conversation::continuity::ContinuityInterpretationProposal>(payload)
        }
        "ContextManifest" => {
            round_trip::<licoup_conversation::continuity::ContinuityContextManifest>(payload)
        }
        "WorkContext" => {
            round_trip::<licoup_conversation::continuity::ContinuityWorkContext>(payload)
        }
        "Wake" => round_trip::<licoup_conversation::continuity::ContinuityWake>(payload),
        "QualificationRecord" => {
            round_trip::<licoup_conversation::continuity::ContinuityQualificationRecord>(payload)
        }
        "ParentCardAnchor" => round_trip::<ContinuityParentCardAnchor>(payload),
        "GoalCompletionTransition" => round_trip::<ContinuityGoalCompletionTransition>(payload),
        "ParentContextGrant" => round_trip::<ContinuityParentContextGrant>(payload),
        "TaskChildAdmission" => round_trip::<ContinuityTaskChildAdmission>(payload),
        "TaskConversationRelation" => round_trip::<ContinuityTaskConversationRelation>(payload),
        "ParentGrantBasis" => round_trip::<ContinuityParentGrantBasis>(payload),
        "ContextCompositionRequest" => round_trip::<ContinuityContextCompositionRequest>(payload),
        other => panic!("unknown fixture type {other}"),
    }
}

fn fixture_principal() -> Principal {
    Principal {
        id: "principal:human".into(),
        kind: PrincipalKind::Human,
        display_name: "Fixture".into(),
        agent_id: None,
        created_at_unix_ms: 1,
    }
}

#[test]
fn legal_fixtures_parse_on_generated_types() {
    let legal = fixtures_root().join("legal");
    let mut accepted = 0;
    for entry in fs::read_dir(legal).unwrap() {
        let path = entry.unwrap().path();
        if path.extension().and_then(|value| value.to_str()) != Some("json") {
            continue;
        }
        let fixture: Value = serde_json::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
        let expect = &fixture["expect"];
        if expect["check"] == "span" {
            admit_utf8_span(
                fixture["text"].as_str().unwrap(),
                fixture["payload"]["startByte"].as_u64().unwrap(),
                fixture["payload"]["endByte"].as_u64().unwrap(),
            )
            .unwrap();
            accepted += 1;
            continue;
        }
        if expect["check"] == "sibling-order" {
            let left: ContinuityParentCardAnchor =
                parse_continuity_value(&fixture["left"]).unwrap();
            let right: ContinuityParentCardAnchor =
                parse_continuity_value(&fixture["right"]).unwrap();
            admit_sibling_card_order(&left, &right).unwrap();
            accepted += 1;
            continue;
        }
        parse_named(expect["type"].as_str().unwrap(), &fixture["payload"]).unwrap();
        match expect["check"].as_str() {
            Some("task-child") => {
                let admission: ContinuityTaskChildAdmission =
                    parse_continuity_value(&fixture["payload"]).unwrap();
                admit_task_child_admission(&admission).unwrap();
            }
            Some("task-relation") => {
                let relation: ContinuityTaskConversationRelation =
                    parse_continuity_value(&fixture["payload"]).unwrap();
                admit_task_relation(&relation, None).unwrap();
            }
            Some("completion-transition") => {
                let transition: ContinuityGoalCompletionTransition =
                    parse_continuity_value(&fixture["payload"]).unwrap();
                let progress: ContinuityGoalProgress =
                    parse_continuity_value(&fixture["progress"]).unwrap();
                admit_completion_transition(&transition, &progress).unwrap();
            }
            Some("parent-grant") => {
                let grant: ContinuityParentContextGrant =
                    parse_continuity_value(&fixture["payload"]).unwrap();
                let requested: ContinuitySourceRef =
                    parse_continuity_value(&fixture["requested"]).unwrap();
                let basis: ContinuityParentGrantBasis =
                    parse_continuity_value(&fixture["basis"]).unwrap();
                admit_parent_context_grant(&grant, &requested, &basis).unwrap();
            }
            Some("composition-request") => {
                let request: ContinuityContextCompositionRequest =
                    parse_continuity_value(&fixture["payload"]).unwrap();
                admit_composition_request(&request).unwrap();
            }
            _ => {}
        }
        accepted += 1;
    }
    assert!(accepted >= 9, "legal fixtures accepted: {accepted}");
}

#[test]
fn illegal_fixtures_use_production_admission() {
    let unknown = read_fixture("illegal/unknown-field.json");
    let error = parse_named("Matter", &unknown["payload"]).unwrap_err();
    assert_eq!(error.code, ContinuityFailureCode::InvalidRequest);
    assert_eq!(
        error.effect_class,
        licoup_conversation::continuity::ContinuityEffectClass::None
    );

    let stale = read_fixture("illegal/stale-revision.json");
    let basis: ContinuityCommitBasis = serde_json::from_value(json!({
        "conversationId": stale["basis"]["conversationId"],
        "revision": stale["basis"]["revision"],
        "designationEpoch": stale["basis"]["designationEpoch"]
    }))
    .unwrap();
    let envelope: ContinuityWriteEnvelope = parse_continuity_value(&stale["payload"]).unwrap();
    assert_eq!(
        admit_versions(&basis, &envelope).unwrap_err().code,
        ContinuityFailureCode::StaleRevision
    );

    let epoch = read_fixture("illegal/wrong-epoch.json");
    let basis: ContinuityCommitBasis = serde_json::from_value(json!({
        "conversationId": epoch["basis"]["conversationId"],
        "revision": epoch["basis"]["revision"],
        "designationEpoch": epoch["basis"]["designationEpoch"]
    }))
    .unwrap();
    let envelope: ContinuityWriteEnvelope = parse_continuity_value(&epoch["payload"]).unwrap();
    assert_eq!(
        admit_versions(&basis, &envelope).unwrap_err().code,
        ContinuityFailureCode::DesignationChanged
    );

    let denied = read_fixture("illegal/denied-scope.json");
    let source: ContinuitySourceRef = parse_continuity_value(&denied["payload"]).unwrap();
    assert_eq!(
        admit_source_ref(&[ContinuityVisibilityScope::Conversation], &source)
            .unwrap_err()
            .code,
        ContinuityFailureCode::ScopeDenied
    );

    let span = read_fixture("illegal/invalid-span.json");
    assert_eq!(
        admit_utf8_span(
            span["text"].as_str().unwrap(),
            span["payload"]["startByte"].as_u64().unwrap(),
            span["payload"]["endByte"].as_u64().unwrap(),
        )
        .unwrap_err()
        .code,
        ContinuityFailureCode::InvalidSpan
    );

    let conflict = read_fixture("illegal/idempotency-conflict.json");
    assert_eq!(
        admit_idempotency(&conflict["previous"], &conflict["payload"])
            .unwrap_err()
            .code,
        ContinuityFailureCode::IdempotencyConflict
    );
}

#[test]
fn terminal_goal_progress_rejects_pending_attention_and_active_claims() {
    let mut payload = read_fixture("legal/goal-progress.json")["payload"].clone();
    payload["lifecycle"] = json!("achieved");
    let with_attention: ContinuityGoalProgress = parse_continuity_value(&payload).unwrap();
    assert_eq!(
        admit_goal_progress(&with_attention).unwrap_err().code,
        ContinuityFailureCode::InvalidRequest
    );

    payload
        .as_object_mut()
        .expect("goal progress object")
        .remove("nextAttention");
    let with_active_refs: ContinuityGoalProgress = parse_continuity_value(&payload).unwrap();
    assert_eq!(
        admit_goal_progress(&with_active_refs).unwrap_err().code,
        ContinuityFailureCode::InvalidRequest
    );

    payload["activeExecutionRefs"] = json!([]);
    let terminal: ContinuityGoalProgress = parse_continuity_value(&payload).unwrap();
    admit_goal_progress(&terminal).unwrap();
}

#[test]
fn rejected_real_commit_for_missing_conversation_has_no_business_effects() {
    let store = ConversationStore::open_in_memory().unwrap();
    let conversation = store
        .create_conversation("Fixture group", fixture_principal())
        .unwrap();
    store.ensure_continuity_migrated().unwrap();
    let before = store.get(&conversation.id).unwrap();
    let proposal = read_fixture("legal/interpretation-proposal.json");
    let parsed = parse_continuity_value(&proposal["payload"]).unwrap();
    let error = store.commit(&parsed).unwrap_err();
    assert_eq!(error.code, ContinuityFailureCode::ScopeDenied);
    assert_eq!(
        error.effect_class,
        licoup_conversation::continuity::ContinuityEffectClass::None
    );
    let after = store.get(&conversation.id).unwrap();
    assert_eq!(after.revision, before.revision);
    assert_eq!(after.event_count, before.event_count);

    let scope = store
        .prepare_runtime_dispatch(
            "fixture-agent",
            "",
            "ordinary dispatch",
            None,
            None,
            None,
            None,
        )
        .unwrap();
    assert!(store.dispatch_record(&scope.dispatch_id).unwrap().is_some());

    assert_eq!(
        UnavailableContinuityCommit
            .commit(&parsed)
            .unwrap_err()
            .code,
        ContinuityFailureCode::UnsupportedCapability
    );
    assert_eq!(
        UnavailableContinuityRead
            .relation_for_goal("goal:notes")
            .unwrap_err()
            .code,
        ContinuityFailureCode::UnsupportedCapability
    );
    assert_eq!(
        UnavailableContinuityRead
            .list_child_relations(&conversation.id, None, 20)
            .unwrap_err()
            .code,
        ContinuityFailureCode::UnsupportedCapability
    );
}

#[test]
fn child_conversation_admission_covers_legal_and_illegal_invariants() {
    let simple = read_fixture("illegal/simple-chat-child.json");
    let admission: ContinuityTaskChildAdmission =
        parse_continuity_value(&simple["payload"]).unwrap();
    assert_eq!(
        admit_task_child_admission(&admission).unwrap_err().code,
        ContinuityFailureCode::InvalidRequest
    );

    let conflict = read_fixture("illegal/identity-conflict.json");
    let previous: ContinuityTaskConversationRelation =
        parse_continuity_value(&conflict["previous"]).unwrap();
    let next: ContinuityTaskConversationRelation =
        parse_continuity_value(&conflict["payload"]).unwrap();
    assert_eq!(
        admit_task_relation(&next, Some(&previous))
            .unwrap_err()
            .code,
        ContinuityFailureCode::IdentityConflict
    );

    let moved = read_fixture("illegal/card-moved.json");
    let previous_card: ContinuityParentCardAnchor =
        parse_continuity_value(&moved["previous"]).unwrap();
    let next_card: ContinuityParentCardAnchor = parse_continuity_value(&moved["payload"]).unwrap();
    assert_eq!(
        admit_card_identity_stable(&previous_card, &next_card)
            .unwrap_err()
            .code,
        ContinuityFailureCode::IdentityConflict
    );

    let premature = read_fixture("illegal/premature-closure.json");
    let transition: ContinuityGoalCompletionTransition =
        parse_continuity_value(&premature["payload"]).unwrap();
    let progress: ContinuityGoalProgress = parse_continuity_value(&premature["progress"]).unwrap();
    assert_eq!(
        admit_completion_transition(&transition, &progress)
            .unwrap_err()
            .code,
        ContinuityFailureCode::PrematureClosure
    );

    let ungranted = read_fixture("illegal/ungranted-parent-ref.json");
    let grant: ContinuityParentContextGrant =
        parse_continuity_value(&ungranted["payload"]).unwrap();
    let requested: ContinuitySourceRef = parse_continuity_value(&ungranted["requested"]).unwrap();
    let basis: ContinuityParentGrantBasis = parse_continuity_value(&ungranted["basis"]).unwrap();
    assert_eq!(
        admit_parent_context_grant(&grant, &requested, &basis)
            .unwrap_err()
            .code,
        ContinuityFailureCode::ScopeDenied
    );

    let part_moved = read_fixture("illegal/card-part-moved.json");
    let previous_part: ContinuityParentCardAnchor =
        parse_continuity_value(&part_moved["previous"]).unwrap();
    let next_part: ContinuityParentCardAnchor =
        parse_continuity_value(&part_moved["payload"]).unwrap();
    assert_eq!(
        admit_card_identity_stable(&previous_part, &next_part)
            .unwrap_err()
            .code,
        ContinuityFailureCode::IdentityConflict
    );

    for name in [
        "illegal/grant-other-member.json",
        "illegal/grant-changed-revision.json",
        "illegal/grant-changed-digest.json",
        "illegal/grant-widened-span.json",
    ] {
        let fixture = read_fixture(name);
        let grant: ContinuityParentContextGrant =
            parse_continuity_value(&fixture["payload"]).unwrap();
        let requested: ContinuitySourceRef = parse_continuity_value(&fixture["requested"]).unwrap();
        let basis: ContinuityParentGrantBasis = parse_continuity_value(&fixture["basis"]).unwrap();
        assert_eq!(
            admit_parent_context_grant(&grant, &requested, &basis)
                .unwrap_err()
                .code,
            ContinuityFailureCode::ScopeDenied,
            "{name}"
        );
    }

    let stale_grant = read_fixture("illegal/grant-stale-generation.json");
    let grant: ContinuityParentContextGrant =
        parse_continuity_value(&stale_grant["payload"]).unwrap();
    let requested: ContinuitySourceRef = parse_continuity_value(&stale_grant["requested"]).unwrap();
    let basis: ContinuityParentGrantBasis = parse_continuity_value(&stale_grant["basis"]).unwrap();
    assert_eq!(
        admit_parent_context_grant(&grant, &requested, &basis)
            .unwrap_err()
            .code,
        ContinuityFailureCode::StaleRevision
    );

    let compose = read_fixture("legal/context-composition-request.json");
    let request: ContinuityContextCompositionRequest =
        parse_continuity_value(&compose["payload"]).unwrap();
    let compose_error = UnavailableContextComposition
        .compose_authorized(&request)
        .unwrap_err();
    assert_eq!(
        compose_error.code,
        ContinuityFailureCode::UnsupportedCapability
    );
    assert_eq!(
        compose_error.effect_class,
        licoup_conversation::continuity::ContinuityEffectClass::None
    );
    assert_eq!(
        UnavailableContinuityRead
            .list_parent_grants(
                "conversation:child-a",
                "membership:child-coordinator",
                None,
                20
            )
            .unwrap_err()
            .code,
        ContinuityFailureCode::UnsupportedCapability
    );

    let collision = read_fixture("illegal/sibling-sequence-collision.json");
    let left: ContinuityParentCardAnchor = parse_continuity_value(&collision["left"]).unwrap();
    let right: ContinuityParentCardAnchor = parse_continuity_value(&collision["right"]).unwrap();
    assert_eq!(
        admit_sibling_card_order(&left, &right).unwrap_err().code,
        ContinuityFailureCode::InvalidRequest
    );

    let reuse = read_fixture("legal/task-conversation-relation.json");
    let relation: ContinuityTaskConversationRelation =
        parse_continuity_value(&reuse["payload"]).unwrap();
    admit_task_relation(&relation, Some(&relation)).unwrap();
}

#[test]
fn store_exposes_usable_unit_of_work_without_m0_commit() {
    let store = ConversationStore::open_in_memory().unwrap();
    let conversation = store
        .create_conversation("Fixture group", fixture_principal())
        .unwrap();
    let basis = store.continuity_commit_basis(&conversation.id).unwrap();
    assert_eq!(basis.conversation_id, conversation.id);
    assert_eq!(basis.designation_epoch, 0);
    store
        .with_continuity_unit_of_work(|unit| {
            let revision: i64 = unit.query_row(
                "SELECT revision FROM conversations WHERE id=?1",
                [conversation.id.as_str()],
                |row| row.get(0),
            )?;
            assert_eq!(revision, conversation.revision);
            Ok(())
        })
        .unwrap();
}
