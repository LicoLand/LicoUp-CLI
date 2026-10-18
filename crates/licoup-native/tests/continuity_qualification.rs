#![cfg(feature = "test-support")]
use licoup_conversation::continuity::{
    ContinuityCandidateIdentity, ContinuityDatasetSplit, ContinuityFailureCode,
    ContinuityQualificationRecord, ContinuityQualificationResult, QualificationPort,
};
use licoup_native::domain::agent_intelligence_catalog::qualification::{
    ClosureClaim, EconomyRole, EvidenceBundle, EvidenceClass, HardInvariantCounts,
    ObservationEconomy, ObservationJudgment, ObservationPolarity, QualificationObservation,
    QualificationPolicy, QualificationService, RequestKind, RoutingCandidate, SyntheticRecipe,
    UnqualifiedReason, WILSON_Z_ONE_SIDED_95, admit_child_scope, admit_identity,
    catalog_model_projection, evaluate_bundle, evaluate_economy, filter_eligible_then_stable_order,
    generate_synthetic, identity_changed, lookup_via_port, model_token_price, nearest_rank_p95,
    one_sided_wilson_upper, qualification_port, query_record, result_from_assessment,
    token_cost_from_owner,
};
use serde_json::Value;
use std::fs;
use std::path::{Path, PathBuf};

fn fixtures_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/fixtures/continuous-assistant/qualification")
}

fn read_fixture(name: &str) -> Value {
    let path = fixtures_root().join(name);
    serde_json::from_str(&fs::read_to_string(path).unwrap()).unwrap()
}

fn fixture_identity() -> ContinuityCandidateIdentity {
    let payload = read_fixture("candidate-identity.json");
    serde_json::from_value(payload["candidateIdentity"].clone()).unwrap()
}

fn fixture_policy() -> QualificationPolicy {
    QualificationPolicy::from_document_value(&read_fixture("evaluation-policy.json")).unwrap()
}

fn recipe_from(name: &str) -> (String, SyntheticRecipe) {
    let payload = read_fixture(name);
    let recipe = &payload["recipe"];
    (
        payload["responsibilityId"].as_str().unwrap().to_owned(),
        SyntheticRecipe {
            negative_families: recipe["negativeFamilies"].as_u64().unwrap(),
            positive_families: recipe["positiveFamilies"].as_u64().unwrap(),
            false_takeovers: recipe["falseTakeovers"].as_u64().unwrap(),
            missed_commitments: recipe["missedCommitments"].as_u64().unwrap(),
            abstentions: recipe["abstentions"].as_u64().unwrap(),
            split: ContinuityDatasetSplit::Heldout,
            family_prefix: recipe["familyPrefix"].as_str().unwrap().to_owned(),
        },
    )
}

fn ingest_recipe(
    service: &mut QualificationService,
    fixture_name: &str,
    identity: ContinuityCandidateIdentity,
) -> EvidenceBundle {
    let policy = service.policy().clone();
    let (responsibility, recipe) = recipe_from(fixture_name);
    let observations = generate_synthetic(&recipe, &policy.required_subgroups).unwrap();
    let bundle = EvidenceBundle::from_fixture_parts(responsibility, identity, observations);
    service.ingest_immutable(bundle.clone()).unwrap();
    bundle
}

fn passing_bundle(identity: ContinuityCandidateIdentity) -> EvidenceBundle {
    let policy = QualificationPolicy::draft_1();
    let (_, recipe) = recipe_from("passing-heldout-recipe.json");
    let observations = generate_synthetic(&recipe, &policy.required_subgroups).unwrap();
    EvidenceBundle::from_fixture_parts(
        "responsibility:delegation".to_owned(),
        identity,
        observations,
    )
}

#[test]
fn draft_policy_fixture_matches_blueprint_defaults() {
    let from_file = fixture_policy();
    let draft = QualificationPolicy::draft_1();
    assert_eq!(from_file, draft);
    assert_eq!(from_file.z, WILSON_Z_ONE_SIDED_95);
    assert_eq!(from_file.policy_revision, "draft-1");
}

#[test]
fn wilson_upper_matches_wikipedia_and_statsmodels_form() {
    // Wikipedia Wilson upper bound with one-sided z=Φ⁻¹(0.95). This is the
    // same closed form as statsmodels.stats.proportion.proportion_confint
    // method='wilson' at alpha=0.10 (90% two-sided == 95% one-sided).
    // Continuity-corrected wilsoncc is a different estimator and is not used.
    let z = WILSON_Z_ONE_SIDED_95;
    assert!(one_sided_wilson_upper(0, 0, z).is_none());
    let n1_k0 = one_sided_wilson_upper(0, 1, z).unwrap();
    let expected_n1_k0 = (z * z) / (1.0 + z * z);
    assert!((n1_k0 - expected_n1_k0).abs() < 1e-12);
    let n300_k0 = one_sided_wilson_upper(0, 300, z).unwrap();
    assert!(n300_k0 > 0.0 && n300_k0 < 0.02);
    let n300_k6 = one_sided_wilson_upper(6, 300, z).unwrap();
    assert!(n300_k6 > 0.02);
    let certain = one_sided_wilson_upper(1, 1, z).unwrap();
    assert!((certain - 1.0).abs() < 1e-12);
    assert!(one_sided_wilson_upper(2, 1, z).is_none());
}

#[test]
fn nearest_rank_p95_is_unknown_for_empty_and_uses_ceiling_rank() {
    assert!(nearest_rank_p95(&[]).is_none());
    assert_eq!(nearest_rank_p95(&[10, 20, 30, 40, 50]).unwrap(), 50.0);
}

#[test]
fn ac_02_007_all_abstain_is_unqualified_for_coverage() {
    let identity = fixture_identity();
    let mut service = QualificationService::draft_port();
    let bundle = ingest_recipe(&mut service, "all-abstain-heldout.json", identity.clone());
    let assessment = evaluate_bundle(&bundle, service.policy(), false);
    assert_eq!(
        assessment.result,
        ContinuityQualificationResult::Unqualified
    );
    assert!(
        assessment
            .reasons
            .contains(&UnqualifiedReason::CoverageBelowMinimum)
    );
    assert!(assessment.reasons.contains(&UnqualifiedReason::AllAbstain));
    assert_eq!(assessment.coverage.point, Some(0.0));
    assert_eq!(assessment.coverage.opportunities, 600);
    assert_eq!(assessment.false_takeover.errors, 0);
    assert_eq!(assessment.false_takeover.opportunities, 300);
    assert_eq!(assessment.missed_commitment.errors, 300);
    assert_eq!(assessment.missed_commitment.opportunities, 300);
    let looked = service
        .lookup_record(&query_record("responsibility:delegation", identity))
        .unwrap();
    assert_eq!(looked.result, ContinuityQualificationResult::Unqualified);
    assert!(service.admit_automatic_advancement(&looked).is_err());
}

#[test]
fn zero_samples_and_extreme_rates_stay_unknown_or_unqualified() {
    let identity = fixture_identity();
    let mut service = QualificationService::draft_port();
    let empty = ingest_recipe(&mut service, "zero-sample-query.json", identity.clone());
    let assessment = evaluate_bundle(&empty, service.policy(), false);
    assert_eq!(assessment.result, ContinuityQualificationResult::Unknown);
    assert!(
        assessment
            .reasons
            .contains(&UnqualifiedReason::ZeroHeldoutSamples)
    );
    assert!(assessment.false_takeover.wilson_upper.is_none());
    let port = qualification_port();
    let unknown = port
        .lookup(&query_record("responsibility:delegation", identity))
        .unwrap();
    assert_eq!(unknown.result, ContinuityQualificationResult::Unknown);
}

#[test]
fn ac_02_008_prompt_or_runtime_change_marks_old_qualification_stale() {
    let identity = fixture_identity();
    let delta = read_fixture("stale-identity-delta.json");
    let mut service = QualificationService::draft_port();
    ingest_recipe(
        &mut service,
        "passing-heldout-recipe.json",
        identity.clone(),
    );
    let issued = service
        .lookup_record(&query_record("responsibility:delegation", identity.clone()))
        .unwrap();
    assert_eq!(issued.result, ContinuityQualificationResult::Unknown);
    assert!(issued.observation_count >= 600);
    let assessment = service
        .assess("responsibility:delegation", &identity)
        .unwrap();
    assert!(assessment.policy_pass);

    let mut changed_prompt = issued.clone();
    changed_prompt.candidate_identity.prompt_digest =
        delta["promptChange"]["to"].as_str().unwrap().to_owned();
    assert!(identity_changed(
        &issued.candidate_identity,
        &changed_prompt.candidate_identity
    ));
    let stale = service.lookup_record(&changed_prompt).unwrap();
    assert_eq!(stale.result, ContinuityQualificationResult::Stale);
    assert_eq!(
        service
            .admit_automatic_advancement(&stale)
            .unwrap_err()
            .code,
        ContinuityFailureCode::QualificationStale
    );

    let mut changed_runtime = identity.clone();
    changed_runtime.adapter_runtime_digest = delta["adapterRuntimeChange"]["to"]
        .as_str()
        .unwrap()
        .to_owned();
    service
        .withdraw_for_identity_change("responsibility:delegation", &identity, &changed_runtime)
        .unwrap();
    let withdrawn = service
        .lookup_record(&query_record("responsibility:delegation", identity))
        .unwrap();
    assert_eq!(withdrawn.result, ContinuityQualificationResult::Stale);
}

#[test]
fn fixture_and_synthetic_evidence_never_become_live_qualified() {
    let identity = fixture_identity();
    let bundle = passing_bundle(identity.clone());
    assert_eq!(bundle.evidence_class, EvidenceClass::Synthetic);
    let assessment = evaluate_bundle(&bundle, &QualificationPolicy::draft_1(), false);
    assert!(assessment.policy_pass);
    assert_eq!(assessment.result, ContinuityQualificationResult::Unknown);
    assert_eq!(
        result_from_assessment(true, EvidenceClass::Synthetic, false, None),
        ContinuityQualificationResult::Unknown
    );
    assert_eq!(
        result_from_assessment(true, EvidenceClass::LiveAuthorized, false, None),
        ContinuityQualificationResult::Unknown
    );
    assert_eq!(
        result_from_assessment(true, EvidenceClass::LiveAuthorized, true, None),
        ContinuityQualificationResult::Unknown
    );
}

#[test]
fn self_confidence_is_ignored_for_admission() {
    let sample = read_fixture("self-confidence.json")["sample"].clone();
    let observation = QualificationObservation {
        observation_id: sample["observationId"].as_str().unwrap().to_owned(),
        conversation_family: sample["conversationFamily"].as_str().unwrap().to_owned(),
        split: ContinuityDatasetSplit::Heldout,
        subgroup: sample["subgroup"].as_str().unwrap().to_owned(),
        polarity: ObservationPolarity::Negative,
        judgment: ObservationJudgment::Abstain,
        self_confidence: Some(sample["selfConfidence"].as_f64().unwrap()),
        hard_invariants: HardInvariantCounts::default(),
        economy: None,
        closure_claim: None,
    };
    assert_eq!(observation.self_confidence, Some(0.99));
    let identity = fixture_identity();
    let mut bundle = passing_bundle(identity);
    for item in &mut bundle.observations {
        item.self_confidence = Some(0.99);
    }
    let assessment = evaluate_bundle(&bundle, &QualificationPolicy::draft_1(), false);
    assert!(assessment.policy_pass);
    assert_eq!(assessment.result, ContinuityQualificationResult::Unknown);
}

#[test]
fn family_leak_between_development_and_heldout_is_unqualified() {
    let leak = read_fixture("family-leak.json");
    let family = leak["sharedFamily"].as_str().unwrap();
    let identity = fixture_identity();
    let mut observations = generate_synthetic(
        &SyntheticRecipe {
            negative_families: 300,
            positive_families: 300,
            false_takeovers: 0,
            missed_commitments: 0,
            abstentions: 0,
            split: ContinuityDatasetSplit::Heldout,
            family_prefix: "family:heldout-pass".to_owned(),
        },
        &QualificationPolicy::draft_1().required_subgroups,
    )
    .unwrap();
    observations.push(QualificationObservation {
        observation_id: "obs:leak".to_owned(),
        conversation_family: family.to_owned(),
        split: ContinuityDatasetSplit::Development,
        subgroup: "en".to_owned(),
        polarity: ObservationPolarity::Negative,
        judgment: ObservationJudgment::Correct,
        self_confidence: None,
        hard_invariants: HardInvariantCounts::default(),
        economy: None,
        closure_claim: None,
    });
    observations[0].conversation_family = family.to_owned();
    let bundle = EvidenceBundle::from_fixture_parts(
        "responsibility:delegation".to_owned(),
        identity,
        observations,
    );
    let assessment = evaluate_bundle(&bundle, &QualificationPolicy::draft_1(), false);
    assert!(
        assessment
            .reasons
            .contains(&UnqualifiedReason::FamilySplitLeak)
    );
    assert_eq!(
        assessment.result,
        ContinuityQualificationResult::Unqualified
    );
}

#[test]
fn worker_or_turn_exit_is_not_goal_acceptance() {
    let identity = fixture_identity();
    let mut observations = generate_synthetic(
        &SyntheticRecipe {
            negative_families: 300,
            positive_families: 300,
            false_takeovers: 0,
            missed_commitments: 0,
            abstentions: 0,
            split: ContinuityDatasetSplit::Heldout,
            family_prefix: "family:heldout-exit".to_owned(),
        },
        &QualificationPolicy::draft_1().required_subgroups,
    )
    .unwrap();
    observations[0].closure_claim = Some(ClosureClaim::WorkerExit);
    let bundle = EvidenceBundle::from_fixture_parts(
        "responsibility:delegation".to_owned(),
        identity,
        observations,
    );
    let assessment = evaluate_bundle(&bundle, &QualificationPolicy::draft_1(), false);
    assert!(
        assessment
            .reasons
            .contains(&UnqualifiedReason::WorkerOrTurnExitClosure)
    );
    assert!(
        assessment
            .reasons
            .contains(&UnqualifiedReason::HardInvariant)
    );
    assert_eq!(
        assessment.result,
        ContinuityQualificationResult::Unqualified
    );
}

#[test]
fn economy_reports_full_cost_latency_corrections_and_native_diff() {
    let fixture = read_fixture("economic-comparison.json");
    let policy = QualificationPolicy::draft_1();
    let mut rows = Vec::new();
    for (index, pair) in fixture["pairs"].as_array().unwrap().iter().enumerate() {
        let task = format!("task:{index}");
        let candidate = &pair["candidate"];
        rows.push(ObservationEconomy {
            role: EconomyRole::Candidate,
            classification_cost: None,
            retrieval_cost: None,
            escalation_cost: None,
            execution_cost: None,
            retry_cost: None,
            rework_cost: None,
            measured_full_cost: candidate["measuredFullCost"].as_f64(),
            input_tokens: None,
            output_tokens: None,
            model_id: None,
            agent_id: None,
            thinking: None,
            serial_latency_ms: candidate["serialLatencyMs"].as_u64(),
            correction_count: candidate["correctionCount"].as_u64().unwrap(),
            accepted_outcome: candidate["acceptedOutcome"].as_bool().unwrap(),
            task_identity: Some(task.clone()),
            version_identity: Some("v1".into()),
            resource_identity: Some("res:1".into()),
        });
        let baseline = &pair["baseline"];
        rows.push(ObservationEconomy {
            role: EconomyRole::Baseline,
            classification_cost: None,
            retrieval_cost: None,
            escalation_cost: None,
            execution_cost: None,
            retry_cost: None,
            rework_cost: None,
            measured_full_cost: baseline["measuredFullCost"].as_f64(),
            input_tokens: None,
            output_tokens: None,
            model_id: None,
            agent_id: None,
            thinking: None,
            serial_latency_ms: baseline["serialLatencyMs"].as_u64(),
            correction_count: baseline["correctionCount"].as_u64().unwrap(),
            accepted_outcome: baseline["acceptedOutcome"].as_bool().unwrap(),
            task_identity: Some(task.clone()),
            version_identity: Some("v1".into()),
            resource_identity: Some("res:1".into()),
        });
        let native = &pair["nativeDirect"];
        rows.push(ObservationEconomy {
            role: EconomyRole::NativeDirect,
            classification_cost: None,
            retrieval_cost: None,
            escalation_cost: None,
            execution_cost: None,
            retry_cost: None,
            rework_cost: None,
            measured_full_cost: native["measuredFullCost"].as_f64(),
            input_tokens: None,
            output_tokens: None,
            model_id: None,
            agent_id: None,
            thinking: None,
            serial_latency_ms: native["serialLatencyMs"].as_u64(),
            correction_count: 0,
            accepted_outcome: native["acceptedOutcome"].as_bool().unwrap(),
            task_identity: Some(task),
            version_identity: Some("v1".into()),
            resource_identity: Some("res:1".into()),
        });
    }
    for excluded in fixture["excludedNatives"].as_array().unwrap() {
        rows.push(ObservationEconomy {
            role: EconomyRole::NativeDirect,
            classification_cost: None,
            retrieval_cost: None,
            escalation_cost: None,
            execution_cost: None,
            retry_cost: None,
            rework_cost: None,
            measured_full_cost: excluded["measuredFullCost"].as_f64(),
            input_tokens: None,
            output_tokens: None,
            model_id: None,
            agent_id: None,
            thinking: None,
            serial_latency_ms: excluded["serialLatencyMs"].as_u64(),
            correction_count: 0,
            accepted_outcome: excluded["acceptedOutcome"].as_bool().unwrap(),
            task_identity: excluded["taskIdentity"].as_str().map(str::to_owned),
            version_identity: excluded["versionIdentity"].as_str().map(str::to_owned),
            resource_identity: excluded["resourceIdentity"].as_str().map(str::to_owned),
        });
    }
    assert!(evaluate_economy(&[], &policy).unknown_cost);

    let mut identity = fixture_identity();
    identity.model_digest = "model:digest-econ".to_owned();
    let mut observations = generate_synthetic(
        &SyntheticRecipe {
            negative_families: 300,
            positive_families: 300,
            false_takeovers: 0,
            missed_commitments: 0,
            abstentions: 0,
            split: ContinuityDatasetSplit::Heldout,
            family_prefix: "family:econ-heldout".to_owned(),
        },
        &policy.required_subgroups,
    )
    .unwrap();
    for (index, row) in rows.iter().enumerate() {
        observations[index].economy = Some(row.clone());
    }
    let assessment = evaluate_bundle(
        &EvidenceBundle::from_fixture_parts(
            "responsibility:economy".to_owned(),
            identity,
            observations,
        ),
        &policy,
        false,
    );
    assert!(assessment.economy.candidate_full_cost.is_some());
    assert!(assessment.economy.baseline_full_cost.is_some());
    assert!(assessment.economy.cost_per_accepted_outcome_ratio.is_some());
    assert!(assessment.economy.extra_serial_latency_p95_ms.is_some());
    assert_eq!(assessment.economy.candidate_corrections, 1);
    assert_eq!(assessment.economy.baseline_corrections, 3);
    assert_eq!(
        assessment.economy.native_success_difference,
        Some(
            fixture["expect"]["nativeSuccessDifference"]
                .as_f64()
                .unwrap()
        )
    );
    assert_ne!(
        assessment.economy.native_success_difference,
        Some(
            fixture["expect"]["unpairedNativeAverageWouldBe"]
                .as_f64()
                .unwrap()
        ),
        "unmatched and version-mismatched native samples must not enter the paired difference"
    );
    assert!(
        assessment.result != ContinuityQualificationResult::Qualified
            || assessment.economy.economically_qualified
    );
}

#[test]
fn unknown_owner_price_is_not_treated_as_cheaper() {
    let unknown = read_fixture("economic-comparison.json")["unknownPriceCase"].clone();
    let model_id = unknown["modelId"].as_str().unwrap();
    assert!(model_token_price(model_id).is_none());
    assert!(
        token_cost_from_owner(
            Some(model_id),
            None,
            None,
            unknown["inputTokens"].as_u64().unwrap(),
            unknown["outputTokens"].as_u64().unwrap()
        )
        .is_none()
    );
    assert!(catalog_model_projection(model_id).is_none());
}

#[test]
fn explicit_model_choice_and_requested_execution_bypass_automatic_gate() {
    let identity = fixture_identity();
    let service = QualificationService::draft_port();
    let unknown = query_record("responsibility:delegation", identity.clone());
    assert_eq!(
        service
            .admit_execution(&unknown, RequestKind::ExplicitlyRequested)
            .unwrap(),
        ()
    );
    assert_eq!(
        service
            .admit_execution(&unknown, RequestKind::AutomaticAdvancement)
            .unwrap_err()
            .code,
        ContinuityFailureCode::QualificationUnknown
    );
    let candidates = filter_eligible_then_stable_order(
        vec![
            RoutingCandidate {
                responsibility_id: "responsibility:delegation".to_owned(),
                identity: identity.clone(),
                stable_key: "b-stable".to_owned(),
                hard_admitted: true,
                explicit_model_choice: true,
                request_kind: RequestKind::AutomaticAdvancement,
            },
            RoutingCandidate {
                responsibility_id: "responsibility:delegation".to_owned(),
                identity: identity.clone(),
                stable_key: "a-stable".to_owned(),
                hard_admitted: true,
                explicit_model_choice: false,
                request_kind: RequestKind::AutomaticAdvancement,
            },
            RoutingCandidate {
                responsibility_id: "responsibility:delegation".to_owned(),
                identity,
                stable_key: "c-stable".to_owned(),
                hard_admitted: false,
                explicit_model_choice: true,
                request_kind: RequestKind::ExplicitlyRequested,
            },
        ],
        |candidate| query_record(&candidate.responsibility_id, candidate.identity.clone()),
    );
    assert_eq!(candidates.len(), 1);
    assert_eq!(candidates[0].stable_key, "b-stable");
}

#[test]
fn child_scope_cannot_widen_permission_or_promote_synthetic_to_live() {
    let parent = fixture_identity();
    let mut child = parent.clone();
    child.prompt_digest = "prompt:child-task".to_owned();
    admit_child_scope(
        &parent,
        &child,
        EvidenceClass::Synthetic,
        EvidenceClass::Synthetic,
    )
    .unwrap();
    child.model_digest = "model:widened".to_owned();
    assert_eq!(
        admit_child_scope(
            &parent,
            &child,
            EvidenceClass::Synthetic,
            EvidenceClass::Synthetic
        )
        .unwrap_err()
        .code,
        ContinuityFailureCode::ScopeDenied
    );
    assert_eq!(
        admit_child_scope(
            &parent,
            &parent,
            EvidenceClass::Synthetic,
            EvidenceClass::LiveAuthorized
        )
        .unwrap_err()
        .code,
        ContinuityFailureCode::ScopeDenied
    );
}

#[test]
fn immutable_ingest_rejects_mutation_and_empty_identity() {
    let identity = fixture_identity();
    let mut service = QualificationService::draft_port();
    ingest_recipe(
        &mut service,
        "passing-heldout-recipe.json",
        identity.clone(),
    );
    let again = passing_bundle(identity.clone());
    assert_eq!(
        service.ingest_immutable(again).unwrap_err().code,
        ContinuityFailureCode::IdempotencyConflict
    );
    let mut empty = identity;
    empty.prompt_digest.clear();
    assert!(admit_identity(&empty).is_err());
}

#[test]
fn port_entry_returns_unknown_without_evidence_and_does_not_copy_catalogs() {
    let port = qualification_port();
    let record = query_record("responsibility:delegation", fixture_identity());
    let looked = lookup_via_port(&port, &record).unwrap();
    assert_eq!(looked.result, ContinuityQualificationResult::Unknown);
    let source = include_str!("../src/domain/agent_intelligence_catalog/qualification/mod.rs");
    assert!(!source.contains("aa_intelligence_index.json"));
    assert!(!source.contains("pricing_catalog.json"));
    let owners = include_str!("../src/domain/agent_intelligence_catalog/qualification/owners.rs");
    assert!(owners.contains("provider_model_pricing::model_price"));
    assert!(owners.contains("skill_hub::skill_list"));
    assert!(owners.contains("targets::inspect_target_read_only"));
}

#[test]
fn development_observations_do_not_qualify_without_heldout() {
    let identity = fixture_identity();
    let observations = generate_synthetic(
        &SyntheticRecipe {
            negative_families: 300,
            positive_families: 300,
            false_takeovers: 0,
            missed_commitments: 0,
            abstentions: 0,
            split: ContinuityDatasetSplit::Development,
            family_prefix: "family:dev-only".to_owned(),
        },
        &QualificationPolicy::draft_1().required_subgroups,
    )
    .unwrap();
    let assessment = evaluate_bundle(
        &EvidenceBundle::from_fixture_parts(
            "responsibility:delegation".to_owned(),
            identity,
            observations,
        ),
        &QualificationPolicy::draft_1(),
        false,
    );
    assert_eq!(assessment.result, ContinuityQualificationResult::Unknown);
    assert!(
        assessment
            .reasons
            .contains(&UnqualifiedReason::ZeroHeldoutSamples)
    );
}

#[test]
fn frozen_qualification_record_round_trip_from_contract_fixture() {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/fixtures/continuous-assistant/contracts/legal/qualification.json");
    let document: Value = serde_json::from_str(&fs::read_to_string(path).unwrap()).unwrap();
    let parsed: ContinuityQualificationRecord =
        serde_json::from_value(document["payload"].clone()).unwrap();
    assert_eq!(parsed.result, ContinuityQualificationResult::Unknown);
    assert_eq!(parsed.observation_count, 0);
}

#[test]
fn economy_pairs_equivalent_task_version_resource_not_input_order() {
    let mismatched = evaluate_economy(
        &[
            ObservationEconomy {
                role: EconomyRole::Candidate,
                measured_full_cost: Some(1.0),
                accepted_outcome: true,
                serial_latency_ms: Some(10),
                task_identity: Some("task:a".into()),
                version_identity: Some("v1".into()),
                resource_identity: Some("res:1".into()),
                ..economy_blank()
            },
            ObservationEconomy {
                role: EconomyRole::Baseline,
                measured_full_cost: Some(1.0),
                accepted_outcome: true,
                serial_latency_ms: Some(8),
                task_identity: Some("task:b".into()),
                version_identity: Some("v1".into()),
                resource_identity: Some("res:1".into()),
                ..economy_blank()
            },
        ],
        &QualificationPolicy::draft_1(),
    );
    assert!(mismatched.paired_success_difference.is_none());

    let matched = evaluate_economy(
        &[
            ObservationEconomy {
                role: EconomyRole::Candidate,
                measured_full_cost: Some(1.0),
                accepted_outcome: true,
                serial_latency_ms: Some(10),
                task_identity: Some("task:a".into()),
                version_identity: Some("v1".into()),
                resource_identity: Some("res:1".into()),
                ..economy_blank()
            },
            ObservationEconomy {
                role: EconomyRole::Baseline,
                measured_full_cost: Some(1.0),
                accepted_outcome: true,
                serial_latency_ms: Some(8),
                task_identity: Some("task:a".into()),
                version_identity: Some("v1".into()),
                resource_identity: Some("res:1".into()),
                ..economy_blank()
            },
        ],
        &QualificationPolicy::draft_1(),
    );
    assert_eq!(matched.paired_success_difference, Some(0.0));
}

fn economy_blank() -> ObservationEconomy {
    ObservationEconomy {
        role: EconomyRole::Candidate,
        classification_cost: None,
        retrieval_cost: None,
        escalation_cost: None,
        execution_cost: None,
        retry_cost: None,
        rework_cost: None,
        measured_full_cost: None,
        input_tokens: None,
        output_tokens: None,
        model_id: None,
        agent_id: None,
        thinking: None,
        serial_latency_ms: None,
        correction_count: 0,
        accepted_outcome: false,
        task_identity: None,
        version_identity: None,
        resource_identity: None,
    }
}
