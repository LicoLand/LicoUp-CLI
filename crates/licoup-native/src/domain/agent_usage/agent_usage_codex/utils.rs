use super::models::TokenTotals;
use licoup_statistics::contract::text_field;
use serde_json::Value;
use std::time::{SystemTime, UNIX_EPOCH};

pub(super) fn turn_id(value: &Value) -> Option<String> {
    text_field(value, &["turn_id", "turnId", "id"]).or_else(|| {
        value
            .get("info")
            .and_then(|info| text_field(info, &["turn_id", "turnId", "id"]))
    })
}

pub(super) fn totals_columns(
    value: Option<TokenTotals>,
) -> (Option<i64>, Option<i64>, Option<i64>) {
    match value {
        Some(value) => (
            Some(to_i64(value.input)),
            Some(to_i64(value.cached)),
            Some(to_i64(value.output)),
        ),
        None => (None, None, None),
    }
}

pub(super) fn totals_from_columns(
    values: (Option<i64>, Option<i64>, Option<i64>),
) -> Option<TokenTotals> {
    if values.0.is_none() && values.1.is_none() && values.2.is_none() {
        return None;
    }
    Some(TokenTotals {
        input: values.0.map(from_i64).unwrap_or(0),
        cached: values.1.map(from_i64).unwrap_or(0),
        output: values.2.map(from_i64).unwrap_or(0),
    })
}

pub(super) fn unix_millis() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .min(u64::MAX as u128) as u64
}

pub(super) fn to_i64(value: u64) -> i64 {
    value.min(i64::MAX as u64) as i64
}

pub(super) fn from_i64(value: i64) -> u64 {
    value.max(0) as u64
}
