#![cfg(feature = "test-support")]
//! M4 adoption: persisted policy, trusted live admission, and offline migration.

use licoup_agent_runtime::work_context::ProtocolFamily;
use licoup_conversation::ConversationStore;
use licoup_conversation::ProfileIntentUpdate;
use licoup_conversation::continuity::migrate::CONTINUITY_SCHEMA_VERSION;
use licoup_conversation::continuity::{
    ContinuityCandidateIdentity, ContinuityDatasetSplit, ContinuityEffectStatus,
    ContinuityFailureCode, ContinuityGoalControl, ContinuityGoalLifecycle,
    ContinuityQualificationResult, ContinuityReadPort, EvaluationCasePolarity,
    EvaluationExpectedAction, StoredEvaluationCase, list_qualification_evidence,
    list_unknown_effect_ids, load_adoption_policy_values, load_effect_status,
    load_evaluation_session, put_effect, put_qualification_evidence, read_goal,
};
use licoup_conversation::continuity::{
    ContinuityCommitmentProposal, ContinuityFollowThroughKind, ContinuityInterpretationProposal,
    ContinuityMatterSubject, ContinuitySourceOwnerKind, ContinuitySourceRef,
    ContinuitySourceValidity, ContinuitySpeechAct, ContinuityVisibilityScope,
    ContinuityWriteEnvelope,
};
use licoup_native::domain::agent_intelligence_catalog::qualification::{
    EvidenceBundle, EvidenceClass, LiveAdmission, LiveProvenance, ObservationJudgment,
    QualificationObservation, QualificationPolicy, RequestKind, SYNTHETIC_TEST_EVIDENCE_LABEL,
    SyntheticRecipe, evaluate_bundle, generate_synthetic,
};
use licoup_native::domain::assistant_continuity::collection::{
    QUALIFICATION_EVALUATION_KIND, STORED_RECEIPT_COLLECTOR_ID, collection_effect_id,
};
use licoup_native::domain::assistant_continuity::{
    AdoptionStage, ContinuityHost, cognition::SemanticScript, cognition::SpanAxis,
};
use licoup_native::domain::client_conversation::{ConversationService, PersistentRuntimePorts};
use licoup_native::platform::runtime_adapters::RuntimeAdapterError;
use licoup_native::platform::work_context_ports::{
    AdapterCall, AdapterTransport, host_driver_transport,
};
use serde_json::{Value, json};
use std::sync::{Arc, Mutex};

fn emit(label: &str, payload: Value) {
    println!("CONTINUITY_ADOPTION_ORACLE:{label}:{payload}");
}

fn fixture_identity() -> ContinuityCandidateIdentity {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/fixtures/continuous-assistant/qualification/candidate-identity.json");
    let payload: Value = serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
    serde_json::from_value(payload["candidateIdentity"].clone()).unwrap()
}

fn test_provenance() -> LiveProvenance {
    LiveProvenance::labeled_test_evidence("test:adoption-boundary", "draft-1")
}

fn passing_bundle(responsibility: String, identity: ContinuityCandidateIdentity) -> EvidenceBundle {
    let policy = QualificationPolicy::draft_1();
    let observations = generate_synthetic(
        &SyntheticRecipe {
            negative_families: 300,
            positive_families: 300,
            false_takeovers: 0,
            missed_commitments: 0,
            abstentions: 0,
            split: ContinuityDatasetSplit::Heldout,
            family_prefix: "fam:adopt-pass".into(),
        },
        &policy.required_subgroups,
    )
    .unwrap();
    EvidenceBundle::from_fixture_parts(responsibility, identity, observations)
}

const FIXTURE_EVALUATION_DATASET: &str = "fixture:admitted-session-cases/v1";

fn fixture_evaluation_cases() -> Vec<StoredEvaluationCase> {
    vec![
        StoredEvaluationCase {
            case_id: "case:chitchat".into(),
            family: "fam:admitted-session:chitchat".into(),
            subgroup: "en".into(),
            polarity: EvaluationCasePolarity::Negative,
            expected_action: EvaluationExpectedAction::Abstain,
            input: "ordinary thanks, just chatting".into(),
        },
        StoredEvaluationCase {
            case_id: "case:prepare-notes".into(),
            family: "fam:admitted-session:prepare-notes".into(),
            subgroup: "non-coding".into(),
            polarity: EvaluationCasePolarity::Positive,
            expected_action: EvaluationExpectedAction::Takeover,
            input: "PRODUCTION-GRANT-SENTINEL prepare notes".into(),
        },
    ]
}

fn admit_fixture_corpus(host: &ContinuityHost, conversation_id: &str, owner: &str) -> String {
    host.admit_live_evaluation_corpus(
        conversation_id,
        owner,
        FIXTURE_EVALUATION_DATASET,
        fixture_evaluation_cases(),
    )
    .unwrap()
    .version_digest
}

fn host_identity(agent: &str) -> ContinuityCandidateIdentity {
    ContinuityCandidateIdentity {
        model_digest: "identity:unselected-model".into(),
        reasoning_digest: "identity:unselected-reasoning".into(),
        prompt_digest: format!("identity:prompt:{agent}:0"),
        skill_digest: "identity:unselected-skills".into(),
        context_policy_digest: "identity:unselected-context".into(),
        tool_contract_digest: "identity:unselected-tools".into(),
        adapter_runtime_digest: "identity:unselected-adapter".into(),
        dataset_version: format!("identity:membership:{agent}:one"),
        policy_revision: "draft-1".into(),
    }
}

fn create_group(service: &ConversationService) -> (String, String, String) {
    let group = service
        .execute(json!({
            "action": "conversation.create",
            "title": "Adoption parent",
            "owner": {"id": "human:local", "kind": "human", "displayName": "You"},
            "members": [{
                "principal": {
                    "id": "agent:one",
                    "kind": "agent",
                    "displayName": "One",
                    "agentId": "one"
                },
                "access": "member"
            }]
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

fn create_group_two_agents(service: &ConversationService) -> (String, String, String, String) {
    let group = service
        .execute(json!({
            "action": "conversation.create",
            "title": "Adoption parent two",
            "owner": {"id": "human:local", "kind": "human", "displayName": "You"},
            "members": [
                {
                    "principal": {
                        "id": "agent:one",
                        "kind": "agent",
                        "displayName": "One",
                        "agentId": "one"
                    },
                    "access": "member"
                },
                {
                    "principal": {
                        "id": "agent:two",
                        "kind": "agent",
                        "displayName": "Two",
                        "agentId": "two"
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
    let first = memberships
        .iter()
        .find(|membership| membership["principal"]["agentId"] == "one")
        .unwrap()["id"]
        .as_str()
        .unwrap()
        .to_owned();
    let second = memberships
        .iter()
        .find(|membership| membership["principal"]["agentId"] == "two")
        .unwrap()["id"]
        .as_str()
        .unwrap()
        .to_owned();
    (
        group["id"].as_str().unwrap().to_owned(),
        owner,
        first,
        second,
    )
}

fn bind_passing_observer(host: &ContinuityHost, prefix: &str) {
    let policy = QualificationPolicy::draft_1();
    let observations = generate_synthetic(
        &SyntheticRecipe {
            negative_families: 300,
            positive_families: 300,
            false_takeovers: 0,
            missed_commitments: 0,
            abstentions: 0,
            split: ContinuityDatasetSplit::Heldout,
            family_prefix: prefix.into(),
        },
        &policy.required_subgroups,
    )
    .unwrap();
    host.bind_hermetic_evaluation_observer(observations);
}

fn collect_with_producer(host: &ContinuityHost, session_id: &str, prefix: &str) {
    bind_passing_observer(host, prefix);
    host.collect_admitted_qualification(session_id).unwrap();
}

fn digest(tag: u8) -> String {
    format!("sha256:{:02x}{}", tag, "ab".repeat(31))
}

fn durable_script(event_id: &str) -> SemanticScript {
    SemanticScript {
        event_opaque_id: event_id.to_owned(),
        axes: vec![SpanAxis {
            source_ref: ContinuitySourceRef {
                owner_kind: ContinuitySourceOwnerKind::Event,
                opaque_id: event_id.to_owned(),
                part_id: None,
                span: None,
                source_revision: 1,
                digest: digest(1),
                visibility_scope: ContinuityVisibilityScope::Conversation,
                validity: ContinuitySourceValidity::Current,
            },
            subject: ContinuityMatterSubject::New,
            speech_act: ContinuitySpeechAct::Delegation,
            follow_through: ContinuityFollowThroughKind::Durable,
            create_goal: true,
            matter_id: Some("matter:adoption".into()),
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

#[test]
fn ac_05_001_default_policy_is_enabled_offline_and_blocks_automatic() {
    let service = ConversationService::from_store(ConversationStore::open_in_memory().unwrap());
    let host = service.continuity().cloned().unwrap();
    let (conversation_id, owner, agent) = create_group(&service);
    let policy = host.adoption_policy();
    assert!(policy.enabled);
    assert_eq!(policy.stage, AdoptionStage::Offline);
    let automatic = host.admit_request(&conversation_id, &agent, RequestKind::AutomaticAdvancement);
    assert_eq!(
        automatic.unwrap_err().code,
        ContinuityFailureCode::QualificationUnknown
    );
    let explicit = host.admit_request(&conversation_id, &agent, RequestKind::ExplicitlyRequested);
    assert!(explicit.is_ok());
    let gotten = service
        .execute(json!({
            "action": "conversation.get",
            "conversationId": conversation_id
        }))
        .unwrap();
    assert_eq!(gotten["adoptionPolicy"]["enabled"], true);
    assert_eq!(gotten["adoptionPolicy"]["stage"], "offline");
    assert_eq!(
        gotten["adoptionPolicy"]["realModelQualification"],
        "unknown"
    );
    let stored = load_adoption_policy_values(service.store()).unwrap();
    assert!(stored.0);
    assert_eq!(stored.1, "offline");
    emit(
        "default-policy",
        json!({
            "enabled": policy.enabled,
            "stage": "offline",
            "automaticDenied": true,
            "explicitRetained": true,
            "realModelQualification": "unknown",
            "ownerPresent": !owner.is_empty()
        }),
    );
}

#[test]
fn ac_05_001_owner_disable_persists_and_blocks_only_new_automatic() {
    let root = std::env::temp_dir().join(format!("lico-ca-adopt-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&root).unwrap();
    let service = ConversationService::from_store(ConversationStore::open(&root).unwrap());
    let host = service.continuity().cloned().unwrap();
    let (conversation_id, owner, agent) = create_group(&service);
    host.install_script(durable_script(""));
    let posted = service
        .execute(json!({
            "action": "conversation.message.post",
            "conversationId": conversation_id,
            "authorMembershipId": owner,
            "content": "Please prepare notes"
        }))
        .unwrap();
    assert_eq!(posted["continuityIngress"]["committed"], true);
    let relations = host
        .store()
        .list_child_relations(&conversation_id, None, 8)
        .unwrap();
    let goal_id = relations[0].goal_id.clone();
    host.pause_goal(&conversation_id, &goal_id).unwrap();
    put_effect(
        host.store(),
        &conversation_id,
        Some(&goal_id),
        "effect:adoption-unknown",
        ContinuityEffectStatus::Unknown,
    )
    .unwrap();
    let paused = read_goal(host.store(), &goal_id).unwrap().unwrap();
    assert_eq!(paused.lifecycle, ContinuityGoalLifecycle::Active);
    assert_eq!(paused.control, ContinuityGoalControl::Paused);

    let stranger = host.set_adoption_enabled(&conversation_id, "membership:missing", false);
    assert_eq!(
        stranger.unwrap_err().code,
        ContinuityFailureCode::ScopeDenied
    );

    let disabled = service
        .execute(json!({
            "action": "set-adoption-enabled",
            "conversationId": conversation_id,
            "ownerMembershipId": owner,
            "enabled": false
        }))
        .unwrap();
    assert_eq!(disabled["adoptionPolicy"]["enabled"], false);

    let automatic = host.admit_request(&conversation_id, &agent, RequestKind::AutomaticAdvancement);
    assert_eq!(
        automatic.unwrap_err().code,
        ContinuityFailureCode::QualificationUnknown
    );
    assert!(
        host.admit_request(&conversation_id, &agent, RequestKind::ExplicitlyRequested)
            .is_ok()
    );

    let reopened = ConversationService::from_store(ConversationStore::open(&root).unwrap());
    let restored = reopened.continuity().cloned().unwrap();
    assert!(!restored.adoption_policy().enabled);
    let after = read_goal(restored.store(), &goal_id).unwrap().unwrap();
    assert_eq!(after.control, ContinuityGoalControl::Paused);
    assert_eq!(after.lifecycle, ContinuityGoalLifecycle::Active);
    let unknown = list_unknown_effect_ids(restored.store()).unwrap();
    assert!(
        unknown
            .iter()
            .any(|(id, _, _)| id == "effect:adoption-unknown")
    );
    restored.request_cancel(&conversation_id, &goal_id).unwrap();
    let cancelling = read_goal(restored.store(), &goal_id).unwrap().unwrap();
    assert_eq!(cancelling.control, ContinuityGoalControl::CancelRequested);
    emit(
        "disable-persist",
        json!({
            "disabledPersisted": !restored.adoption_policy().enabled,
            "goalPreserved": after.control == ContinuityGoalControl::Paused,
            "unknownPreserved": true,
            "pauseDistinctFromCancel": cancelling.control
                == ContinuityGoalControl::CancelRequested,
            "nonOwnerDenied": true,
            "explicitRetained": true
        }),
    );
}

#[test]
fn ac_05_001_live_admission_persists_provenance_and_advances_stage() {
    let root = std::env::temp_dir().join(format!("lico-ca-live-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&root).unwrap();
    let service = ConversationService::from_store(ConversationStore::open(&root).unwrap());
    let host = service.continuity().cloned().unwrap();
    let (conversation_id, _owner, agent) = create_group(&service);
    let identity = host_identity(&agent);
    let responsibility = format!("{conversation_id}:{agent}");
    let synthetic = passing_bundle(responsibility.clone(), identity.clone());
    assert_eq!(
        evaluate_bundle(&synthetic, &QualificationPolicy::draft_1(), false).result,
        ContinuityQualificationResult::Unknown
    );

    let renamed = EvidenceBundle {
        evidence_class: EvidenceClass::LiveAuthorized,
        provenance: None,
        ..synthetic.clone()
    };
    assert!(host.ingest_test_qualification(renamed).is_err());

    let mismatch =
        LiveAdmission::admit_test_evidence(fixture_identity(), test_provenance()).unwrap();
    assert!(synthetic.clone().authorize_live(mismatch).is_err());

    let admission =
        LiveAdmission::admit_test_evidence(identity.clone(), test_provenance()).unwrap();
    host.ingest_test_live_qualification(synthetic, admission)
        .unwrap();
    assert_eq!(host.adoption_policy().stage, AdoptionStage::AdmittedShadow);

    let reopened = ConversationService::from_store(ConversationStore::open(&root).unwrap());
    let restored = reopened.continuity().cloned().unwrap();
    assert_eq!(
        restored.adoption_policy().stage,
        AdoptionStage::AdmittedShadow
    );
    let rows =
        licoup_conversation::continuity::list_qualification_evidence(restored.store()).unwrap();
    assert!(rows.iter().any(|(_, _, payload, class)| {
        class == "live-authorized"
            && payload.contains(SYNTHETIC_TEST_EVIDENCE_LABEL)
            && payload.contains("synthetic-test-boundary")
    }));
    let automatic =
        restored.admit_request(&conversation_id, &agent, RequestKind::AutomaticAdvancement);
    assert_eq!(
        automatic.unwrap_err().code,
        ContinuityFailureCode::QualificationUnknown
    );
    emit(
        "live-admission",
        json!({
            "renamedSyntheticRejected": true,
            "identityMismatchRejected": true,
            "stage": "admitted-shadow",
            "reloadKeptLive": true,
            "labeledSyntheticTestEvidence": true,
            "testEvidenceDoesNotPromoteQualification": true,
            "realModelQualification": "unknown",
            "automaticAdmittedByMechanism": false
        }),
    );
}

#[test]
fn ac_05_001_owner_issued_live_requires_stored_authority() {
    let root = std::env::temp_dir().join(format!("lico-ca-owner-live-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&root).unwrap();
    let service = ConversationService::from_store(ConversationStore::open(&root).unwrap());
    let host = service.continuity().cloned().unwrap();
    let (conversation_id, owner, agent) = create_group(&service);
    let identity = host_identity(&agent);
    let responsibility = format!("{conversation_id}:{agent}");
    let synthetic = passing_bundle(responsibility, identity);
    let spoofed = EvidenceBundle {
        evidence_class: EvidenceClass::LiveAuthorized,
        provenance: None,
        ..synthetic
    };
    assert!(
        host.ingest_test_qualification(spoofed).is_err(),
        "owner-valid synthetic import must stay rejected"
    );
    assert!(
        host.collect_admitted_qualification("evaluation-session:missing")
            .is_err()
    );
    let stranger =
        host.admit_live_evaluation_session(&conversation_id, "membership:missing", &agent);
    assert_eq!(
        stranger.unwrap_err().code,
        ContinuityFailureCode::ScopeDenied
    );
    let session_id = host
        .admit_live_evaluation_session(&conversation_id, &owner, &agent)
        .unwrap();
    assert_eq!(
        host.collect_admitted_qualification(&session_id)
            .unwrap_err()
            .code,
        ContinuityFailureCode::SourceUnavailable,
        "production producer without replaced observer cannot fabricate Qualified"
    );
    assert_eq!(host.adoption_policy().stage, AdoptionStage::Offline);
    collect_with_producer(&host, &session_id, "fam:owner-session");
    assert_eq!(
        host.adoption_policy().stage,
        AdoptionStage::QualifiedLowRisk
    );
    assert!(
        host.admit_request(&conversation_id, &agent, RequestKind::AutomaticAdvancement)
            .is_ok()
    );

    let reopened = ConversationService::from_store(ConversationStore::open(&root).unwrap());
    let restored = reopened.continuity().cloned().unwrap();
    assert_eq!(
        restored.adoption_policy().stage,
        AdoptionStage::QualifiedLowRisk
    );
    restored
        .store()
        .archive_conversation(&conversation_id, true)
        .unwrap();
    let revoked = ConversationService::from_store(ConversationStore::open(&root).unwrap());
    let after_revoke = revoked.continuity().cloned().unwrap();
    assert_ne!(
        after_revoke.adoption_policy().stage,
        AdoptionStage::QualifiedLowRisk
    );
    assert_eq!(
        after_revoke
            .admit_request(&conversation_id, &agent, RequestKind::AutomaticAdvancement)
            .unwrap_err()
            .code,
        ContinuityFailureCode::QualificationUnknown
    );
    emit(
        "owner-issued-live",
        json!({
            "spoofedSyntheticImportRejected": true,
            "collectWithoutSessionRejected": true,
            "productionProducerWithoutObserverUnavailable": true,
            "nonOwnerDenied": true,
            "stage": "qualified-low-risk",
            "automaticAdmittedByStoredOwner": true,
            "reloadRevalidated": true,
            "revocationDroppedLive": after_revoke.adoption_policy().stage
                != AdoptionStage::QualifiedLowRisk,
            "realModelQualification": "unknown"
        }),
    );
}

#[test]
fn ac_05_002_migration_is_idempotent_and_preserves_facts() {
    let root = std::env::temp_dir().join(format!("lico-ca-mig-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&root).unwrap();
    let store = ConversationStore::open(&root).unwrap();
    store.ensure_continuity_migrated().unwrap();
    let first = load_adoption_policy_values(&store).unwrap();
    assert!(first.0);
    assert_eq!(first.1, "offline");
    put_effect(
        &store,
        "conversation:missing",
        None,
        "effect:migration-unknown",
        ContinuityEffectStatus::Unknown,
    )
    .unwrap();
    store.ensure_continuity_migrated().unwrap();
    let second = load_adoption_policy_values(&store).unwrap();
    assert_eq!(first, second);
    let unknown = list_unknown_effect_ids(&store).unwrap();
    assert!(
        unknown
            .iter()
            .any(|(id, _, _)| id == "effect:migration-unknown")
    );
    let host = ContinuityHost::attach(store.clone()).unwrap();
    assert_eq!(host.adoption_policy().stage, AdoptionStage::Offline);
    emit(
        "migration",
        json!({
            "schemaVersion": CONTINUITY_SCHEMA_VERSION,
            "idempotent": first == second,
            "unknownPreserved": true,
            "noHistoricalExecutionCreated": host.unknown_effect_ids().len() == 1,
            "realModelQualification": "unknown"
        }),
    );
}

#[test]
fn ac_05_001_within_process_revoke_denies_automatic_without_reopen() {
    let service = ConversationService::from_store(ConversationStore::open_in_memory().unwrap());
    let host = service.continuity().cloned().unwrap();
    let (conversation_id, owner, agent) = create_group(&service);
    let session_id = host
        .admit_live_evaluation_session(&conversation_id, &owner, &agent)
        .unwrap();
    collect_with_producer(&host, &session_id, "fam:revoke-same-process");
    assert!(
        host.admit_request(&conversation_id, &agent, RequestKind::AutomaticAdvancement)
            .is_ok()
    );
    host.store()
        .archive_conversation(&conversation_id, true)
        .unwrap();
    assert_eq!(
        host.admit_request(&conversation_id, &agent, RequestKind::AutomaticAdvancement)
            .unwrap_err()
            .code,
        ContinuityFailureCode::QualificationUnknown
    );
    assert!(
        host.admit_request(&conversation_id, &agent, RequestKind::ExplicitlyRequested)
            .is_ok()
    );
    emit(
        "within-process-revoke",
        json!({
            "automaticDeniedWithoutReopen": true,
            "explicitRetained": true,
            "realModelQualification": "unknown"
        }),
    );
}

#[test]
fn ac_05_001_same_responsibility_multi_identity_cannot_expand() {
    let service = ConversationService::from_store(ConversationStore::open_in_memory().unwrap());
    let host = service.continuity().cloned().unwrap();
    let (conversation_id, owner, agent) = create_group(&service);
    let first = host
        .admit_live_evaluation_session(&conversation_id, &owner, &agent)
        .unwrap();
    collect_with_producer(&host, &first, "fam:same-resp-a");
    assert_eq!(
        host.adoption_policy().stage,
        AdoptionStage::QualifiedLowRisk
    );
    host.store()
        .set_membership_profile(
            &conversation_id,
            &agent,
            &owner,
            0,
            &ProfileIntentUpdate {
                preferred_model: Some("other-model".into()),
                ..ProfileIntentUpdate::default()
            },
        )
        .unwrap();
    let second = host
        .admit_live_evaluation_session(&conversation_id, &owner, &agent)
        .unwrap();
    collect_with_producer(&host, &second, "fam:same-resp-b");
    assert_eq!(
        host.adoption_policy().stage,
        AdoptionStage::QualifiedLowRisk
    );
    emit(
        "same-responsibility-multi-identity",
        json!({
            "stage": "qualified-low-risk",
            "sameResponsibilityDoesNotExpand": true,
            "realModelQualification": "unknown"
        }),
    );
}

#[test]
fn ac_05_001_distinct_responsibility_qualification_expands() {
    let service = ConversationService::from_store(ConversationStore::open_in_memory().unwrap());
    let host = service.continuity().cloned().unwrap();
    let (conversation_id, owner, first, second) = create_group_two_agents(&service);
    let session_one = host
        .admit_live_evaluation_session(&conversation_id, &owner, &first)
        .unwrap();
    collect_with_producer(&host, &session_one, "fam:distinct-a");
    assert_eq!(
        host.adoption_policy().stage,
        AdoptionStage::QualifiedLowRisk
    );
    let session_two = host
        .admit_live_evaluation_session(&conversation_id, &owner, &second)
        .unwrap();
    collect_with_producer(&host, &session_two, "fam:distinct-b");
    assert_eq!(host.adoption_policy().stage, AdoptionStage::Expanded);
    emit(
        "distinct-responsibility-expands",
        json!({
            "stage": "expanded",
            "distinctResponsibilitiesExpand": true,
            "realModelQualification": "unknown"
        }),
    );
}

#[test]
fn ac_05_001_successful_collection_once_replay_cannot_mutate() {
    let service = ConversationService::from_store(ConversationStore::open_in_memory().unwrap());
    let host = service.continuity().cloned().unwrap();
    let (conversation_id, owner, agent) = create_group(&service);
    let session_id = host
        .admit_live_evaluation_session(&conversation_id, &owner, &agent)
        .unwrap();
    collect_with_producer(&host, &session_id, "fam:collect-once");
    let before = list_qualification_evidence(host.store()).unwrap();
    let stage = host.adoption_policy().stage;
    bind_passing_observer(&host, "fam:collect-replay");
    assert_eq!(
        host.collect_admitted_qualification(&session_id)
            .unwrap_err()
            .code,
        ContinuityFailureCode::IdempotencyConflict
    );
    let after = list_qualification_evidence(host.store()).unwrap();
    assert_eq!(before, after);
    assert_eq!(host.adoption_policy().stage, stage);
    let stored = load_evaluation_session(host.store(), &session_id)
        .unwrap()
        .unwrap();
    assert!(stored.consumed);
    emit(
        "successful-collection-once",
        json!({
            "replayRejected": true,
            "dbUnchanged": before == after,
            "cacheUnchanged": host.adoption_policy().stage == stage,
            "sessionConsumed": stored.consumed,
            "realModelQualification": "unknown"
        }),
    );
}

fn compile_fake_codex() -> std::path::PathBuf {
    use std::process::Command;
    use std::time::{SystemTime, UNIX_EPOCH};

    let fixture = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
        .join("fake_codex_app_server.rs");
    let suffix = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let temp_dir = std::env::temp_dir().join(format!("lico-ca-eval-fake-codex-{suffix}"));
    std::fs::create_dir_all(&temp_dir).unwrap();
    let executable = temp_dir.join(format!("fake-codex{}", std::env::consts::EXE_SUFFIX));
    let rustc = std::env::var("RUSTC").unwrap_or_else(|_| "rustc".to_string());
    let compile = Command::new(rustc)
        .arg("--edition=2024")
        .arg(&fixture)
        .arg("-o")
        .arg(&executable)
        .status()
        .expect("fake Codex fixture should compile");
    assert!(compile.success(), "fake Codex fixture failed to compile");
    executable
}

fn typed_takeover_output(conversation_id: &str) -> String {
    serde_json::to_string(&ContinuityInterpretationProposal {
        envelope: ContinuityWriteEnvelope {
            conversation_id: conversation_id.to_owned(),
            source_event_refs: Vec::new(),
            observed_revision: 0,
            designation_epoch: 0,
            request_id: "request:eval:prepare-notes".into(),
        },
        matter_associations: Vec::new(),
        speech_act: ContinuitySpeechAct::Delegation,
        commitment_proposals: vec![ContinuityCommitmentProposal {
            matter_id: Some("matter:notes".into()),
            subject: ContinuityMatterSubject::New,
            expected_result: "Prepare notes".into(),
            criteria: Vec::new(),
            create_goal: true,
        }],
        agreement_proposals: Vec::new(),
        capability_needs: Vec::new(),
        uncertainty_reasons: Vec::new(),
        requested_reads: Vec::new(),
        task_child_admission: None,
    })
    .unwrap()
}

fn create_codex_group(service: &ConversationService) -> (String, String, String) {
    let group = service
        .execute(json!({
            "action": "conversation.create",
            "title": "Evaluation parent",
            "owner": {"id": "human:local", "kind": "human", "displayName": "You"},
            "members": [{
                "principal": {
                    "id": "agent:codex",
                    "kind": "agent",
                    "displayName": "Codex",
                    "agentId": "codex"
                },
                "access": "member"
            }]
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

fn candidate_params_leak_expected_label(params: &Value) -> bool {
    let dumped = params.to_string().to_ascii_lowercase();
    [
        "abstain",
        "takeover",
        "negative",
        "positive",
        "expected",
        "judgment",
        "polarity",
        "false-takeover",
        "missed-commitment",
    ]
    .iter()
    .any(|token| dumped.contains(token))
}

fn bind_native_complete_turn(
    service: ConversationService,
    complete_turn: impl Fn(&Value) -> Result<Value, RuntimeAdapterError> + Send + Sync + 'static,
) -> ConversationService {
    service.bind_conversation_runtime(PersistentRuntimePorts::new(
        |_params: &Value| Ok(json!({ "ok": true, "accepted": true })),
        |_conversation_id: &str| json!([]),
        |_params: &Value| Ok(json!({ "ok": true })),
        complete_turn,
        |_request: Value| Ok(json!({})),
    ))
}

#[test]
fn ac_05_001_bound_runtime_collects_typed_observations_without_hermetic_observer() {
    let executable = compile_fake_codex();
    let cwd = std::env::temp_dir().join(format!("lico-ca-eval-cwd-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&cwd).unwrap();
    let root = std::env::temp_dir().join(format!("lico-ca-bound-runtime-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&root).unwrap();
    let store = ConversationStore::open(&root).unwrap();
    let invoked = Arc::new(Mutex::new(Vec::<Value>::new()));
    let invoked_for_turn = invoked.clone();
    let transport = Arc::new(
        host_driver_transport(ProtocolFamily::Codex)
            .with_executable(executable.to_string_lossy().into_owned())
            .with_working_directory(cwd.clone()),
    );
    let transport_for_turn = transport.clone();
    let service = bind_native_complete_turn(
        ConversationService::from_store(store),
        move |params: &Value| {
            invoked_for_turn
                .lock()
                .unwrap_or_else(|poison| poison.into_inner())
                .push(params.clone());
            let response = transport_for_turn.invoke(&AdapterCall {
                method: "thread/start",
                params: params.clone(),
            });
            if !response.ok {
                return Err(RuntimeAdapterError::ConversationDispatchFailed);
            }
            let output = response
                .result
                .get("output")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_owned();
            Ok(json!({
                "ok": true,
                "output": output,
                "conversationId": params.get("conversationId"),
                "evaluationSessionId": params.get("evaluationSessionId"),
                "responsibilityId": params.get("responsibilityId"),
                "membershipId": params.get("membershipId"),
                "policyRevision": params.get("policyRevision"),
            }))
        },
    );
    let host = service.continuity().cloned().unwrap();
    let (conversation_id, owner, agent) = create_codex_group(&service);
    let mut result_path = executable.clone();
    result_path.set_extension("result.json");
    std::fs::write(&result_path, typed_takeover_output(&conversation_id)).unwrap();
    let corpus_version = admit_fixture_corpus(&host, &conversation_id, &owner);
    let session_id = host
        .admit_live_evaluation_session(&conversation_id, &owner, &agent)
        .unwrap();
    let stored_before = load_evaluation_session(host.store(), &session_id)
        .unwrap()
        .unwrap();
    assert_eq!(stored_before.dataset_id, FIXTURE_EVALUATION_DATASET);
    assert_eq!(stored_before.corpus_version, corpus_version);
    assert_eq!(stored_before.identity.dataset_version, corpus_version);
    host.collect_admitted_qualification(&session_id)
        .expect("bound admitted runtime must collect without a hermetic observer");
    assert_eq!(host.adoption_policy().stage, AdoptionStage::AdmittedShadow);
    assert!(
        host.admit_request(&conversation_id, &agent, RequestKind::AutomaticAdvancement)
            .is_err()
    );

    let calls = invoked
        .lock()
        .unwrap_or_else(|poison| poison.into_inner())
        .clone();
    assert_eq!(calls.len(), 2, "each admitted case must invoke the runtime");
    assert!(
        calls.iter().all(|params| {
            params["evaluationSessionId"] == session_id
                && params["conversationId"] == conversation_id
                && params["membershipId"] == agent
                && params["responsibilityId"] == format!("{conversation_id}:{agent}")
                && params["continuityKind"] == QUALIFICATION_EVALUATION_KIND
                && params["datasetId"] == FIXTURE_EVALUATION_DATASET
                && params["datasetVersion"] == corpus_version
                && params["agentId"] == "codex"
        }),
        "native invocation must carry the exact admitted session, candidate, and materials"
    );
    assert!(
        calls.iter().any(|params| {
            params["text"]
                .as_str()
                .is_some_and(|text| text.contains("PRODUCTION-GRANT-SENTINEL"))
        }),
        "positive case input must reach the admitted runtime"
    );
    assert!(
        calls.iter().any(|params| {
            params["text"]
                .as_str()
                .is_some_and(|text| text.contains("ordinary thanks, just chatting"))
        }),
        "chitchat case input must reach the admitted runtime"
    );
    assert!(
        calls
            .iter()
            .all(|params| !candidate_params_leak_expected_label(params)),
        "expected labels must stay evaluator-private"
    );
    assert!(
        transport.invocation_count() >= 2,
        "lowest fake process must be invoked for admitted cases"
    );
    let mut seen = executable.clone();
    seen.set_extension("turn-start.seen");
    assert_eq!(
        std::fs::read_to_string(&seen).unwrap_or_default().trim(),
        "1",
        "lowest fake process must observe the granted sentinel"
    );
    let mut leak = executable.clone();
    leak.set_extension("leak.seen");
    assert!(!leak.exists(), "out-of-scope material must not leak");

    let rows = list_qualification_evidence(host.store()).unwrap();
    let live = rows
        .iter()
        .find(|(_, _, _, class)| class == "live-authorized")
        .expect("collected evidence must persist");
    let payload: Value = serde_json::from_str(&live.2).unwrap();
    let observations: Vec<QualificationObservation> =
        serde_json::from_value(payload["observations"].clone()).unwrap();
    assert_eq!(observations.len(), 2);
    assert_eq!(observations[0].judgment, ObservationJudgment::FalseTakeover);
    assert_eq!(observations[1].judgment, ObservationJudgment::Correct);
    assert_eq!(
        payload["collectionReceipt"]["collectorId"],
        STORED_RECEIPT_COLLECTOR_ID
    );
    assert!(
        payload["collectionReceipt"]["digest"]
            .as_str()
            .is_some_and(|digest| digest.starts_with("sha256:"))
    );
    let stored = load_evaluation_session(host.store(), &session_id)
        .unwrap()
        .unwrap();
    assert!(stored.consumed);
    assert_eq!(
        host.collect_admitted_qualification(&session_id)
            .unwrap_err()
            .code,
        ContinuityFailureCode::IdempotencyConflict
    );
    assert_eq!(
        invoked
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
            .len(),
        2,
        "durable collection claim must prevent a second native invocation"
    );

    let reopened = ConversationService::from_store(ConversationStore::open(&root).unwrap());
    let restored = reopened.continuity().cloned().unwrap();
    assert_eq!(
        restored.adoption_policy().stage,
        AdoptionStage::AdmittedShadow
    );
    emit(
        "bound-runtime-collect",
        json!({
            "nativeInvoked": true,
            "hermeticObserverUnused": true,
            "sessionMatched": true,
            "candidateMatched": true,
            "materialsObserved": true,
            "typedObservations": true,
            "receiptBoundEvidence": true,
            "persistedAndReloaded": true,
            "stage": "admitted-shadow",
            "notQualifiedFromSmallSet": true,
            "expectedLabelsPrivate": true,
            "noDuplicateNative": true,
            "corpusBound": true,
            "datasetVersionMatched": true,
            "realModelQualification": "unknown"
        }),
    );
    let _ = std::fs::remove_file(result_path);
    let _ = std::fs::remove_dir_all(executable.parent().unwrap());
    let _ = std::fs::remove_dir_all(cwd);
}

#[test]
fn ac_05_001_native_failure_malformed_or_stale_reply_never_qualifies() {
    let transport_fail =
        ConversationService::from_store(ConversationStore::open_in_memory().unwrap());
    let transport_fail = bind_native_complete_turn(transport_fail, |_params: &Value| {
        Err(RuntimeAdapterError::ConversationDispatchFailed)
    });
    let host = transport_fail.continuity().cloned().unwrap();
    let (conversation_id, owner, agent) = create_group(&transport_fail);
    admit_fixture_corpus(&host, &conversation_id, &owner);
    let session_id = host
        .admit_live_evaluation_session(&conversation_id, &owner, &agent)
        .unwrap();
    assert_eq!(
        host.collect_admitted_qualification(&session_id)
            .unwrap_err()
            .code,
        ContinuityFailureCode::SourceUnavailable
    );
    assert_eq!(host.adoption_policy().stage, AdoptionStage::Offline);
    assert!(
        !load_evaluation_session(host.store(), &session_id)
            .unwrap()
            .unwrap()
            .consumed
    );

    let malformed = ConversationService::from_store(ConversationStore::open_in_memory().unwrap());
    let malformed = bind_native_complete_turn(malformed, |params: &Value| {
        Ok(json!({
            "ok": true,
            "output": "not-a-typed-proposal",
            "conversationId": params.get("conversationId"),
            "evaluationSessionId": params.get("evaluationSessionId"),
            "responsibilityId": params.get("responsibilityId"),
            "membershipId": params.get("membershipId"),
            "policyRevision": params.get("policyRevision"),
        }))
    });
    let host = malformed.continuity().cloned().unwrap();
    let (conversation_id, owner, agent) = create_group(&malformed);
    admit_fixture_corpus(&host, &conversation_id, &owner);
    let session_id = host
        .admit_live_evaluation_session(&conversation_id, &owner, &agent)
        .unwrap();
    assert_eq!(
        host.collect_admitted_qualification(&session_id)
            .unwrap_err()
            .code,
        ContinuityFailureCode::InvalidRequest
    );
    assert_eq!(host.adoption_policy().stage, AdoptionStage::Offline);
    assert!(
        !load_evaluation_session(host.store(), &session_id)
            .unwrap()
            .unwrap()
            .consumed
    );

    let stale = ConversationService::from_store(ConversationStore::open_in_memory().unwrap());
    let stale = bind_native_complete_turn(stale, |_params: &Value| {
        Ok(json!({
            "ok": true,
            "output": "",
            "conversationId": "conversation:other",
            "evaluationSessionId": "evaluation-session:other",
        }))
    });
    let host = stale.continuity().cloned().unwrap();
    let (conversation_id, owner, agent) = create_group(&stale);
    admit_fixture_corpus(&host, &conversation_id, &owner);
    let session_id = host
        .admit_live_evaluation_session(&conversation_id, &owner, &agent)
        .unwrap();
    assert_eq!(
        host.collect_admitted_qualification(&session_id)
            .unwrap_err()
            .code,
        ContinuityFailureCode::StaleRevision
    );
    assert_eq!(host.adoption_policy().stage, AdoptionStage::Offline);
    assert!(
        !load_evaluation_session(host.store(), &session_id)
            .unwrap()
            .unwrap()
            .consumed
    );
    emit(
        "native-failure-closed",
        json!({
            "transportFailureNotQualified": true,
            "malformedNotQualified": true,
            "staleOrMismatchNotQualified": true,
            "sessionNotConsumed": true,
            "realModelQualification": "unknown"
        }),
    );
}

#[test]
fn ac_05_001_missing_empty_or_mismatched_corpus_fails_before_native() {
    let invoked = Arc::new(Mutex::new(0_usize));
    let invoked_for_turn = invoked.clone();
    let service = bind_native_complete_turn(
        ConversationService::from_store(ConversationStore::open_in_memory().unwrap()),
        move |_params: &Value| {
            *invoked_for_turn
                .lock()
                .unwrap_or_else(|poison| poison.into_inner()) += 1;
            Ok(json!({ "ok": true, "output": "" }))
        },
    );
    let host = service.continuity().cloned().unwrap();
    let (conversation_id, owner, agent) = create_group(&service);
    let missing_session = host
        .admit_live_evaluation_session(&conversation_id, &owner, &agent)
        .unwrap();
    assert_eq!(
        host.collect_admitted_qualification(&missing_session)
            .unwrap_err()
            .code,
        ContinuityFailureCode::SourceUnavailable
    );
    assert_eq!(
        *invoked.lock().unwrap_or_else(|poison| poison.into_inner()),
        0
    );
    assert!(
        load_effect_status(host.store(), &collection_effect_id(&missing_session))
            .unwrap()
            .is_none()
    );

    assert_eq!(
        host.admit_live_evaluation_corpus(
            &conversation_id,
            &owner,
            FIXTURE_EVALUATION_DATASET,
            Vec::new(),
        )
        .unwrap_err()
        .code,
        ContinuityFailureCode::InvalidRequest
    );

    let first_digest = admit_fixture_corpus(&host, &conversation_id, &owner);
    let bound_session = host
        .admit_live_evaluation_session(&conversation_id, &owner, &agent)
        .unwrap();
    let mut replaced = fixture_evaluation_cases();
    replaced.push(StoredEvaluationCase {
        case_id: "case:extra".into(),
        family: "fam:admitted-session:extra".into(),
        subgroup: "en".into(),
        polarity: EvaluationCasePolarity::Negative,
        expected_action: EvaluationExpectedAction::Abstain,
        input: "another unlabeled fixture input".into(),
    });
    let replaced_digest = host
        .admit_live_evaluation_corpus(
            &conversation_id,
            &owner,
            FIXTURE_EVALUATION_DATASET,
            replaced,
        )
        .unwrap()
        .version_digest;
    assert_ne!(first_digest, replaced_digest);
    assert_eq!(
        host.collect_admitted_qualification(&bound_session)
            .unwrap_err()
            .code,
        ContinuityFailureCode::InvalidRequest
    );
    assert_eq!(
        *invoked.lock().unwrap_or_else(|poison| poison.into_inner()),
        0
    );
    emit(
        "corpus-binding",
        json!({
            "missingFailedBeforeNative": true,
            "emptyRejected": true,
            "mismatchFailedBeforeNative": true,
            "nativeInvoked": false,
            "realModelQualification": "unknown"
        }),
    );
}

#[test]
fn ac_05_001_partial_native_failure_retries_as_unknown_without_rerun() {
    let root = std::env::temp_dir().join(format!("lico-ca-unknown-claim-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&root).unwrap();
    let invoked = Arc::new(Mutex::new(0_usize));
    let invoked_for_turn = invoked.clone();
    let service = bind_native_complete_turn(
        ConversationService::from_store(ConversationStore::open(&root).unwrap()),
        move |_params: &Value| {
            let mut count = invoked_for_turn
                .lock()
                .unwrap_or_else(|poison| poison.into_inner());
            *count += 1;
            if *count == 1 {
                return Ok(json!({ "ok": true, "output": "" }));
            }
            Err(RuntimeAdapterError::ConversationDispatchFailed)
        },
    );
    let host = service.continuity().cloned().unwrap();
    let (conversation_id, owner, agent) = create_group(&service);
    admit_fixture_corpus(&host, &conversation_id, &owner);
    let session_id = host
        .admit_live_evaluation_session(&conversation_id, &owner, &agent)
        .unwrap();
    assert_eq!(
        host.collect_admitted_qualification(&session_id)
            .unwrap_err()
            .code,
        ContinuityFailureCode::SourceUnavailable
    );
    assert_eq!(
        *invoked.lock().unwrap_or_else(|poison| poison.into_inner()),
        2
    );
    assert_eq!(
        load_effect_status(host.store(), &collection_effect_id(&session_id)).unwrap(),
        Some(ContinuityEffectStatus::Unknown)
    );
    assert!(
        !load_evaluation_session(host.store(), &session_id)
            .unwrap()
            .unwrap()
            .consumed
    );
    assert_eq!(host.adoption_policy().stage, AdoptionStage::Offline);
    assert_eq!(
        host.collect_admitted_qualification(&session_id)
            .unwrap_err()
            .code,
        ContinuityFailureCode::ReconciliationRequired
    );
    assert_eq!(
        *invoked.lock().unwrap_or_else(|poison| poison.into_inner()),
        2
    );

    let reopened = ConversationService::from_store(ConversationStore::open(&root).unwrap());
    let restored = reopened.continuity().cloned().unwrap();
    assert_eq!(restored.adoption_policy().stage, AdoptionStage::Offline);
    assert_eq!(
        restored
            .collect_admitted_qualification(&session_id)
            .unwrap_err()
            .code,
        ContinuityFailureCode::ReconciliationRequired
    );
    assert_eq!(
        *invoked.lock().unwrap_or_else(|poison| poison.into_inner()),
        2
    );
    assert!(
        !load_evaluation_session(restored.store(), &session_id)
            .unwrap()
            .unwrap()
            .consumed
    );
    emit(
        "unknown-claim-retry",
        json!({
            "partialFailureLeftUnknown": true,
            "retryReconciles": true,
            "reopenRetryReconciles": true,
            "invocationCountUnchanged": true,
            "sessionUnconsumed": true,
            "stageUnchanged": true,
            "unknownIsNotProofOfNoExecution": true,
            "realModelQualification": "unknown"
        }),
    );
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn ac_05_001_preinvoke_failure_releases_and_retries_after_fix() {
    let invoked = Arc::new(Mutex::new(0_usize));
    let invoked_for_turn = invoked.clone();
    let missing_runtime =
        ConversationService::from_store(ConversationStore::open_in_memory().unwrap());
    let host = missing_runtime.continuity().cloned().unwrap();
    let (conversation_id, owner, agent) = create_group(&missing_runtime);
    admit_fixture_corpus(&host, &conversation_id, &owner);
    let session_id = host
        .admit_live_evaluation_session(&conversation_id, &owner, &agent)
        .unwrap();
    assert_eq!(
        host.collect_admitted_qualification(&session_id)
            .unwrap_err()
            .code,
        ContinuityFailureCode::SourceUnavailable
    );
    assert!(
        load_effect_status(host.store(), &collection_effect_id(&session_id))
            .unwrap()
            .is_none()
    );

    let bound = bind_native_complete_turn(missing_runtime, move |params: &Value| {
        *invoked_for_turn
            .lock()
            .unwrap_or_else(|poison| poison.into_inner()) += 1;
        Ok(json!({
            "ok": true,
            "output": "",
            "conversationId": params.get("conversationId"),
            "evaluationSessionId": params.get("evaluationSessionId"),
            "responsibilityId": params.get("responsibilityId"),
            "membershipId": params.get("membershipId"),
            "policyRevision": params.get("policyRevision"),
        }))
    });
    let host = bound.continuity().cloned().unwrap();
    host.collect_admitted_qualification(&session_id)
        .expect("same session retries after pre-invoke runtime bind");
    assert_eq!(
        *invoked.lock().unwrap_or_else(|poison| poison.into_inner()),
        2
    );
    assert!(
        load_evaluation_session(host.store(), &session_id)
            .unwrap()
            .unwrap()
            .consumed
    );

    let missing_corpus =
        ConversationService::from_store(ConversationStore::open_in_memory().unwrap());
    let missing_corpus = bind_native_complete_turn(missing_corpus, |_params: &Value| {
        Ok(json!({ "ok": true, "output": "" }))
    });
    let host = missing_corpus.continuity().cloned().unwrap();
    let (conversation_id, owner, agent) = create_group(&missing_corpus);
    let session_id = host
        .admit_live_evaluation_session(&conversation_id, &owner, &agent)
        .unwrap();
    assert_eq!(
        host.collect_admitted_qualification(&session_id)
            .unwrap_err()
            .code,
        ContinuityFailureCode::SourceUnavailable
    );
    admit_fixture_corpus(&host, &conversation_id, &owner);
    let retry_session = host
        .admit_live_evaluation_session(&conversation_id, &owner, &agent)
        .unwrap();
    host.collect_admitted_qualification(&retry_session)
        .expect("new session after corpus admit can collect");
    emit(
        "preinvoke-retry",
        json!({
            "missingRuntimeReleased": true,
            "missingRuntimeRetrySucceeded": true,
            "missingCorpusReleased": true,
            "missingCorpusRetrySucceeded": true,
            "realModelQualification": "unknown"
        }),
    );
}

#[test]
fn ac_05_001_stored_observation_field_mutation_rejects_reload() {
    let root =
        std::env::temp_dir().join(format!("lico-ca-receipt-mutate-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&root).unwrap();
    let service = ConversationService::from_store(ConversationStore::open(&root).unwrap());
    let host = service.continuity().cloned().unwrap();
    let (conversation_id, owner, agent) = create_group(&service);
    let session_id = host
        .admit_live_evaluation_session(&conversation_id, &owner, &agent)
        .unwrap();
    collect_with_producer(&host, &session_id, "fam:receipt-mutate");
    assert_eq!(
        host.adoption_policy().stage,
        AdoptionStage::QualifiedLowRisk
    );
    let rows = list_qualification_evidence(host.store()).unwrap();
    let (responsibility, identity_key, payload, class) = rows
        .into_iter()
        .find(|(_, _, _, class)| class == "live-authorized")
        .expect("live row");
    let mut stored: Value = serde_json::from_str(&payload).unwrap();
    stored["observations"][0]["economy"] = json!({
        "role": "candidate",
        "classificationCost": 9.0,
        "measuredFullCost": 9.0,
        "correctionCount": 0,
        "acceptedOutcome": false
    });
    put_qualification_evidence(
        host.store(),
        &responsibility,
        &identity_key,
        &stored.to_string(),
        &class,
    )
    .unwrap();
    let reopened = ConversationService::from_store(ConversationStore::open(&root).unwrap());
    let restored = reopened.continuity().cloned().unwrap();
    assert_ne!(
        restored.adoption_policy().stage,
        AdoptionStage::QualifiedLowRisk
    );
    emit(
        "receipt-mutation-reload",
        json!({
            "mutatedEconomyRejectedOnReload": true,
            "liveRowDropped": restored.adoption_policy().stage != AdoptionStage::QualifiedLowRisk,
            "realModelQualification": "unknown"
        }),
    );
    let _ = std::fs::remove_dir_all(root);
}
