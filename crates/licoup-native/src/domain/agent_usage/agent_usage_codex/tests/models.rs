use super::super::models::TokenTotals;
use serde_json::json;

#[test]
fn token_totals_normalize_aliases_and_bound_cached_input() {
    let totals = TokenTotals::from_value(&json!({
        "promptTokens": 8,
        "cacheReadInputTokens": 12,
        "completionTokens": 3
    }))
    .unwrap();
    assert_eq!(totals.input, 8);
    assert_eq!(totals.cached, 8);
    assert_eq!(totals.output, 3);
}

#[test]
fn token_totals_use_saturating_delta_and_addition() {
    let baseline = TokenTotals {
        input: 10,
        cached: 4,
        output: 5,
    };
    let current = TokenTotals {
        input: 16,
        cached: 6,
        output: 8,
    };
    let delta = current.saturating_delta(baseline);
    assert_eq!(delta.input, 6);
    assert_eq!(delta.cached, 2);
    assert_eq!(delta.output, 3);
    assert_eq!(baseline.add(delta), current);
}
