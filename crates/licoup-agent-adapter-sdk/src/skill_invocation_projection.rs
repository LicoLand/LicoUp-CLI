//! Privacy-minimal projection of structured runtime skill-call events.
//!
//! Vendor payloads remain inside their adapters. Only an allowlisted skill id
//! is projected, and only when the surrounding structured event explicitly
//! identifies a skill invocation. Prompts, arguments, paths, and tool results
//! are never copied.
//!
//! Two profiles share the same node-level matching semantics:
//! - `Runtime` projects live driver payloads and is intentionally frozen.
//! - `History` adds the envelope dialects found in persisted local transcript
//!   files (Kimi wire `context.append_loop_event` wrappers, Codex
//!   `response_item` payloads with string-encoded `function_call` arguments)
//!   and is used only by the local skill-usage backfill scanner.

use serde_json::{Value, json};
use sha2::{Digest, Sha256};

const MAX_NODES: usize = 64;
const MAX_SKILL_ID_BYTES: usize = 128;
const MAX_RAW_INVOCATION_ID_BYTES: usize = 512;
const MAX_ARGUMENTS_JSON_BYTES: usize = 64 * 1024;

#[derive(Clone, Copy, Eq, PartialEq)]
pub(crate) enum SkillInvocationProfile {
    Runtime,
    History,
}

pub fn project_skill_invocations(value: &Value) -> Vec<Value> {
    project_with_profile(value, SkillInvocationProfile::Runtime)
}

/// Same matching semantics as the live-driver projection with history-file
/// envelope unwrapping layered on top. Driver behavior stays on the frozen
/// runtime profile; this entry point serves only local history backfill.
pub fn project_history_skill_invocations(value: &Value) -> Vec<Value> {
    project_with_profile(value, SkillInvocationProfile::History)
}

fn project_with_profile(value: &Value, profile: SkillInvocationProfile) -> Vec<Value> {
    let mut remaining = MAX_NODES;
    let mut events = Vec::new();
    inspect_node(value, profile, &mut remaining, &mut events);
    events
}

fn inspect_node(
    value: &Value,
    profile: SkillInvocationProfile,
    remaining: &mut usize,
    events: &mut Vec<Value>,
) {
    if *remaining == 0 {
        return;
    }
    *remaining -= 1;
    if let Some(event) = skill_invocation_from_node(value, profile) {
        events.push(event);
    }
    for child in known_children(value, profile) {
        inspect_node(child, profile, remaining, events);
    }
}

fn known_children<'a>(value: &'a Value, profile: SkillInvocationProfile) -> Vec<&'a Value> {
    let mut children = Vec::new();
    if let Some(items) = value.as_array() {
        children.extend(items.iter());
        return children;
    }
    let mut paths = vec![
        "/content",
        "/parts",
        "/message/content",
        "/params/item",
        "/params/update",
        "/properties/part",
    ];
    if profile == SkillInvocationProfile::History {
        // Kimi wire records wrap loop events under `/event`; Codex rollout
        // records wrap response items under `/payload`. `/message/content` is
        // already unwrapped above, so `/message` itself is not added here —
        // visiting both would project the same tool call twice.
        paths.extend(["/event", "/payload"]);
    }
    for path in paths {
        let Some(child) = value.pointer(path) else {
            continue;
        };
        if let Some(items) = child.as_array() {
            children.extend(items.iter());
        } else if child.is_object() {
            children.push(child);
        }
    }
    children
}

fn skill_invocation_from_node(value: &Value, profile: SkillInvocationProfile) -> Option<Value> {
    let marker = first_text(
        value,
        &[
            "/event",
            "/type",
            "/sessionUpdate",
            "/kind",
            "/toolCall/kind",
        ],
    )?
    .to_ascii_lowercase();
    let tool_name = first_text(
        value,
        &[
            "/toolName",
            "/tool",
            "/tool/name",
            "/toolCall/name",
            "/toolCall/title",
            "/name",
            "/title",
        ],
    );
    let explicit_skill_event = matches!(
        marker.as_str(),
        "skill_invoked"
            | "skill.invoked"
            | "skill-invoked"
            | "skillinvocation"
            | "skill_invocation"
            | "skill_invocation_start"
            | "skill.invocation.started"
    );
    let skill_tool = tool_name.is_some_and(is_skill_tool_name);
    let single_call_or_start = matches!(
        marker.as_str(),
        "tool_call"
            | "toolcall"
            | "dynamic_tool_call"
            | "dynamictoolcall"
            | "tool_execution_start"
            | "tool_use"
            | "tool"
    ) || (profile == SkillInvocationProfile::History
        && matches!(
            marker.as_str(),
            "tool.call" | "tool-call" | "function_call" | "functioncall"
        ));
    if !explicit_skill_event && !(single_call_or_start && skill_tool) {
        return None;
    }

    let candidate = first_text(
        value,
        &[
            "/skillId",
            "/skill/id",
            "/input/skillId",
            "/input/skill",
            "/input/name",
            "/arguments/skillId",
            "/arguments/skill",
            "/arguments/name",
            "/args/skillId",
            "/args/skill",
            "/args/name",
            "/rawInput/skillId",
            "/rawInput/skill",
            "/toolCall/input/skillId",
            "/toolCall/input/skill",
            "/toolCall/arguments/skillId",
            "/toolCall/arguments/skill",
            "/state/input/skillId",
            "/state/input/skill",
            "/state/input/name",
            "/metadata/skillId",
        ],
    )
    .map(str::to_owned)
    .or_else(|| {
        tool_name
            .and_then(skill_id_from_tool_name)
            .map(str::to_owned)
    })
    .or_else(|| {
        explicit_skill_event
            .then(|| first_text(value, &["/name", "/title"]))
            .flatten()
            .map(str::to_owned)
    })
    .or_else(|| {
        (profile == SkillInvocationProfile::History)
            .then(|| string_encoded_arguments_skill_id(value))
            .flatten()
    })?;
    let skill_id = safe_skill_id(&candidate)?;
    let mut event = json!({"event": "skill.invoked", "skillId": skill_id});
    if let Some(digest) = invocation_id_digest(value, profile) {
        event["invocationIdDigest"] = json!(digest);
    }
    Some(event)
}

/// Codex history records carry `function_call` arguments as one JSON-encoded
/// string. The skill id is projected from the parsed object without retaining
/// any argument payload.
fn string_encoded_arguments_skill_id(value: &Value) -> Option<String> {
    let raw = value.get("arguments")?.as_str()?.trim();
    if !raw.starts_with('{') || raw.len() > MAX_ARGUMENTS_JSON_BYTES {
        return None;
    }
    let parsed: Value = serde_json::from_str(raw).ok()?;
    first_text(&parsed, &["/skillId", "/skill", "/name"]).map(str::to_owned)
}

fn invocation_id_digest(value: &Value, profile: SkillInvocationProfile) -> Option<String> {
    let mut paths = vec![
        "/toolCallId",
        "/tool_call_id",
        "/toolCall/id",
        "/callId",
        "/callID",
        "/call/id",
        "/invocationId",
        "/invocation/id",
        "/itemId",
        "/id",
    ];
    if profile == SkillInvocationProfile::History {
        // Codex history `function_call` items identify calls with `call_id`.
        paths.push("/call_id");
    }
    let raw_id = first_text(value, &paths)?;
    if raw_id.len() > MAX_RAW_INVOCATION_ID_BYTES {
        return None;
    }
    let mut digest = Sha256::new();
    digest.update(b"licoup.skill-invocation.v1\0");
    digest.update(raw_id.as_bytes());
    Some(format!("sha256:{:x}", digest.finalize()))
}

fn first_text<'a>(value: &'a Value, paths: &[&str]) -> Option<&'a str> {
    paths
        .iter()
        .find_map(|path| value.pointer(path).and_then(Value::as_str))
        .map(str::trim)
        .filter(|text| !text.is_empty())
}

fn is_skill_tool_name(value: &str) -> bool {
    let normalized = value.trim().to_ascii_lowercase();
    matches!(normalized.as_str(), "skill" | "skills" | "skill.invoke")
        || normalized.starts_with("skill:")
        || normalized.starts_with("skill/")
}

fn skill_id_from_tool_name(value: &str) -> Option<&str> {
    let trimmed = value.trim();
    trimmed
        .strip_prefix("skill:")
        .or_else(|| trimmed.strip_prefix("Skill:"))
        .or_else(|| trimmed.strip_prefix("skill/"))
        .or_else(|| trimmed.strip_prefix("Skill/"))
}

fn safe_skill_id(value: &str) -> Option<String> {
    let trimmed = value.trim();
    if trimmed.is_empty()
        || trimmed.len() > MAX_SKILL_ID_BYTES
        || !trimmed
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
    {
        return None;
    }
    Some(trimmed.to_ascii_lowercase())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn projects_only_explicit_structured_skill_calls() {
        let events = project_skill_invocations(&json!({
            "type": "assistant",
            "message": {"content": [
                {"type": "tool_use", "name": "Skill", "input": {"skill": "review-helper"}},
                {"type": "tool_use", "name": "Read", "input": {"skill": "not-a-call"}}
            ]}
        }));
        assert_eq!(
            events,
            vec![json!({
                "event": "skill.invoked",
                "skillId": "review-helper"
            })]
        );

        assert_eq!(
            project_skill_invocations(&json!({
                "parts": [{
                    "type": "tool",
                    "tool": "Skill",
                    "state": {"input": {"name": "release-check"}}
                }]
            })),
            vec![json!({
                "event": "skill.invoked",
                "skillId": "release-check"
            })]
        );

        assert_eq!(
            project_skill_invocations(&json!({
                "type": "tool_execution_start",
                "toolName": "Skill",
                "args": {"skill": "runtime-audit"}
            })),
            vec![json!({
                "event": "skill.invoked",
                "skillId": "runtime-audit"
            })]
        );

        assert_eq!(
            project_skill_invocations(&json!({
                "type": "dynamicToolCall",
                "name": "Skill",
                "arguments": {"name": "repo-audit"}
            })),
            vec![json!({
                "event": "skill.invoked",
                "skillId": "repo-audit"
            })]
        );
    }

    #[test]
    fn rejects_free_text_paths_and_arguments() {
        let payload = json!({
            "sessionUpdate": "tool_call",
            "name": "Skill",
            "arguments": {"skill": "invalid.skill.id", "prompt": "synthetic-sensitive-text"}
        });
        assert!(project_skill_invocations(&payload).is_empty());
        assert!(
            project_skill_invocations(&json!({
                "type": "agent_message",
                "text": "skill:review-helper"
            }))
            .is_empty()
        );
    }

    #[test]
    fn preserves_call_instances_and_hashes_stable_runtime_ids() {
        let distinct = project_skill_invocations(&json!({
            "content": [
                {
                    "type": "tool_use",
                    "id": "raw-call-one",
                    "name": "Skill",
                    "input": {"skill": "review-helper"}
                },
                {
                    "type": "tool_use",
                    "id": "raw-call-two",
                    "name": "Skill",
                    "input": {"skill": "review-helper"}
                }
            ]
        }));
        assert_eq!(distinct.len(), 2);
        assert_eq!(distinct[0]["skillId"], "review-helper");
        assert_eq!(distinct[1]["skillId"], "review-helper");
        assert_ne!(
            distinct[0]["invocationIdDigest"],
            distinct[1]["invocationIdDigest"]
        );
        assert!(
            !serde_json::to_string(&distinct)
                .unwrap()
                .contains("raw-call")
        );

        let repeated_payload = json!({
            "sessionUpdate": "tool_call",
            "toolCallId": "opaque-runtime-call",
            "title": "Skill",
            "rawInput": {"skill": "review-helper"}
        });
        let first = project_skill_invocations(&repeated_payload);
        let repeated = project_skill_invocations(&repeated_payload);
        assert_eq!(
            first[0]["invocationIdDigest"],
            repeated[0]["invocationIdDigest"]
        );
        assert!(
            !serde_json::to_string(&first)
                .unwrap()
                .contains("opaque-runtime-call")
        );

        let without_ids = project_skill_invocations(&json!({
            "content": [
                {"type": "tool_use", "name": "Skill", "input": {"skill": "review-helper"}},
                {"type": "tool_use", "name": "Skill", "input": {"skill": "review-helper"}}
            ]
        }));
        assert_eq!(without_ids.len(), 2);
        assert!(
            without_ids
                .iter()
                .all(|event| event.get("invocationIdDigest").is_none())
        );
    }

    #[test]
    fn history_profile_unwraps_vendor_transcript_envelopes() {
        let kimi_wire = json!({
            "type": "context.append_loop_event",
            "turnId": "turn-1",
            "time": "2026-07-10T00:00:02Z",
            "event": {"type": "tool.call", "name": "Skill", "arguments": {"skill": "review-helper"}}
        });
        assert_eq!(
            project_history_skill_invocations(&kimi_wire),
            vec![json!({"event": "skill.invoked", "skillId": "review-helper"})]
        );
        // The runtime profile must not recognize history envelopes.
        assert!(project_skill_invocations(&kimi_wire).is_empty());

        let codex_rollout = json!({
            "timestamp": "2026-06-03T10:53:55.000Z",
            "type": "response_item",
            "payload": {
                "type": "function_call",
                "name": "Skill",
                "call_id": "call-synthetic",
                "arguments": "{\"skill\":\"repo-audit\",\"prompt\":\"synthetic-sensitive-text\"}"
            }
        });
        let codex_events = project_history_skill_invocations(&codex_rollout);
        assert_eq!(codex_events.len(), 1);
        assert_eq!(codex_events[0]["skillId"], "repo-audit");
        assert!(
            codex_events[0]["invocationIdDigest"]
                .as_str()
                .is_some_and(|digest| digest.starts_with("sha256:"))
        );
        let serialized = serde_json::to_string(&codex_events).unwrap();
        assert!(!serialized.contains("synthetic-sensitive-text"));
        assert!(!serialized.contains("call-synthetic"));
        assert!(project_skill_invocations(&codex_rollout).is_empty());

        let claude_transcript = json!({
            "type": "assistant",
            "timestamp": "2026-07-14T00:00:02Z",
            "message": {"content": [
                {"type": "tool_use", "id": "toolu-synthetic", "name": "Skill", "input": {"skill": "lint-fix"}}
            ]}
        });
        let claude_events = project_history_skill_invocations(&claude_transcript);
        assert_eq!(claude_events.len(), 1);
        assert_eq!(claude_events[0]["skillId"], "lint-fix");
        // Claude transcripts already project under the runtime profile.
        assert_eq!(project_skill_invocations(&claude_transcript), claude_events);
    }

    #[test]
    fn history_profile_keeps_runtime_rejections() {
        assert!(
            project_history_skill_invocations(&json!({
                "type": "context.append_loop_event",
                "event": {"type": "tool.call", "name": "exec", "arguments": {"skill": "not-a-skill-tool"}}
            }))
            .is_empty()
        );
        assert!(
            project_history_skill_invocations(&json!({
                "type": "response_item",
                "payload": {
                    "type": "function_call",
                    "name": "Skill",
                    "arguments": "{\"skill\":\"invalid.skill.id\"}"
                }
            }))
            .is_empty()
        );
        assert!(
            project_history_skill_invocations(&json!({
                "type": "response_item",
                "payload": {"type": "function_call_output", "call_id": "c", "output": "skill:review-helper"}
            }))
            .is_empty()
        );
    }
}
