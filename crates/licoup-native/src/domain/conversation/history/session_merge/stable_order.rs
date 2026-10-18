use serde_json::Value;
use time::{OffsetDateTime, format_description::well_known::Rfc3339};

use super::dedupe_paging::history_session_dedupe_key;

pub(super) fn message_role(message: &Value) -> String {
    message
        .get("role")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_ascii_lowercase()
}

pub(super) fn message_order_key(message: &Value) -> Option<i128> {
    message.get("createdAt").and_then(history_value_order_key)
}

pub(super) fn history_value_order_key(value: &Value) -> Option<i128> {
    match value {
        Value::String(text) => history_time_order_key(text),
        Value::Number(number) => number
            .as_i64()
            .map(i128::from)
            .or_else(|| number.as_u64().map(|value| value as i128))
            .or_else(|| number.as_f64().map(|value| value as i128)),
        _ => None,
    }
}

pub(super) fn history_time_order_key(value: &str) -> Option<i128> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return None;
    }
    if let Ok(timestamp) = OffsetDateTime::parse(trimmed, &Rfc3339) {
        return Some(
            timestamp.unix_timestamp() as i128 * 1_000_000_000 + timestamp.nanosecond() as i128,
        );
    }
    trimmed.parse::<i128>().ok()
}

pub(crate) fn sort_sessions_by_updated_at(sessions: &mut [Value]) {
    sessions.sort_by(|left, right| {
        session_updated_order_key(right)
            .cmp(&session_updated_order_key(left))
            .then_with(|| history_session_dedupe_key(left).cmp(&history_session_dedupe_key(right)))
    });
}

pub(super) fn session_updated_order_key(session: &Value) -> i128 {
    session
        .get("messages")
        .and_then(Value::as_array)
        .and_then(|messages| messages.iter().filter_map(message_order_key).max())
        .or_else(|| session.get("updatedAt").and_then(history_value_order_key))
        .or_else(|| session.get("createdAt").and_then(history_value_order_key))
        .or_else(|| {
            session
                .get("messages")
                .and_then(Value::as_array)
                .and_then(|messages| messages.iter().rev().filter_map(message_order_key).next())
        })
        .unwrap_or(0)
}
