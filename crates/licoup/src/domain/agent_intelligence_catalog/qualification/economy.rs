use super::evidence::{EconomyRole, ObservationEconomy};
use super::owners::token_cost_from_owner;
use super::policy::QualificationPolicy;
use super::wilson::nearest_rank_p95;
use std::collections::{BTreeMap, VecDeque};

#[derive(Clone, Debug, PartialEq)]
pub struct EconomicReport {
    pub candidate_full_cost: Option<f64>,
    pub baseline_full_cost: Option<f64>,
    pub cost_per_accepted_outcome_ratio: Option<f64>,
    pub extra_serial_latency_p95_ms: Option<f64>,
    pub candidate_corrections: u64,
    pub baseline_corrections: u64,
    pub native_success_difference: Option<f64>,
    pub paired_success_difference: Option<f64>,
    pub economically_qualified: bool,
    pub unknown_cost: bool,
}

impl EconomicReport {
    pub fn independent_unknown() -> Self {
        Self {
            candidate_full_cost: None,
            baseline_full_cost: None,
            cost_per_accepted_outcome_ratio: None,
            extra_serial_latency_p95_ms: None,
            candidate_corrections: 0,
            baseline_corrections: 0,
            native_success_difference: None,
            paired_success_difference: None,
            economically_qualified: false,
            unknown_cost: true,
        }
    }
}

pub fn evaluate_economy(
    rows: &[ObservationEconomy],
    policy: &QualificationPolicy,
) -> EconomicReport {
    if rows.is_empty() {
        return EconomicReport::independent_unknown();
    }

    let mut candidate_cost = 0.0;
    let mut baseline_cost = 0.0;
    let mut candidate_corrections = 0_u64;
    let mut baseline_corrections = 0_u64;
    let mut saw_candidate_cost = false;
    let mut saw_baseline_cost = false;
    let mut unknown_cost = false;

    for row in rows {
        match row.role {
            EconomyRole::Candidate => {
                candidate_corrections = candidate_corrections.saturating_add(row.correction_count);
                add_known_cost(
                    resolve_full_cost(row),
                    &mut candidate_cost,
                    &mut saw_candidate_cost,
                    &mut unknown_cost,
                );
            }
            EconomyRole::Baseline => {
                baseline_corrections = baseline_corrections.saturating_add(row.correction_count);
                add_known_cost(
                    resolve_full_cost(row),
                    &mut baseline_cost,
                    &mut saw_baseline_cost,
                    &mut unknown_cost,
                );
            }
            EconomyRole::NativeDirect => {}
        }
    }

    let families = pair_roles(rows, EconomyRole::Candidate, EconomyRole::Baseline);
    let mut paired_candidate_cost = 0.0;
    let mut paired_baseline_cost = 0.0;
    let mut paired_candidate_accepted = 0_u64;
    let mut paired_baseline_accepted = 0_u64;
    let mut saw_paired_candidate_cost = false;
    let mut saw_paired_baseline_cost = false;
    let mut paired_unknown_cost = false;
    let mut extra_latency = Vec::new();

    for (candidate, baseline) in &families {
        if let (Some(left), Some(right)) = (candidate.serial_latency_ms, baseline.serial_latency_ms)
        {
            extra_latency.push(left.saturating_sub(right));
        }
        if candidate.accepted_outcome {
            paired_candidate_accepted += 1;
        }
        if baseline.accepted_outcome {
            paired_baseline_accepted += 1;
        }
        match resolve_full_cost(candidate) {
            Some(value) => {
                paired_candidate_cost += value;
                saw_paired_candidate_cost = true;
            }
            None => paired_unknown_cost = true,
        }
        match resolve_full_cost(baseline) {
            Some(value) => {
                paired_baseline_cost += value;
                saw_paired_baseline_cost = true;
            }
            None => paired_unknown_cost = true,
        }
    }

    unknown_cost |= paired_unknown_cost;
    let candidate_cpao =
        (!paired_unknown_cost && saw_paired_candidate_cost && paired_candidate_accepted > 0)
            .then(|| paired_candidate_cost / paired_candidate_accepted as f64);
    let baseline_cpao =
        (!paired_unknown_cost && saw_paired_baseline_cost && paired_baseline_accepted > 0)
            .then(|| paired_baseline_cost / paired_baseline_accepted as f64);
    let ratio = match (candidate_cpao, baseline_cpao) {
        (Some(candidate), Some(baseline)) if baseline > 0.0 => Some(candidate / baseline),
        _ => None,
    };

    let candidate_success = families
        .iter()
        .filter(|(candidate, _)| candidate.accepted_outcome)
        .count();
    let baseline_success = families
        .iter()
        .filter(|(_, baseline)| baseline.accepted_outcome)
        .count();
    let paired_success_difference = (!families.is_empty()).then(|| {
        candidate_success as f64 / families.len() as f64
            - baseline_success as f64 / families.len() as f64
    });
    let native_pairs = pair_roles(rows, EconomyRole::Candidate, EconomyRole::NativeDirect);
    let native_success_difference = (!native_pairs.is_empty()).then(|| {
        let candidate = native_pairs
            .iter()
            .filter(|(row, _)| row.accepted_outcome)
            .count() as f64;
        let native = native_pairs
            .iter()
            .filter(|(_, row)| row.accepted_outcome)
            .count() as f64;
        candidate / native_pairs.len() as f64 - native / native_pairs.len() as f64
    });

    let extra_serial_latency_p95_ms = nearest_rank_p95(&extra_latency);
    let success_ok = paired_success_difference
        .is_some_and(|diff| diff + policy.success_noninferiority_margin >= 0.0);
    let cost_ok = ratio.is_some_and(|value| value <= policy.cost_per_accepted_outcome_ratio_max);
    let latency_ok = extra_serial_latency_p95_ms
        .is_some_and(|value| value <= policy.foreground_added_latency_p95_ms_max);

    EconomicReport {
        candidate_full_cost: saw_candidate_cost.then_some(candidate_cost),
        baseline_full_cost: saw_baseline_cost.then_some(baseline_cost),
        cost_per_accepted_outcome_ratio: ratio,
        extra_serial_latency_p95_ms,
        candidate_corrections,
        baseline_corrections,
        native_success_difference,
        paired_success_difference,
        economically_qualified: !unknown_cost && success_ok && cost_ok && latency_ok,
        unknown_cost,
    }
}

pub fn resolve_full_cost(row: &ObservationEconomy) -> Option<f64> {
    if let Some(measured) = row.measured_full_cost {
        return Some(measured);
    }
    let components = [
        row.classification_cost,
        row.retrieval_cost,
        row.escalation_cost,
        row.execution_cost,
        row.retry_cost,
        row.rework_cost,
    ];
    if components.iter().all(Option::is_some) {
        return Some(components.into_iter().flatten().sum());
    }
    match (row.input_tokens, row.output_tokens) {
        (Some(input), Some(output)) => token_cost_from_owner(
            row.model_id.as_deref(),
            row.agent_id.as_deref(),
            row.thinking.as_deref(),
            input,
            output,
        ),
        _ => None,
    }
}

fn add_known_cost(cost: Option<f64>, total: &mut f64, saw: &mut bool, unknown_cost: &mut bool) {
    match cost {
        Some(value) => {
            *total += value;
            *saw = true;
        }
        None => *unknown_cost = true,
    }
}

fn pair_roles(
    rows: &[ObservationEconomy],
    left: EconomyRole,
    right: EconomyRole,
) -> Vec<(&ObservationEconomy, &ObservationEconomy)> {
    let mut rights: BTreeMap<(&str, &str, &str), VecDeque<&ObservationEconomy>> = BTreeMap::new();
    for row in rows {
        if row.role != right {
            continue;
        }
        let Some(identity) = economy_identity(row) else {
            continue;
        };
        rights.entry(identity).or_default().push_back(row);
    }

    let mut pairs = Vec::new();
    for row in rows {
        if row.role != left {
            continue;
        }
        let Some(identity) = economy_identity(row) else {
            continue;
        };
        if let Some(matched) = rights.get_mut(&identity).and_then(VecDeque::pop_front) {
            pairs.push((row, matched));
        }
    }
    pairs
}

fn economy_identity(row: &ObservationEconomy) -> Option<(&str, &str, &str)> {
    let task = row
        .task_identity
        .as_deref()
        .filter(|value| !value.trim().is_empty())?;
    let version = row
        .version_identity
        .as_deref()
        .filter(|value| !value.trim().is_empty())?;
    let resource = row
        .resource_identity
        .as_deref()
        .filter(|value| !value.trim().is_empty())?;
    Some((task, version, resource))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(
        role: EconomyRole,
        task: &str,
        cost: Option<f64>,
        accepted: bool,
        latency_ms: Option<u64>,
    ) -> ObservationEconomy {
        ObservationEconomy {
            role,
            classification_cost: None,
            retrieval_cost: None,
            escalation_cost: None,
            execution_cost: None,
            retry_cost: None,
            rework_cost: None,
            measured_full_cost: cost,
            input_tokens: None,
            output_tokens: None,
            model_id: None,
            agent_id: None,
            thinking: None,
            serial_latency_ms: latency_ms,
            correction_count: 0,
            accepted_outcome: accepted,
            task_identity: Some(task.to_owned()),
            version_identity: Some("v1".to_owned()),
            resource_identity: Some("res:1".to_owned()),
        }
    }

    #[test]
    fn matched_unknown_cost_is_not_zero_and_does_not_qualify() {
        let report = evaluate_economy(
            &[
                row(EconomyRole::Candidate, "task:paired", None, true, Some(10)),
                row(
                    EconomyRole::Baseline,
                    "task:paired",
                    Some(10.0),
                    true,
                    Some(10),
                ),
                row(
                    EconomyRole::Candidate,
                    "task:other",
                    Some(5.0),
                    true,
                    Some(10),
                ),
            ],
            &QualificationPolicy::draft_1(),
        );
        assert!(report.unknown_cost);
        assert_eq!(report.candidate_full_cost, Some(5.0));
        assert_eq!(report.baseline_full_cost, Some(10.0));
        assert_ne!(report.candidate_full_cost, Some(0.0));
        assert!(report.cost_per_accepted_outcome_ratio.is_none());
        assert!(!report.economically_qualified);
    }

    #[test]
    fn duplicate_identity_pairs_baselines_in_encounter_order() {
        let report = evaluate_economy(
            &[
                row(
                    EconomyRole::Candidate,
                    "task:same",
                    Some(1.0),
                    true,
                    Some(100),
                ),
                row(
                    EconomyRole::Candidate,
                    "task:same",
                    Some(1.0),
                    true,
                    Some(10),
                ),
                row(
                    EconomyRole::Baseline,
                    "task:same",
                    Some(1.0),
                    true,
                    Some(10),
                ),
                row(
                    EconomyRole::Baseline,
                    "task:same",
                    Some(1.0),
                    true,
                    Some(100),
                ),
            ],
            &QualificationPolicy::draft_1(),
        );
        assert_eq!(report.extra_serial_latency_p95_ms, Some(90.0));
        assert_eq!(report.cost_per_accepted_outcome_ratio, Some(1.0));
        assert_eq!(report.paired_success_difference, Some(0.0));
    }

    fn native_row(
        role: EconomyRole,
        task: &str,
        version: &str,
        resource: &str,
        cost: Option<f64>,
        accepted: bool,
    ) -> ObservationEconomy {
        ObservationEconomy {
            role,
            classification_cost: None,
            retrieval_cost: None,
            escalation_cost: None,
            execution_cost: None,
            retry_cost: None,
            rework_cost: None,
            measured_full_cost: cost,
            input_tokens: None,
            output_tokens: None,
            model_id: None,
            agent_id: None,
            thinking: None,
            serial_latency_ms: Some(10),
            correction_count: 0,
            accepted_outcome: accepted,
            task_identity: Some(task.to_owned()),
            version_identity: Some(version.to_owned()),
            resource_identity: Some(resource.to_owned()),
        }
    }

    #[test]
    fn direct_native_pairs_equivalent_identity_and_excludes_unmatched() {
        let report = evaluate_economy(
            &[
                native_row(
                    EconomyRole::Candidate,
                    "task:matched",
                    "v1",
                    "res:1",
                    Some(2.0),
                    true,
                ),
                native_row(
                    EconomyRole::Baseline,
                    "task:matched",
                    "v1",
                    "res:1",
                    Some(3.0),
                    true,
                ),
                native_row(
                    EconomyRole::NativeDirect,
                    "task:matched",
                    "v1",
                    "res:1",
                    Some(50.0),
                    false,
                ),
                native_row(
                    EconomyRole::Candidate,
                    "task:full-chain-only",
                    "v1",
                    "res:1",
                    Some(9.0),
                    true,
                ),
                native_row(
                    EconomyRole::NativeDirect,
                    "task:direct-only",
                    "v2",
                    "res:other",
                    Some(50.0),
                    true,
                ),
                native_row(
                    EconomyRole::NativeDirect,
                    "task:matched",
                    "v2",
                    "res:1",
                    Some(7.0),
                    true,
                ),
            ],
            &QualificationPolicy::draft_1(),
        );
        assert_eq!(report.native_success_difference, Some(1.0));
        assert_eq!(report.paired_success_difference, Some(0.0));
        assert_eq!(report.candidate_full_cost, Some(11.0));
        assert_eq!(report.baseline_full_cost, Some(3.0));
        assert_ne!(report.candidate_full_cost, Some(61.0));
        assert_ne!(report.native_success_difference, Some(0.5));
    }
}
