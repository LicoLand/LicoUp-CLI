#![cfg(feature = "test-support")]
//! TASK-04-002 evaluation target. Calls production qualification APIs only.

use licoup_conversation::continuity::{
    ContinuityCandidateIdentity, ContinuityDatasetSplit, ContinuityFailureCode,
    ContinuityQualificationResult,
};
use licoup_native::domain::agent_intelligence_catalog::qualification::{
    EconomyRole, EvidenceBundle, EvidenceClass, HardInvariantCounts, LiveAdmission, LiveProvenance,
    ObservationEconomy, ObservationJudgment, ObservationPolarity, QualificationObservation,
    QualificationPolicy, QualificationService, RequestKind, SYNTHETIC_TEST_EVIDENCE_LABEL,
    SyntheticRecipe, UnqualifiedReason, WILSON_Z_ONE_SIDED_95, evaluate_bundle, evaluate_economy,
    generate_synthetic, identity_changed, one_sided_wilson_upper, qualification_port, query_record,
    result_from_assessment, token_cost_from_owner,
};
use serde_json::{Value, json};
use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

fn fixtures_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/fixtures/continuous-assistant/evaluation")
}

fn read_fixture(name: &str) -> Value {
    let path = fixtures_root().join(name);
    serde_json::from_str(
        &fs::read_to_string(&path).unwrap_or_else(|error| {
            panic!("missing evaluation fixture {}: {error}", path.display())
        }),
    )
    .unwrap()
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

fn emit_oracle(label: &str, payload: Value) {
    println!("CONTINUITY_EVALUATION_ORACLE:{label}:{payload}");
}

fn polarity(value: &str) -> ObservationPolarity {
    match value {
        "positive" => ObservationPolarity::Positive,
        _ => ObservationPolarity::Negative,
    }
}

fn judgment(value: &str) -> ObservationJudgment {
    match value {
        "falseTakeover" => ObservationJudgment::FalseTakeover,
        "missedCommitment" => ObservationJudgment::MissedCommitment,
        "abstain" => ObservationJudgment::Abstain,
        "escalation" => ObservationJudgment::Escalation,
        _ => ObservationJudgment::Correct,
    }
}

fn split_from(value: &str) -> ContinuityDatasetSplit {
    match value {
        "development" => ContinuityDatasetSplit::Development,
        "calibration" => ContinuityDatasetSplit::Calibration,
        _ => ContinuityDatasetSplit::Heldout,
    }
}

fn families_to_set(value: &Value) -> BTreeSet<String> {
    value
        .as_array()
        .unwrap()
        .iter()
        .map(|item| item.as_str().unwrap().to_owned())
        .collect()
}

fn observation(
    id: &str,
    family: &str,
    split: ContinuityDatasetSplit,
    subgroup: &str,
    polarity: ObservationPolarity,
    judgment: ObservationJudgment,
) -> QualificationObservation {
    QualificationObservation {
        observation_id: id.to_owned(),
        conversation_family: family.to_owned(),
        split,
        subgroup: subgroup.to_owned(),
        polarity,
        judgment,
        self_confidence: None,
        hard_invariants: HardInvariantCounts::default(),
        economy: None,
        closure_claim: None,
    }
}

fn passing_heldout_bundle(identity: ContinuityCandidateIdentity) -> EvidenceBundle {
    let policy = fixture_policy();
    let observations = generate_synthetic(
        &SyntheticRecipe {
            negative_families: 300,
            positive_families: 300,
            false_takeovers: 0,
            missed_commitments: 0,
            abstentions: 0,
            split: ContinuityDatasetSplit::Heldout,
            family_prefix: "fam:eval-pass".to_owned(),
        },
        &policy.required_subgroups,
    )
    .unwrap();
    EvidenceBundle::from_fixture_parts(
        "responsibility:evaluation".to_owned(),
        identity,
        observations,
    )
}

fn apply_identity_field(identity: &mut ContinuityCandidateIdentity, field: &str, value: &str) {
    match field {
        "modelDigest" => identity.model_digest = value.to_owned(),
        "adapterRuntimeDigest" => identity.adapter_runtime_digest = value.to_owned(),
        "promptDigest" => identity.prompt_digest = value.to_owned(),
        "toolContractDigest" => identity.tool_contract_digest = value.to_owned(),
        "policyRevision" => identity.policy_revision = value.to_owned(),
        "datasetVersion" => identity.dataset_version = value.to_owned(),
        other => panic!("unsupported identity field {other}"),
    }
}

fn family_observations(split: &Value) -> Vec<QualificationObservation> {
    let mut observations = Vec::new();
    for family in families_to_set(&split["developmentFamilies"]) {
        observations.push(observation(
            &family,
            &family,
            ContinuityDatasetSplit::Development,
            "en",
            ObservationPolarity::Negative,
            ObservationJudgment::Correct,
        ));
    }
    for family in families_to_set(&split["calibrationFamilies"]) {
        observations.push(observation(
            &family,
            &family,
            ContinuityDatasetSplit::Calibration,
            "en",
            ObservationPolarity::Negative,
            ObservationJudgment::Correct,
        ));
    }
    for family in families_to_set(&split["heldoutFamilies"]) {
        observations.push(observation(
            &family,
            &family,
            ContinuityDatasetSplit::Heldout,
            "en",
            ObservationPolarity::Negative,
            ObservationJudgment::Correct,
        ));
    }
    observations
}

fn leak_family_into(observations: &mut Vec<QualificationObservation>, leak_family: &str) {
    observations.push(observation(
        "obs:leak",
        leak_family,
        ContinuityDatasetSplit::Development,
        "en",
        ObservationPolarity::Negative,
        ObservationJudgment::Correct,
    ));
}

#[test]
fn ac_04_002_heldout_family_split_is_disjoint_and_version_isolated() {
    let split = read_fixture("family-heldout-split.json");
    let development = families_to_set(&split["developmentFamilies"]);
    let calibration = families_to_set(&split["calibrationFamilies"]);
    let heldout = families_to_set(&split["heldoutFamilies"]);
    assert!(development.is_disjoint(&heldout));
    assert!(development.is_disjoint(&calibration));
    assert!(heldout.is_disjoint(&calibration));

    let clean_observations = family_observations(&split);
    let identity = fixture_identity();
    let policy = fixture_policy();
    let clean = evaluate_bundle(
        &EvidenceBundle::from_fixture_parts(
            "responsibility:evaluation".to_owned(),
            identity.clone(),
            clean_observations.clone(),
        ),
        &policy,
        false,
    );
    assert_eq!(clean.result, ContinuityQualificationResult::Unqualified);
    assert!(!clean.reasons.contains(&UnqualifiedReason::FamilySplitLeak));

    let leak_family = split["leakFamily"].as_str().unwrap();
    let mut leaked_observations = clean_observations.clone();
    leak_family_into(&mut leaked_observations, leak_family);
    let leaked = evaluate_bundle(
        &EvidenceBundle::from_fixture_parts(
            "responsibility:evaluation".to_owned(),
            identity.clone(),
            leaked_observations.clone(),
        ),
        &policy,
        false,
    );
    assert!(
        leaked.reasons.contains(&UnqualifiedReason::FamilySplitLeak),
        "FamilySplitLeak must appear after the leaked family, not only generic Unqualified"
    );

    let mut isolated_identity = identity.clone();
    apply_identity_field(
        &mut isolated_identity,
        split["isolatedVersion"]["field"].as_str().unwrap(),
        split["isolatedVersion"]["to"].as_str().unwrap(),
    );
    assert!(identity_changed(&identity, &isolated_identity));

    let mut service = QualificationService::with_policy(policy);
    service
        .ingest_immutable(EvidenceBundle::from_fixture_parts(
            "responsibility:evaluation".to_owned(),
            identity.clone(),
            leaked_observations,
        ))
        .unwrap();
    service
        .ingest_immutable(EvidenceBundle::from_fixture_parts(
            "responsibility:evaluation".to_owned(),
            isolated_identity.clone(),
            clean_observations,
        ))
        .unwrap();
    let leaked_version = service
        .assess("responsibility:evaluation", &identity)
        .unwrap();
    let isolated_version = service
        .assess("responsibility:evaluation", &isolated_identity)
        .unwrap();
    assert!(
        leaked_version
            .reasons
            .contains(&UnqualifiedReason::FamilySplitLeak)
    );
    assert!(
        !isolated_version
            .reasons
            .contains(&UnqualifiedReason::FamilySplitLeak)
    );
    emit_oracle(
        "family-split",
        json!({
            "heldoutFamilySplitDisjoint": true,
            "familySplitLeakAbsentBefore": !clean.reasons.contains(&UnqualifiedReason::FamilySplitLeak),
            "familySplitLeakPresentAfter": leaked.reasons.contains(&UnqualifiedReason::FamilySplitLeak),
            "versionIsolated": !isolated_version.reasons.contains(&UnqualifiedReason::FamilySplitLeak)
                && leaked_version.reasons.contains(&UnqualifiedReason::FamilySplitLeak),
            "isolatedVersionField": split["isolatedVersion"]["field"],
            "externalCalls": 0
        }),
    );
}

#[test]
fn ac_04_002_fpr_and_miss_use_separate_denominators() {
    let payload = read_fixture("heldout-denominators.json");
    let identity = fixture_identity();
    let mut observations = Vec::new();
    for family in payload["families"].as_array().unwrap() {
        observations.push(observation(
            family["id"].as_str().unwrap(),
            family["id"].as_str().unwrap(),
            split_from(family["split"].as_str().unwrap()),
            family["subgroup"].as_str().unwrap(),
            polarity(family["polarity"].as_str().unwrap()),
            judgment(family["judgment"].as_str().unwrap()),
        ));
    }
    let assessment = evaluate_bundle(
        &EvidenceBundle::from_fixture_parts(
            payload["responsibilityId"].as_str().unwrap().to_owned(),
            identity,
            observations,
        ),
        &fixture_policy(),
        false,
    );
    let expect = &payload["expect"];
    assert_eq!(
        assessment.false_takeover.errors,
        expect["falseTakeoverErrors"].as_u64().unwrap()
    );
    assert_eq!(
        assessment.false_takeover.opportunities,
        expect["falseTakeoverOpportunities"].as_u64().unwrap()
    );
    assert_eq!(
        assessment.missed_commitment.errors,
        expect["missedCommitmentErrors"].as_u64().unwrap()
    );
    assert_eq!(
        assessment.missed_commitment.opportunities,
        expect["missedCommitmentOpportunities"].as_u64().unwrap()
    );
    assert_eq!(
        assessment.coverage.errors,
        expect["coverageAbstentions"].as_u64().unwrap()
    );
    assert_eq!(
        assessment.coverage.opportunities,
        expect["coverageOpportunities"].as_u64().unwrap()
    );
    assert_eq!(assessment.false_takeover.opportunities, 4);
    assert_eq!(assessment.missed_commitment.opportunities, 4);
    emit_oracle(
        "denominators",
        json!({
            "falseTakeoverErrors": assessment.false_takeover.errors,
            "falseTakeoverOpportunities": assessment.false_takeover.opportunities,
            "missedCommitmentErrors": assessment.missed_commitment.errors,
            "missedCommitmentOpportunities": assessment.missed_commitment.opportunities,
            "coverageAbstentions": assessment.coverage.errors,
            "coverageOpportunities": assessment.coverage.opportunities,
            "externalCalls": 0
        }),
    );
}

#[test]
fn ac_04_002_all_abstain_rejects_zero_fpr_coverage() {
    let identity = fixture_identity();
    let mut service = QualificationService::with_policy(fixture_policy());
    let (responsibility, recipe) = recipe_from("all-abstain-heldout.json");
    let observations = generate_synthetic(&recipe, &service.policy().required_subgroups).unwrap();
    let bundle = EvidenceBundle::from_fixture_parts(responsibility, identity.clone(), observations);
    service.ingest_immutable(bundle.clone()).unwrap();
    let assessment = evaluate_bundle(&bundle, service.policy(), false);
    assert_eq!(
        assessment.result,
        ContinuityQualificationResult::Unqualified
    );
    assert!(assessment.reasons.contains(&UnqualifiedReason::AllAbstain));
    assert!(
        assessment
            .reasons
            .contains(&UnqualifiedReason::CoverageBelowMinimum)
    );
    assert_eq!(assessment.false_takeover.errors, 0);
    assert_eq!(assessment.false_takeover.opportunities, 300);
    assert_eq!(assessment.missed_commitment.errors, 300);
    assert_eq!(assessment.missed_commitment.opportunities, 300);
    assert_eq!(assessment.coverage.point, Some(0.0));
    let looked = service
        .lookup_record(&query_record("responsibility:evaluation", identity))
        .unwrap();
    assert_eq!(looked.result, ContinuityQualificationResult::Unqualified);
    assert!(service.admit_automatic_advancement(&looked).is_err());
    emit_oracle(
        "all-abstain",
        json!({
            "allAbstainRejected": true,
            "zeroFalseTakeoverDoesNotPassCoverage": true,
            "result": "unqualified",
            "externalCalls": 0
        }),
    );
}

#[test]
fn ac_04_002_zero_heldout_samples_are_unknown() {
    let identity = fixture_identity();
    let (responsibility, recipe) = recipe_from("zero-sample.json");
    let observations = generate_synthetic(&recipe, &fixture_policy().required_subgroups).unwrap();
    let assessment = evaluate_bundle(
        &EvidenceBundle::from_fixture_parts(responsibility, identity.clone(), observations),
        &fixture_policy(),
        false,
    );
    assert_eq!(assessment.result, ContinuityQualificationResult::Unknown);
    assert!(assessment.false_takeover.wilson_upper.is_none());
    assert!(assessment.missed_commitment.wilson_upper.is_none());
    let unknown = qualification_port()
        .lookup_record(&query_record("responsibility:evaluation", identity))
        .unwrap();
    assert_eq!(unknown.result, ContinuityQualificationResult::Unknown);
    emit_oracle(
        "zero-sample",
        json!({
            "zeroSamplesUnknown": true,
            "wilsonUnknown": true,
            "externalCalls": 0
        }),
    );
}

#[test]
fn ac_04_002_wilson_bounds_come_from_rust() {
    let z = WILSON_Z_ONE_SIDED_95;
    let policy = fixture_policy();
    assert_eq!(policy.z, z);
    assert!(one_sided_wilson_upper(0, 0, z).is_none());
    let n300_k0 = one_sided_wilson_upper(0, 300, z).unwrap();
    let n300_k6 = one_sided_wilson_upper(6, 300, z).unwrap();
    assert!(n300_k0 < policy.false_takeover_upper_max);
    assert!(n300_k6 > policy.false_takeover_upper_max);
    emit_oracle(
        "wilson",
        json!({
            "wilsonN0Unknown": true,
            "wilsonN300K0BelowFprMax": true,
            "wilsonN300K6AboveFprMax": true,
            "externalCalls": 0
        }),
    );
}

#[test]
fn ac_04_002_model_runtime_prompt_resource_policy_drift_is_stale() {
    let delta = read_fixture("identity-drift.json");
    let mut dimensions = serde_json::Map::new();
    for key in ["model", "runtime", "prompt", "resource", "policy"] {
        let change = &delta["changes"][key];
        let field = change["field"].as_str().unwrap();
        let to = change["to"].as_str().unwrap();
        let baseline = fixture_identity();
        let mut service = QualificationService::with_policy(fixture_policy());
        service
            .ingest_immutable(passing_heldout_bundle(baseline.clone()))
            .unwrap();
        let issued = service
            .lookup_record(&query_record("responsibility:evaluation", baseline.clone()))
            .unwrap();
        assert_eq!(issued.result, ContinuityQualificationResult::Unknown);
        let baseline_assessment = service
            .assess("responsibility:evaluation", &baseline)
            .unwrap();
        assert!(baseline_assessment.policy_pass);
        assert!(!baseline_assessment.stale);

        let mut drifted = baseline.clone();
        apply_identity_field(&mut drifted, field, to);
        assert!(identity_changed(&baseline, &drifted));
        service
            .withdraw_for_identity_change("responsibility:evaluation", &baseline, &drifted)
            .unwrap();
        let withdrawn = service
            .lookup_record(&query_record("responsibility:evaluation", baseline.clone()))
            .unwrap();
        assert_eq!(withdrawn.result, ContinuityQualificationResult::Stale);
        assert_eq!(
            service
                .admit_execution(&withdrawn, RequestKind::AutomaticAdvancement)
                .unwrap_err()
                .code,
            ContinuityFailureCode::QualificationStale
        );
        let drifted_record = service
            .lookup_record(&query_record("responsibility:evaluation", drifted))
            .unwrap();
        assert_eq!(
            drifted_record.result,
            ContinuityQualificationResult::Unknown
        );
        assert_eq!(drifted_record.observation_count, 0);
        dimensions.insert(
            key.to_owned(),
            json!({
                "field": field,
                "baselinePolicyPass": baseline_assessment.policy_pass,
                "baselineBecameStale": withdrawn.result == ContinuityQualificationResult::Stale,
                "automaticAdvancementDenied": true,
                "driftedDidNotInheritBaselineEvidence": drifted_record.observation_count == 0
            }),
        );
    }
    emit_oracle(
        "identity-drift",
        json!({
            "identityDriftStale": dimensions.values().all(|item| {
                item["baselineBecameStale"].as_bool() == Some(true)
            }),
            "driftedDimensions": dimensions.len() as u64,
            "automaticAdvancementDenied": dimensions.values().all(|item| {
                item["automaticAdvancementDenied"].as_bool() == Some(true)
            }),
            "dimensions": dimensions,
            "externalCalls": 0
        }),
    );
}

#[test]
fn ac_04_002_unknown_cost_is_not_zero() {
    let payload = read_fixture("unknown-cost.json");
    let model_id = payload["modelId"].as_str().unwrap();
    assert!(
        token_cost_from_owner(
            Some(model_id),
            None,
            None,
            payload["inputTokens"].as_u64().unwrap(),
            payload["outputTokens"].as_u64().unwrap()
        )
        .is_none()
    );
    let report = evaluate_economy(
        &[ObservationEconomy {
            role: EconomyRole::Candidate,
            input_tokens: payload["inputTokens"].as_u64(),
            output_tokens: payload["outputTokens"].as_u64(),
            model_id: Some(model_id.to_owned()),
            accepted_outcome: true,
            task_identity: payload["taskIdentity"].as_str().map(str::to_owned),
            version_identity: payload["versionIdentity"].as_str().map(str::to_owned),
            resource_identity: payload["resourceIdentity"].as_str().map(str::to_owned),
            classification_cost: None,
            retrieval_cost: None,
            escalation_cost: None,
            execution_cost: None,
            retry_cost: None,
            rework_cost: None,
            measured_full_cost: None,
            agent_id: None,
            thinking: None,
            serial_latency_ms: None,
            correction_count: 0,
        }],
        &fixture_policy(),
    );
    assert!(report.unknown_cost);
    assert_ne!(report.candidate_full_cost, Some(0.0));
    assert!(report.candidate_full_cost.is_none());
    assert!(!report.economically_qualified);
    emit_oracle(
        "unknown-cost",
        json!({
            "unknownCostNotZero": true,
            "unknownCostNotEconomicallyQualified": true,
            "externalCalls": 0
        }),
    );
}

#[test]
fn ac_04_002_synthetic_evidence_is_never_live_qualified() {
    let bundle = passing_heldout_bundle(fixture_identity());
    assert_eq!(bundle.evidence_class, EvidenceClass::Synthetic);
    let assessment = evaluate_bundle(&bundle, &fixture_policy(), false);
    assert!(assessment.policy_pass);
    assert_eq!(assessment.result, ContinuityQualificationResult::Unknown);
    assert_eq!(
        result_from_assessment(true, EvidenceClass::Synthetic, false, None),
        ContinuityQualificationResult::Unknown
    );
    emit_oracle(
        "synthetic-class",
        json!({
            "syntheticNotLiveQualified": true,
            "realModelQualification": "unknown",
            "externalCalls": 0
        }),
    );
}

#[test]
fn ac_04_005_full_chain_and_direct_native_exclude_unmatched() {
    let payload = read_fixture("native-comparable.json");
    let rows: Vec<ObservationEconomy> = serde_json::from_value(payload["rows"].clone()).unwrap();
    let expect = &payload["expect"];
    let report = evaluate_economy(&rows, &fixture_policy());
    assert!(
        rows.iter()
            .any(|row| row.role == EconomyRole::NativeDirect && !row.accepted_outcome),
        "matched native observation must be a genuine NativeDirect accepted_outcome"
    );
    assert_eq!(
        report.native_success_difference,
        Some(expect["nativeSuccessDifference"].as_f64().unwrap())
    );
    assert_eq!(
        report.paired_success_difference,
        Some(expect["pairedSuccessDifference"].as_f64().unwrap())
    );
    assert_eq!(
        report.candidate_full_cost,
        Some(expect["candidateFullCost"].as_f64().unwrap())
    );
    assert_eq!(
        report.baseline_full_cost,
        Some(expect["baselineFullCost"].as_f64().unwrap())
    );
    assert_ne!(report.candidate_full_cost, Some(118.0));
    assert_ne!(
        report.native_success_difference,
        Some(0.5),
        "unpaired native samples must not drive the native difference"
    );
    emit_oracle(
        "native-comparable",
        json!({
            "comparablePairsUsed": true,
            "unmatchedExcluded": true,
            "versionMismatchExcluded": true,
            "nativeDirectPairedDifference": report.native_success_difference,
            "unpairedNativeAverageUnused": report.native_success_difference != Some(0.5),
            "totalsExcludeNativeDirect": report.candidate_full_cost == Some(11.0)
                && report.baseline_full_cost == Some(3.0),
            "externalCalls": 0
        }),
    );
}

#[test]
fn ac_04_005_unpaired_cost_must_not_enter_paired_ratio() {
    let payload = read_fixture("pairing-unmatched-cost.json");
    let rows: Vec<ObservationEconomy> = serde_json::from_value(payload["rows"].clone()).unwrap();
    let report = evaluate_economy(&rows, &fixture_policy());
    let observed_ratio = report.cost_per_accepted_outcome_ratio;
    emit_oracle(
        "pairing-defect",
        json!({
            "pairedSuccessDifference": report.paired_success_difference,
            "observedRatioPresent": observed_ratio.is_some(),
            "totalsRetainSpend": report.candidate_full_cost == Some(10.0)
                && report.baseline_full_cost == Some(1010.0),
            "correctPairedRatio": 1.0,
            "correctEconomicallyQualified": false,
            "externalCalls": 0
        }),
    );
    assert_eq!(report.paired_success_difference, Some(0.0));
    assert_eq!(report.candidate_full_cost, Some(10.0));
    assert_eq!(report.baseline_full_cost, Some(1010.0));
    assert_eq!(
        report.cost_per_accepted_outcome_ratio,
        Some(1.0),
        "unpaired baseline spend must not enter the paired cost ratio"
    );
    assert!(
        !report.economically_qualified,
        "unpaired baseline spend must not mark a 1.0 paired ratio economical"
    );
}

#[test]
fn ac_04_003_live_admission_requires_authority_facts_and_reload() {
    let identity = fixture_identity();
    let policy = fixture_policy();
    let synthetic = passing_heldout_bundle(identity.clone());
    assert_eq!(synthetic.evidence_class, EvidenceClass::Synthetic);
    let synthetic_assessment = evaluate_bundle(&synthetic, &policy, false);
    assert_eq!(
        synthetic_assessment.result,
        ContinuityQualificationResult::Unknown
    );
    assert_eq!(
        result_from_assessment(true, EvidenceClass::Synthetic, false, None),
        ContinuityQualificationResult::Unknown
    );

    let empty_facts =
        LiveAdmission::admit_test_evidence(identity.clone(), LiveProvenance::default());
    assert!(empty_facts.is_err(), "empty provenance must not admit");

    let mismatched =
        LiveAdmission::admit_test_evidence(identity.clone(), test_live_provenance()).unwrap();
    let mut other = identity.clone();
    other.model_digest = "model:digest-other".into();
    let wrong_identity = passing_heldout_bundle(other);
    assert!(wrong_identity.authorize_live(mismatched).is_err());

    let admission = LiveAdmission::admit_test_evidence(identity, test_live_provenance()).unwrap();
    let authorized = synthetic.authorize_live(admission).unwrap();
    assert_eq!(authorized.evidence_class, EvidenceClass::LiveAuthorized);
    assert_eq!(
        authorized
            .provenance
            .as_ref()
            .map(|item| item.evidence_label.as_str()),
        Some(SYNTHETIC_TEST_EVIDENCE_LABEL)
    );
    let in_process = evaluate_bundle(&authorized, &policy, false);
    assert_eq!(in_process.result, ContinuityQualificationResult::Unknown);
    emit_oracle(
        "live-marker",
        json!({
            "liveMarkerRequiresNoAuthorityFacts": false,
            "realAdmissionRequired": true,
            "emptyProvenanceRejected": empty_facts.is_err(),
            "identityMismatchRejected": true,
            "syntheticRemainsUnknown": synthetic_assessment.result
                == ContinuityQualificationResult::Unknown,
            "labeledSyntheticTestEvidence": authorized
                .provenance
                .as_ref()
                .is_some_and(|item| item.evidence_label == SYNTHETIC_TEST_EVIDENCE_LABEL),
            "testEvidenceDoesNotPromoteQualification": in_process.result
                == ContinuityQualificationResult::Unknown,
            "realModelQualification": "unknown",
            "externalCalls": 0
        }),
    );
}

fn test_live_provenance() -> LiveProvenance {
    LiveProvenance::labeled_test_evidence("test:evaluation-boundary", "draft-1")
}
