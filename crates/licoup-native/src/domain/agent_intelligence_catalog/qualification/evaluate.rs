use licoup_conversation::continuity::{
    ContinuityDatasetSplit, ContinuityQualificationRecord, ContinuityQualificationResult,
};
use std::collections::{BTreeMap, BTreeSet};

use super::economy::{EconomicReport, evaluate_economy};
use super::evidence::{
    EvidenceBundle, EvidenceClass, LiveProvenance, ObservationPolarity, QualificationObservation,
};
use super::policy::QualificationPolicy;
use super::wilson::one_sided_wilson_upper;

#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd)]
pub enum UnqualifiedReason {
    ZeroHeldoutSamples,
    FamilySplitLeak,
    BelowMinimumNegatives,
    BelowMinimumPositives,
    CoverageBelowMinimum,
    AllAbstain,
    FalseTakeoverBound,
    MissedCommitmentBound,
    SubgroupShortage,
    SubgroupRawError,
    HardInvariant,
    WorkerOrTurnExitClosure,
}

#[derive(Clone, Debug, PartialEq)]
pub struct RateReport {
    pub errors: u64,
    pub opportunities: u64,
    pub point: Option<f64>,
    pub wilson_upper: Option<f64>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct QualificationAssessment {
    pub observation_count: u64,
    pub policy_revision: String,
    pub evidence_class: EvidenceClass,
    pub policy_pass: bool,
    pub stale: bool,
    pub result: ContinuityQualificationResult,
    pub reasons: Vec<UnqualifiedReason>,
    pub false_takeover: RateReport,
    pub missed_commitment: RateReport,
    pub coverage: RateReport,
    pub subgroup_counts: BTreeMap<String, u64>,
    pub economy: EconomicReport,
}

impl RateReport {
    fn from_counts(errors: u64, opportunities: u64, z: f64) -> Self {
        let point = (opportunities > 0).then(|| errors as f64 / opportunities as f64);
        Self {
            errors,
            opportunities,
            point,
            wilson_upper: one_sided_wilson_upper(errors, opportunities, z),
        }
    }
}

pub fn evaluate_bundle(
    bundle: &EvidenceBundle,
    policy: &QualificationPolicy,
    stale: bool,
) -> QualificationAssessment {
    let mut reasons = Vec::new();
    if stale {
        return assessment_with_result(
            bundle,
            policy,
            stale,
            ContinuityQualificationResult::Stale,
            reasons,
            empty_rates(policy.z),
            evaluate_economy(&[], policy),
        );
    }

    let leak = family_leak(&bundle.observations);
    if leak {
        reasons.push(UnqualifiedReason::FamilySplitLeak);
    }

    let heldout: Vec<&QualificationObservation> = bundle
        .observations
        .iter()
        .filter(|observation| observation.split == ContinuityDatasetSplit::Heldout)
        .collect();

    let negative_families = unique_families(
        heldout
            .iter()
            .filter(|observation| observation.polarity == ObservationPolarity::Negative)
            .copied(),
    );
    let positive_families = unique_families(
        heldout
            .iter()
            .filter(|observation| observation.polarity == ObservationPolarity::Positive)
            .copied(),
    );
    let all_families = unique_families(heldout.iter().copied());
    let false_families = unique_families(heldout.iter().filter(|o| o.is_false_takeover()).copied());
    let missed_families =
        unique_families(heldout.iter().filter(|o| o.is_missed_commitment()).copied());
    let judged_families = unique_families(heldout.iter().filter(|o| !o.is_abstain()).copied());

    if all_families.is_empty() {
        reasons.push(UnqualifiedReason::ZeroHeldoutSamples);
    }
    if (negative_families.len() as u64) < policy.minimum_negative_cases {
        reasons.push(UnqualifiedReason::BelowMinimumNegatives);
    }
    if (positive_families.len() as u64) < policy.minimum_positive_cases {
        reasons.push(UnqualifiedReason::BelowMinimumPositives);
    }

    let false_takeover = RateReport::from_counts(
        false_families.len() as u64,
        negative_families.len() as u64,
        policy.z,
    );
    let missed_commitment = RateReport::from_counts(
        missed_families.len() as u64,
        positive_families.len() as u64,
        policy.z,
    );
    let abstain_count = all_families.len().saturating_sub(judged_families.len()) as u64;
    let coverage = RateReport::from_counts(abstain_count, all_families.len() as u64, policy.z);
    let coverage_rate = judged_families.len() as f64 / (all_families.len().max(1) as f64);
    if all_families.is_empty() {
        // coverage remains unknown; zero-sample reason already recorded
    } else if judged_families.is_empty() {
        reasons.push(UnqualifiedReason::AllAbstain);
        reasons.push(UnqualifiedReason::CoverageBelowMinimum);
    } else if coverage_rate < policy.non_abstain_coverage_min {
        reasons.push(UnqualifiedReason::CoverageBelowMinimum);
    }

    if false_takeover
        .wilson_upper
        .is_none_or(|upper| upper > policy.false_takeover_upper_max)
        && !negative_families.is_empty()
    {
        reasons.push(UnqualifiedReason::FalseTakeoverBound);
    }
    if missed_commitment
        .wilson_upper
        .is_none_or(|upper| upper > policy.missed_commitment_upper_max)
        && !positive_families.is_empty()
    {
        reasons.push(UnqualifiedReason::MissedCommitmentBound);
    }

    let subgroup_counts = subgroup_family_counts(&heldout);
    for required in &policy.required_subgroups {
        let count = subgroup_counts.get(required).copied().unwrap_or(0);
        if count < policy.minimum_cases_per_required_subgroup {
            reasons.push(UnqualifiedReason::SubgroupShortage);
            break;
        }
    }
    for required in &policy.required_subgroups {
        let members: Vec<&QualificationObservation> = heldout
            .iter()
            .copied()
            .filter(|observation| observation.subgroup == *required)
            .collect();
        let families = unique_families(members.iter().copied());
        if families.is_empty() {
            continue;
        }
        let errors = unique_families(members.iter().copied().filter(|o| o.is_raw_error()));
        let raw = errors.len() as f64 / families.len() as f64;
        if raw > policy.subgroup_raw_error_max {
            reasons.push(UnqualifiedReason::SubgroupRawError);
            break;
        }
    }

    let mut invariants = super::evidence::HardInvariantCounts::default();
    let mut exit_closures = 0_u64;
    for observation in &heldout {
        invariants.unapproved_disclosures = invariants
            .unapproved_disclosures
            .saturating_add(observation.hard_invariants.unapproved_disclosures);
        invariants.known_duplicate_effects = invariants
            .known_duplicate_effects
            .saturating_add(observation.hard_invariants.known_duplicate_effects);
        invariants.rewritten_authorship = invariants
            .rewritten_authorship
            .saturating_add(observation.hard_invariants.rewritten_authorship);
        invariants.deleted_source_resurrections = invariants
            .deleted_source_resurrections
            .saturating_add(observation.hard_invariants.deleted_source_resurrections);
        exit_closures = exit_closures.saturating_add(observation.fabricated_from_exit());
    }
    invariants.fabricated_goal_closures = exit_closures;
    if invariants.unapproved_disclosures > policy.unapproved_disclosures_max
        || invariants.known_duplicate_effects > policy.known_duplicate_effects_max
        || invariants.fabricated_goal_closures > policy.fabricated_goal_closures_max
        || invariants.rewritten_authorship > policy.rewritten_authorship_max
        || invariants.deleted_source_resurrections > policy.deleted_source_resurrections_max
    {
        reasons.push(UnqualifiedReason::HardInvariant);
    }
    if exit_closures > 0 {
        reasons.push(UnqualifiedReason::WorkerOrTurnExitClosure);
    }

    reasons.sort();
    reasons.dedup();
    let policy_pass = reasons.is_empty();
    let result = result_from_assessment(
        policy_pass,
        bundle.evidence_class,
        all_families.is_empty(),
        bundle.provenance.as_ref(),
    );
    let economy = evaluate_economy(
        &heldout
            .iter()
            .filter_map(|observation| observation.economy.as_ref())
            .cloned()
            .collect::<Vec<_>>(),
        policy,
    );
    QualificationAssessment {
        observation_count: all_families.len() as u64,
        policy_revision: policy.policy_revision.clone(),
        evidence_class: bundle.evidence_class,
        policy_pass,
        stale: false,
        result,
        reasons,
        false_takeover,
        missed_commitment,
        coverage: RateReport {
            errors: abstain_count,
            opportunities: all_families.len() as u64,
            point: (!all_families.is_empty()).then_some(coverage_rate),
            wilson_upper: coverage.wilson_upper,
        },
        subgroup_counts,
        economy,
    }
}

pub fn result_from_assessment(
    policy_pass: bool,
    evidence_class: EvidenceClass,
    zero_samples: bool,
    provenance: Option<&LiveProvenance>,
) -> ContinuityQualificationResult {
    if zero_samples {
        return ContinuityQualificationResult::Unknown;
    }
    if !policy_pass {
        return ContinuityQualificationResult::Unqualified;
    }
    if evidence_class != EvidenceClass::LiveAuthorized {
        return ContinuityQualificationResult::Unknown;
    }
    if provenance.is_some_and(LiveProvenance::is_stored_authority) {
        return ContinuityQualificationResult::Qualified;
    }
    ContinuityQualificationResult::Unknown
}

pub fn apply_assessment(
    query: &ContinuityQualificationRecord,
    assessment: &QualificationAssessment,
) -> ContinuityQualificationRecord {
    ContinuityQualificationRecord {
        responsibility_id: query.responsibility_id.clone(),
        candidate_identity: query.candidate_identity.clone(),
        dataset_family_split: ContinuityDatasetSplit::Heldout,
        observation_count: assessment.observation_count,
        policy_revision: assessment.policy_revision.clone(),
        result: assessment.result,
        expires_at: None,
        revoked: query.revoked,
    }
}

fn unique_families<'a, I>(observations: I) -> BTreeSet<String>
where
    I: Iterator<Item = &'a QualificationObservation>,
{
    observations
        .map(|observation| observation.conversation_family.clone())
        .collect()
}

fn subgroup_family_counts(heldout: &[&QualificationObservation]) -> BTreeMap<String, u64> {
    let mut families: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    for observation in heldout {
        families
            .entry(observation.subgroup.clone())
            .or_default()
            .insert(observation.conversation_family.clone());
    }
    families
        .into_iter()
        .map(|(subgroup, families)| (subgroup, families.len() as u64))
        .collect()
}

fn family_leak(observations: &[QualificationObservation]) -> bool {
    let mut development = BTreeSet::new();
    let mut heldout = BTreeSet::new();
    let mut calibration = BTreeSet::new();
    for observation in observations {
        match observation.split {
            ContinuityDatasetSplit::Development => {
                development.insert(observation.conversation_family.as_str());
            }
            ContinuityDatasetSplit::Heldout => {
                heldout.insert(observation.conversation_family.as_str());
            }
            ContinuityDatasetSplit::Calibration => {
                calibration.insert(observation.conversation_family.as_str());
            }
        }
    }
    !development.is_disjoint(&heldout)
        || !development.is_disjoint(&calibration)
        || !heldout.is_disjoint(&calibration)
}

fn empty_rates(z: f64) -> [RateReport; 3] {
    [
        RateReport::from_counts(0, 0, z),
        RateReport::from_counts(0, 0, z),
        RateReport::from_counts(0, 0, z),
    ]
}

fn assessment_with_result(
    bundle: &EvidenceBundle,
    policy: &QualificationPolicy,
    stale: bool,
    result: ContinuityQualificationResult,
    reasons: Vec<UnqualifiedReason>,
    rates: [RateReport; 3],
    economy: EconomicReport,
) -> QualificationAssessment {
    QualificationAssessment {
        observation_count: bundle.observations.len() as u64,
        policy_revision: policy.policy_revision.clone(),
        evidence_class: bundle.evidence_class,
        policy_pass: false,
        stale,
        result,
        reasons,
        false_takeover: rates[0].clone(),
        missed_commitment: rates[1].clone(),
        coverage: rates[2].clone(),
        subgroup_counts: BTreeMap::new(),
        economy,
    }
}
