//! One-sided Wilson score upper bound.
//!
//! Formula matches `blueprints/04-EVALUATION.md` and the Wikipedia
//! [Wilson score interval](https://en.wikipedia.org/wiki/Binomial_proportion_confidence_interval#Wilson_score_interval)
//! upper bound. The z used here is Φ⁻¹(0.95) = 1.6448536269514722, which is
//! the one-sided 95% critical value (equivalent to a two-sided 90% interval).
//! Continuity correction is not applied (`wilsoncc` is a different estimator).

/// Φ⁻¹(0.95). Same literal as `fixtures/evaluation-policy.json`.
pub const WILSON_Z_ONE_SIDED_95: f64 = 1.6448536269514722;

/// One-sided Wilson upper bound for `k` errors in `n` opportunities.
///
/// `n == 0` is unknown: the function returns `None` and callers must not treat
/// a missing sample as a passing rate.
pub fn one_sided_wilson_upper(errors: u64, opportunities: u64, z: f64) -> Option<f64> {
    if opportunities == 0 || !z.is_finite() || z <= 0.0 {
        return None;
    }
    let n = opportunities as f64;
    let k = errors as f64;
    if k > n {
        return None;
    }
    let p = k / n;
    let z2 = z * z;
    let variance_term = p * (1.0 - p) / n + z2 / (4.0 * n * n);
    if variance_term < 0.0 {
        return None;
    }
    let numerator = p + z2 / (2.0 * n) + z * variance_term.sqrt();
    let denominator = 1.0 + z2 / n;
    if denominator == 0.0 {
        return None;
    }
    Some(numerator / denominator)
}

/// Nearest-rank p95 (NIST / Wikipedia nearest-rank). Empty input is unknown.
pub fn nearest_rank_p95(samples_ms: &[u64]) -> Option<f64> {
    if samples_ms.is_empty() {
        return None;
    }
    let mut ordered = samples_ms.to_vec();
    ordered.sort_unstable();
    let rank = ((0.95 * ordered.len() as f64).ceil() as usize).clamp(1, ordered.len());
    Some(ordered[rank - 1] as f64)
}
