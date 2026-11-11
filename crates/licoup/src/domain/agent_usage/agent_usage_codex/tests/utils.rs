use super::super::models::TokenTotals;
use super::super::utils::{from_i64, to_i64, totals_columns, totals_from_columns, turn_id};
use serde_json::json;

#[test]
fn numeric_storage_helpers_saturate_and_round_trip_totals() {
    assert_eq!(to_i64(u64::MAX), i64::MAX);
    assert_eq!(from_i64(-1), 0);
    let totals = TokenTotals {
        input: 9,
        cached: 3,
        output: 4,
    };
    assert_eq!(
        totals_from_columns(totals_columns(Some(totals))),
        Some(totals)
    );
    assert_eq!(totals_from_columns((None, None, None)), None);
}

#[test]
fn turn_identity_accepts_direct_and_nested_fields() {
    assert_eq!(
        turn_id(&json!({"turnId": "direct"})).as_deref(),
        Some("direct")
    );
    assert_eq!(
        turn_id(&json!({"info": {"turn_id": "nested"}})).as_deref(),
        Some("nested")
    );
}
