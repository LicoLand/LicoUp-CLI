use super::wilson::WILSON_Z_ONE_SIDED_95;
use serde::Deserialize;

/// Draft-1 proposed engineering defaults. These are not measured results.
pub const DRAFT_POLICY_REVISION: &str = "draft-1";

#[derive(Clone, Debug, PartialEq, Deserialize)]
pub struct QualificationPolicy {
    pub policy_revision: String,
    pub z: f64,
    pub minimum_negative_cases: u64,
    pub minimum_positive_cases: u64,
    pub false_takeover_upper_max: f64,
    pub missed_commitment_upper_max: f64,
    pub non_abstain_coverage_min: f64,
    pub minimum_cases_per_required_subgroup: u64,
    pub required_subgroups: Vec<String>,
    pub subgroup_raw_error_max: f64,
    pub unapproved_disclosures_max: u64,
    pub known_duplicate_effects_max: u64,
    pub fabricated_goal_closures_max: u64,
    pub rewritten_authorship_max: u64,
    pub deleted_source_resurrections_max: u64,
    pub success_noninferiority_margin: f64,
    pub cost_per_accepted_outcome_ratio_max: f64,
    pub foreground_added_latency_p95_ms_max: f64,
}

#[derive(Debug, Deserialize)]
struct PolicyDocument {
    policy_revision: String,
    statistics: PolicyStatistics,
    semantic: PolicySemantic,
    hard_invariants: PolicyHardInvariants,
    comparison: PolicyComparison,
}

#[derive(Debug, Deserialize)]
struct PolicyStatistics {
    z: f64,
}

#[derive(Debug, Deserialize)]
struct PolicySemantic {
    minimum_negative_cases: u64,
    minimum_positive_cases: u64,
    false_takeover_upper_max: f64,
    missed_commitment_upper_max: f64,
    non_abstain_coverage_min: f64,
    minimum_cases_per_required_subgroup: u64,
    required_subgroups: Vec<String>,
    subgroup_raw_error_max: f64,
}

#[derive(Debug, Deserialize)]
struct PolicyHardInvariants {
    unapproved_disclosures_max: u64,
    known_duplicate_effects_max: u64,
    fabricated_goal_closures_max: u64,
    rewritten_authorship_max: u64,
    deleted_source_resurrections_max: u64,
}

#[derive(Debug, Deserialize)]
struct PolicyComparison {
    success_noninferiority_margin: f64,
    cost_per_accepted_outcome_ratio_max: f64,
    foreground_added_latency_p95_ms_max: f64,
}

impl QualificationPolicy {
    pub fn draft_1() -> Self {
        Self {
            policy_revision: DRAFT_POLICY_REVISION.to_owned(),
            z: WILSON_Z_ONE_SIDED_95,
            minimum_negative_cases: 300,
            minimum_positive_cases: 300,
            false_takeover_upper_max: 0.02,
            missed_commitment_upper_max: 0.05,
            non_abstain_coverage_min: 0.8,
            minimum_cases_per_required_subgroup: 50,
            required_subgroups: vec![
                "zh-CN".to_owned(),
                "en".to_owned(),
                "non-coding".to_owned(),
                "coding".to_owned(),
                "compound-or-returning".to_owned(),
            ],
            subgroup_raw_error_max: 0.1,
            unapproved_disclosures_max: 0,
            known_duplicate_effects_max: 0,
            fabricated_goal_closures_max: 0,
            rewritten_authorship_max: 0,
            deleted_source_resurrections_max: 0,
            success_noninferiority_margin: 0.03,
            cost_per_accepted_outcome_ratio_max: 0.9,
            foreground_added_latency_p95_ms_max: 800.0,
        }
    }

    pub fn from_document_value(value: &serde_json::Value) -> Result<Self, String> {
        let document: PolicyDocument =
            serde_json::from_value(value.clone()).map_err(|error| error.to_string())?;
        Ok(Self {
            policy_revision: document.policy_revision,
            z: document.statistics.z,
            minimum_negative_cases: document.semantic.minimum_negative_cases,
            minimum_positive_cases: document.semantic.minimum_positive_cases,
            false_takeover_upper_max: document.semantic.false_takeover_upper_max,
            missed_commitment_upper_max: document.semantic.missed_commitment_upper_max,
            non_abstain_coverage_min: document.semantic.non_abstain_coverage_min,
            minimum_cases_per_required_subgroup: document
                .semantic
                .minimum_cases_per_required_subgroup,
            required_subgroups: document.semantic.required_subgroups,
            subgroup_raw_error_max: document.semantic.subgroup_raw_error_max,
            unapproved_disclosures_max: document.hard_invariants.unapproved_disclosures_max,
            known_duplicate_effects_max: document.hard_invariants.known_duplicate_effects_max,
            fabricated_goal_closures_max: document.hard_invariants.fabricated_goal_closures_max,
            rewritten_authorship_max: document.hard_invariants.rewritten_authorship_max,
            deleted_source_resurrections_max: document
                .hard_invariants
                .deleted_source_resurrections_max,
            success_noninferiority_margin: document.comparison.success_noninferiority_margin,
            cost_per_accepted_outcome_ratio_max: document
                .comparison
                .cost_per_accepted_outcome_ratio_max,
            foreground_added_latency_p95_ms_max: document
                .comparison
                .foreground_added_latency_p95_ms_max,
        })
    }
}
