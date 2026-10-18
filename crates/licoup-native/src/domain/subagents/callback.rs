//! Privacy-bounded caller callback notice for one settled subagent claim.
//!
//! The delegated turn's full output already lives in the group Conversation
//! Event/Part stream authored by the target Membership; this module only
//! builds the bounded derivative notice dispatched to the caller Membership.
//! The notice carries nothing but the allow-listed fields below: task/claim
//! id, terminal (or current, on timeout) state, the surface error code, the
//! bounded root-cause class, its recovery hint, and a sanitized excerpt of
//! the terminal output. Credentials, absolute paths, prompt originals, and
//! native session ids never enter the notice.

use crate::platform::runtime_adapters::root_cause;
use licoup_conversation::{ConversationStore, MembershipStatus, SubagentDispatchClaim};
use serde_json::{Value, json};
use std::sync::OnceLock;

/// Causation marker for the callback turn's Conversation facts. The callback
/// dispatch never requests a dispatch id, so no claim row can match it and a
/// callback can never trigger a further callback.
pub const CALLBACK_CAUSATION_ID: &str = "subagent-callback";

/// The summary is a derivative notice, not the subagent's output. The full
/// output is already in the conversation; this bound applies only to the
/// notice excerpt.
pub const MAX_CALLBACK_SUMMARY_CHARS: usize = 2_000;

const MAX_NOTICE_FIELD_CHARS: usize = 256;

const RECOVERY_ALLOW_LIST: &[&str] = &[
    "review_terminal_result",
    "preserve_draft_and_retry",
    root_cause::RECOVERY_QUOTA,
    root_cause::RECOVERY_AUTH,
    root_cause::RECOVERY_NETWORK,
    root_cause::RECOVERY_ENV_MISMATCH,
];

const ROOT_CAUSE_ALLOW_LIST: &[&str] = &[
    "auth",
    "network_unreachable",
    "env_mismatch",
    "quota",
    "unknown",
];

/// Build the Membership-scoped dispatch params for the caller callback turn.
/// The dispatch key is `conversationId` + the caller membership, exactly like
/// every other dispatch door. Returns `None` when the caller Membership can
/// no longer be addressed (left the conversation or lost its agent binding).
pub fn subagent_callback_plan(
    store: &ConversationStore,
    claim: &SubagentDispatchClaim,
    state: &str,
    terminal_payload: Option<&Value>,
) -> Option<Value> {
    let conversation = store.get(&claim.conversation_id).ok()?;
    let caller = conversation
        .memberships
        .iter()
        .find(|membership| membership.id == claim.caller_membership_id)
        .filter(|membership| membership.status == MembershipStatus::Active)?;
    let agent_id = caller.principal.agent_id.as_deref()?;
    Some(json!({
        "agent": agent_id,
        "agentId": agent_id,
        "text": subagent_callback_prompt(claim, state, terminal_payload),
        "streamEvents": true,
        "timeoutMs": 0,
        "conversationId": claim.conversation_id,
        "membershipId": claim.caller_membership_id,
        "causationId": CALLBACK_CAUSATION_ID,
    }))
}

/// The privacy-sanitized notice text for one subagent signal. Only
/// allow-listed fields are rendered; everything else stays in the private
/// dispatch record and the conversation event stream.
pub fn subagent_callback_prompt(
    claim: &SubagentDispatchClaim,
    state: &str,
    terminal_payload: Option<&Value>,
) -> String {
    let terminal = matches!(state, "completed" | "failed" | "cancelled");
    let mut lines = vec![
        "[LicoUp subagent callback] A task delegated through lico_subagent_delegate reported a signal.".to_owned(),
        format!("Task: {}", claim.id),
        format!("State: {state}"),
    ];
    if !terminal {
        lines.push(
            "Timeout: the configured timeoutMs elapsed before a terminal signal; the state above is the current state."
                .to_owned(),
        );
    }
    if let Some(payload) = terminal_payload {
        let error = payload.get("error").unwrap_or(payload);
        if let Some(code) = error
            .get("code")
            .and_then(Value::as_str)
            .and_then(bounded_field)
        {
            lines.push(format!("Error: {code}"));
        }
        if let Some(cause) = error
            .get("rootCause")
            .and_then(Value::as_str)
            .filter(|value| ROOT_CAUSE_ALLOW_LIST.contains(value))
        {
            lines.push(format!("Root cause: {cause}"));
        }
        if let Some(recovery) = error
            .get("recovery")
            .and_then(Value::as_str)
            .filter(|value| RECOVERY_ALLOW_LIST.contains(value))
        {
            lines.push(format!("Recovery: {recovery}"));
        }
    }
    match terminal_payload
        .and_then(|payload| payload.get("output"))
        .and_then(Value::as_str)
        .and_then(sanitize_callback_summary)
    {
        Some(summary) => lines.push(format!("Summary: {summary}")),
        None if terminal => lines.push(
            "Summary: no terminal output was recorded; inspect the conversation event stream."
                .to_owned(),
        ),
        None => {}
    }
    lines.push("The complete delegated output is in this conversation's event stream.".to_owned());
    lines.join("\n")
}

/// One allow-listed machine field is bounded and control-free; anything else
/// is dropped from the notice rather than escaped.
fn bounded_field(value: &str) -> Option<&str> {
    let trimmed = value.trim();
    (!trimmed.is_empty()
        && trimmed.chars().count() <= MAX_NOTICE_FIELD_CHARS
        && !trimmed.chars().any(char::is_control))
    .then_some(trimmed)
}

/// Sanitize and bound the terminal-output excerpt. Modeled on the existing
/// structured-event sanitizer
/// (`domain/conversation/history/message_projection/structured_privacy.rs`):
/// bearer tokens, secret assignments, local paths, and long opaque values are
/// redacted; an empty or fully-redacted candidate yields no summary.
fn sanitize_callback_summary(output: &str) -> Option<String> {
    let trimmed = output.trim();
    if trimmed.is_empty() {
        return None;
    }
    let redacted = bearer_regex().replace_all(trimmed, "Bearer [redacted]");
    let redacted = secret_assignment_regex().replace_all(&redacted, "$1: [redacted]");
    let redacted = relative_path_regex().replace_all(&redacted, "$1[local path hidden]");
    let redacted = local_path_regex().replace_all(&redacted, "[local path hidden]");
    let redacted = opaque_value_regex().replace_all(&redacted, "[opaque value hidden]");
    let redacted = redacted.trim();
    if redacted.is_empty() {
        return None;
    }
    Some(bound_chars(redacted, MAX_CALLBACK_SUMMARY_CHARS))
}

fn bound_chars(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        return text.to_owned();
    }
    let truncated: String = text.chars().take(max).collect();
    format!("{truncated}…")
}

fn bearer_regex() -> &'static regex::Regex {
    static REGEX: OnceLock<regex::Regex> = OnceLock::new();
    REGEX.get_or_init(|| {
        regex::Regex::new(r"(?i)\bbearer\s+[a-z0-9._~+\-/]+=*").expect("valid bearer regex")
    })
}

fn secret_assignment_regex() -> &'static regex::Regex {
    static REGEX: OnceLock<regex::Regex> = OnceLock::new();
    REGEX.get_or_init(|| {
        regex::Regex::new(
            r#"(?i)\b(api[_-]?key|access[_-]?token|refresh[_-]?token|authorization|password|secret|cookie|credential)\b\s*[:=]\s*(?:\"[^\"]*\"|'[^']*'|[^\s,;]+)"#,
        )
        .expect("valid secret assignment regex")
    })
}

fn local_path_regex() -> &'static regex::Regex {
    static REGEX: OnceLock<regex::Regex> = OnceLock::new();
    REGEX.get_or_init(|| {
        regex::Regex::new(
            r#"(?i)(?:file://)?/(?:[a-z0-9._-]+/)*[a-z0-9._-]+[^\s\"'<>]*|[a-z]:\\[^\s\"'<>]*|~[/\\][^\s\"'<>]*"#,
        )
        .expect("valid local path regex")
    })
}

fn relative_path_regex() -> &'static regex::Regex {
    static REGEX: OnceLock<regex::Regex> = OnceLock::new();
    REGEX.get_or_init(|| {
        regex::Regex::new(
            r#"(?i)(^|[\s(\"'=])((?:\.{1,2}[/\\])?[a-z0-9._-]+(?:[/\\][a-z0-9._-]+)+)"#,
        )
        .expect("valid relative local path regex")
    })
}

fn opaque_value_regex() -> &'static regex::Regex {
    static REGEX: OnceLock<regex::Regex> = OnceLock::new();
    REGEX.get_or_init(|| {
        regex::Regex::new(r"\b[a-zA-Z0-9_-]{40,}\b").expect("valid opaque value regex")
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use licoup_conversation::{MembershipAccess, Principal, PrincipalKind};

    fn fixture_claim() -> (ConversationStore, SubagentDispatchClaim, String) {
        let store = ConversationStore::open_in_memory().unwrap();
        let owner = Principal {
            id: "human:owner".into(),
            kind: PrincipalKind::Human,
            display_name: "Owner".into(),
            agent_id: None,
            created_at_unix_ms: 1,
        };
        let members = ["caller-agent", "target-agent"].map(|agent_id| {
            (
                Principal {
                    id: format!("agent:{agent_id}"),
                    kind: PrincipalKind::Agent,
                    display_name: agent_id.into(),
                    agent_id: Some(agent_id.into()),
                    created_at_unix_ms: 1,
                },
                MembershipAccess::Member,
            )
        });
        let conversation = store
            .create_conversation_with_members("Callback", owner, &members)
            .unwrap();
        let membership = |agent_id: &str| {
            conversation
                .memberships
                .iter()
                .find(|membership| membership.principal.agent_id.as_deref() == Some(agent_id))
                .unwrap()
                .id
                .clone()
        };
        let caller = membership("caller-agent");
        let target = membership("target-agent");
        let claim = store
            .claim_subagent_dispatch(&conversation.id, &caller, &target, None)
            .unwrap();
        (store, claim, caller)
    }

    #[test]
    fn terminal_prompt_carries_only_allowlisted_fields_and_sanitized_summary() {
        let (_store, claim, _caller) = fixture_claim();
        let prompt = subagent_callback_prompt(
            &claim,
            "failed",
            Some(&json!({
                "ok": false,
                "output": "audit written to /fixture/agent/repo/report.md\nauthorization: Bearer sk-test",
                "error": {
                    "code": "antigravity_hook_receipt_missing",
                    "stage": "session/new",
                    "rootCause": "env_mismatch",
                    "recovery": root_cause::RECOVERY_ENV_MISMATCH,
                    "message": "raw driver message never enters the notice",
                    "sessionId": "native-session-must-not-leak"
                }
            })),
        );

        assert!(prompt.contains(&format!("Task: {}", claim.id)));
        assert!(prompt.contains("State: failed"));
        assert!(prompt.contains("Error: antigravity_hook_receipt_missing"));
        assert!(prompt.contains("Root cause: env_mismatch"));
        assert!(prompt.contains(&format!("Recovery: {}", root_cause::RECOVERY_ENV_MISMATCH)));
        assert!(prompt.contains("Summary:"));
        assert!(!prompt.contains("/fixture/agent"));
        assert!(!prompt.contains("repo/report.md"));
        assert!(prompt.contains("[local path hidden]"));
        assert!(!prompt.contains("sk-test"));
        assert!(!prompt.contains("native-session-must-not-leak"));
        assert!(!prompt.contains("raw driver message"));
    }

    #[test]
    fn timeout_prompt_reports_current_state_without_output_summary() {
        let (_store, claim, _caller) = fixture_claim();
        let prompt = subagent_callback_prompt(&claim, "running", None);

        assert!(prompt.contains("State: running"));
        assert!(prompt.contains("Timeout:"));
        assert!(!prompt.contains("Summary:"));
        assert!(!prompt.contains("Error:"));
    }

    #[test]
    fn summary_is_bounded_and_unlisted_diagnostics_are_dropped() {
        let (_store, claim, _caller) = fixture_claim();
        let long = "line of delegated output ".repeat(500);
        let prompt = subagent_callback_prompt(
            &claim,
            "completed",
            Some(&json!({
                "ok": true,
                "output": long,
                "error": {
                    "code": "unlisted-code",
                    "rootCause": "unlisted-cause",
                    "recovery": "unlisted-recovery"
                }
            })),
        );

        assert!(prompt.contains("State: completed"));
        assert!(prompt.contains("Error: unlisted-code"));
        assert!(!prompt.contains("unlisted-cause"));
        assert!(!prompt.contains("unlisted-recovery"));
        let summary = prompt
            .lines()
            .find(|line| line.starts_with("Summary: "))
            .unwrap();
        assert!(summary.ends_with('…'));
        assert!(summary.chars().count() <= MAX_CALLBACK_SUMMARY_CHARS + 10);
    }

    #[test]
    fn plan_targets_the_caller_membership_and_never_carries_a_claim_identity() {
        let (store, claim, caller) = fixture_claim();
        let params = subagent_callback_plan(
            &store,
            &claim,
            "completed",
            Some(&json!({"ok": true, "output": "done"})),
        )
        .unwrap();

        assert_eq!(params["agent"], "caller-agent");
        assert_eq!(params["conversationId"], claim.conversation_id);
        assert_eq!(params["membershipId"], caller);
        assert_eq!(params["timeoutMs"], 0);
        assert_eq!(params["causationId"], CALLBACK_CAUSATION_ID);
        assert!(params.get("dispatchId").is_none());
        assert!(params.get("parentDispatchId").is_none());
        assert!(params.get("sessionId").is_none());
        assert!(params["text"].as_str().unwrap().contains(&claim.id));
    }

    #[test]
    fn plan_is_unavailable_once_the_caller_membership_left() {
        let (store, claim, caller) = fixture_claim();
        store.leave_member(&claim.conversation_id, &caller).unwrap();

        assert!(subagent_callback_plan(&store, &claim, "completed", None).is_none());
    }
}
