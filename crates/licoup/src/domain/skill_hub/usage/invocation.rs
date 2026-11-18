use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};

const MAX_INVOCATIONS_PER_TURN: usize = 256;
const MAX_EVENTS_SCANNED_PER_TURN: usize = 4096;

/// Extract only the normalized event contract emitted by runtime adapters.
/// Free text, generic tool calls, tool updates, and failed turns never count.
pub(super) fn invocation_counts(result: &Value) -> BTreeMap<String, u64> {
    if result.get("ok").and_then(Value::as_bool) != Some(true) {
        return BTreeMap::new();
    }
    let mut counts = BTreeMap::<String, u64>::new();
    let mut seen_invocations = BTreeSet::<String>::new();
    let mut accepted = 0_usize;
    let events = result
        .get("events")
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or_default();
    for event in events.iter().take(MAX_EVENTS_SCANNED_PER_TURN) {
        if accepted == MAX_INVOCATIONS_PER_TURN {
            break;
        }
        if event.get("event").and_then(Value::as_str) != Some("skill.invoked") {
            continue;
        }
        let Some(skill_id) = event.get("skillId").and_then(Value::as_str) else {
            continue;
        };
        let Some(skill_id) = safe_skill_id(skill_id) else {
            continue;
        };
        let invocation_digest = match event.get("invocationIdDigest") {
            None => None,
            Some(Value::String(value)) => match safe_invocation_digest(value) {
                Some(value) => Some(value),
                None => continue,
            },
            Some(_) => continue,
        };
        if invocation_digest.is_some_and(|digest| !seen_invocations.insert(digest.to_string())) {
            continue;
        }
        *counts.entry(skill_id).or_default() += 1;
        accepted += 1;
    }
    counts
}

fn safe_skill_id(value: &str) -> Option<String> {
    let trimmed = value.trim();
    (!trimmed.is_empty()
        && trimmed.len() <= 128
        && trimmed
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_')))
    .then(|| trimmed.to_ascii_lowercase())
}

fn safe_invocation_digest(value: &str) -> Option<&str> {
    let digest = value.strip_prefix("sha256:")?;
    (digest.len() == 64
        && digest
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte)))
    .then_some(value)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn only_successful_normalized_invocation_events_count() {
        let counts = invocation_counts(&json!({
            "ok": true,
            "events": [
                {"event": "skill.invoked", "skillId": "review-helper"},
                {"event": "skill.invoked", "skillId": "review-helper"},
                {"sessionUpdate": "tool_call", "skillId": "not-normalized"},
                {"event": "skill.invoked", "skillId": "invalid.skill.id"}
            ],
            "output": "unobserved conversation content"
        }));
        assert_eq!(counts.get("review-helper"), Some(&2));
        assert_eq!(counts.len(), 1);
        assert!(
            invocation_counts(&json!({
                "ok": false,
                "events": [{"event": "skill.invoked", "skillId": "review-helper"}]
            }))
            .is_empty()
        );
    }

    #[test]
    fn counts_call_instances_and_deduplicates_streamed_repeats() {
        const CALL_A: &str =
            "sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
        const CALL_B: &str =
            "sha256:bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";

        let distinct = invocation_counts(&json!({
            "ok": true,
            "events": [
                {"event": "skill.invoked", "skillId": "review-helper", "invocationIdDigest": CALL_A},
                {"event": "skill.invoked", "skillId": "review-helper", "invocationIdDigest": CALL_B}
            ]
        }));
        assert_eq!(distinct.get("review-helper"), Some(&2));

        let streamed_repeat = invocation_counts(&json!({
            "ok": true,
            "events": [
                {"event": "skill.invoked", "skillId": "review-helper", "invocationIdDigest": CALL_A},
                {"event": "skill.invoked", "skillId": "review-helper", "invocationIdDigest": CALL_A}
            ]
        }));
        assert_eq!(streamed_repeat.get("review-helper"), Some(&1));

        let without_ids = invocation_counts(&json!({
            "ok": true,
            "events": [
                {"event": "skill.invoked", "skillId": "review-helper"},
                {"event": "skill.invoked", "skillId": "review-helper"}
            ]
        }));
        assert_eq!(without_ids.get("review-helper"), Some(&2));
    }
}
