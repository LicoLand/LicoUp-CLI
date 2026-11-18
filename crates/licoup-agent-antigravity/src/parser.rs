//! Antigravity's vendor protocol below the adapter port.
//!
//! Antigravity exposes no line-oriented protocol. A turn's vendor facts arrive
//! through three entries, and this module is the single place that reads them:
//!
//! 1. the official **Agent Hooks receipt** — the vendor CLI, its own Stop-hook
//!    writer, or this package's [`crate::hook`] writes a JSON object carrying
//!    the native `conversationId`; [`parse_hook_receipt`] recovers it, per
//!    ADR-0008 classification happening here and nowhere above;
//! 2. the **PTY lane** — [`PtyOutputParser`] strips terminal control from the
//!    supervised process's stdout and accumulates the turn's output;
//! 3. the **terminal process outcome** — [`classify_terminal`] combines the
//!    launch-time requested conversation, the receipt and the process result
//!    into this Agent's success or its own stable failure.
//!
//! The declaration below is the same string the replay corpus records as its
//! channel, so a fixture cannot pass against another Agent's framing.

use licoup_agent_adapter_sdk::adapters::AdapterContract;
use licoup_agent_adapter_sdk::{LifecycleStage, Transition, TransitionReducer};
use licoup_foundation::platform::ansi_stripper::AnsiStripper;
use serde_json::Value;

/// The one adapter declaration this package's parser reports.
pub const CONTRACT: AdapterContract = AdapterContract::new("antigravity", "pty-hook-json");

const MIN_SESSION_ID_LEN: usize = 8;
const MAX_SESSION_ID_LEN: usize = 128;

/// Whether a value is one Antigravity native conversation identity.
///
/// The vendor's conversation ids are opaque but bounded: a durable identity is
/// accepted only when it is the documented length range of alphanumerics,
/// `_` and `-`. Everything else is refused rather than quoted upstream.
pub fn valid_session_id(session_id: &str) -> bool {
    let len = session_id.len();
    (MIN_SESSION_ID_LEN..=MAX_SESSION_ID_LEN).contains(&len)
        && session_id
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '_' | '-'))
}

/// Recovers the native conversation identifier from a session receipt.
///
/// The primary input is the direct vendor receipt: a single JSON object with a
/// top-level `conversationId` (or an accepted alias) that the Antigravity CLI,
/// its official Stop-hook writer, or the LicoUp hook bridge writes. The wrapped
/// vendor payload and the vendor environment identifier are retained only as
/// compatible inputs for receipts produced by earlier writer layouts, so any
/// compatible writer order resolves to the same conversation.
pub fn parse_hook_receipt(text: &str) -> Option<String> {
    let envelope: Value = serde_json::from_str(text).ok()?;
    let payload = envelope
        .get("hookPayload")
        .and_then(Value::as_str)
        .and_then(|raw| serde_json::from_str::<Value>(raw).ok());
    [
        conversation_identifier(&envelope),
        payload.as_ref().and_then(conversation_identifier),
        envelope
            .get("environmentConversationId")
            .and_then(Value::as_str),
    ]
    .into_iter()
    .flatten()
    .map(str::trim)
    .filter(|value| valid_session_id(value))
    .map(str::to_owned)
    .next()
}

/// The vendor conversation identifier key set accepted at the top level of a
/// direct receipt and inside the retained wrapped payload.
fn conversation_identifier(value: &Value) -> Option<&str> {
    [
        "conversationId",
        "conversation_id",
        "sessionId",
        "session_id",
    ]
    .into_iter()
    .find_map(|key| value.get(key).and_then(Value::as_str))
}

/// The PTY lane's stdout parser: strips terminal control once, accumulates the
/// turn's output, and reports each visible chunk as it arrives.
pub struct PtyOutputParser {
    stripper: AnsiStripper,
    output: String,
}

impl PtyOutputParser {
    /// A parser for one turn's stdout, holding no state between turns.
    pub fn new() -> Self {
        Self {
            stripper: AnsiStripper::new(),
            output: String::new(),
        }
    }

    /// Feed one stdout chunk; `None` means the chunk carried no visible text.
    pub fn push(&mut self, bytes: &[u8]) -> Option<String> {
        let text = self.stripper.push(bytes);
        if text.is_empty() {
            None
        } else {
            self.output.push_str(&text);
            Some(text)
        }
    }

    /// Finish the lane: the trimmed turn output and any trailing partial
    /// escape sequence that is flushed as one final visible chunk.
    pub fn finish(mut self) -> (String, Option<String>) {
        let tail = self.stripper.finish();
        let effect = (!tail.is_empty()).then(|| tail.clone());
        self.output.push_str(&tail);
        (self.output.trim().to_owned(), effect)
    }
}

#[derive(Clone, Copy, Debug)]
/// The facts the terminal classification reads: the launch-time requested
/// conversation, the receipt's conversation, the collected output, and the
/// supervised process's own outcome. They never arrive on a vendor wire.
pub struct TerminalFacts<'a> {
    pub requested_session: &'a str,
    pub receipt_session: Option<&'a str>,
    pub output: &'a str,
    pub timed_out: bool,
    pub exit_success: bool,
}

#[derive(Debug, Eq, PartialEq)]
/// This Agent's own refusal of one terminal outcome.
pub struct TerminalFailure {
    pub code: &'static str,
    pub message: &'static str,
    pub stage: &'static str,
    pub session_id: String,
}

#[derive(Debug)]
/// This Agent's report of one completed turn.
pub struct TerminalSuccess {
    pub session_id: String,
    pub output: String,
    pub transitions: Vec<Transition>,
}

/// Classify one finished turn from the facts the driver collected.
///
/// Order is the protocol's, not a caller's: a timed-out process is reported as
/// the timeout, a turn that bound no valid identity is reported as the missing
/// receipt, a resumed conversation that drifted is reported as drift, and only
/// then is a non-zero exit or empty output the turn's failure.
pub fn classify_terminal(facts: TerminalFacts<'_>) -> Result<TerminalSuccess, TerminalFailure> {
    if facts.timed_out {
        return Err(failure(
            "antigravity_cli_timeout",
            "Antigravity CLI timed out before completing the turn.",
            "turn/execute",
            facts.requested_session,
        ));
    }
    let receipt = facts.receipt_session.unwrap_or_default();
    let native_session = if facts.requested_session.is_empty() {
        receipt
    } else {
        facts.requested_session
    };
    if !valid_session_id(native_session) {
        return Err(failure(
            "antigravity_hook_receipt_missing",
            "Antigravity hook bridge did not return a native conversation identifier.",
            "session/new",
            facts.requested_session,
        ));
    }
    if !facts.requested_session.is_empty()
        && !receipt.is_empty()
        && receipt != facts.requested_session
    {
        return Err(failure(
            "antigravity_cli_session_drift",
            "Antigravity CLI resumed a different native conversation than requested.",
            "session/resume",
            facts.requested_session,
        ));
    }
    if !facts.exit_success {
        return Err(failure(
            "antigravity_cli_turn_failed",
            "Antigravity CLI exited without a successful turn.",
            "turn/execute",
            native_session,
        ));
    }
    if facts.output.is_empty() {
        return Err(failure(
            "antigravity_cli_empty_output",
            "Antigravity CLI returned an empty final response.",
            "turn/execute",
            native_session,
        ));
    }
    Ok(TerminalSuccess {
        session_id: native_session.to_owned(),
        output: facts.output.to_owned(),
        transitions: success_transitions(facts.output),
    })
}

/// This Agent's normalized transitions for one completed turn.
///
/// The reply transitions a completed execution reports: the processing and
/// responding stages, the turn's output as this Agent's one reply unit, and the
/// terminal completed stage. It is the same projection
/// [`classify_terminal`] reports, reached from an execution outcome rather than
/// from a classified turn.
pub fn completed_transitions(output: &str) -> Vec<Transition> {
    success_transitions(output)
}

fn success_transitions(output: &str) -> Vec<Transition> {
    let mut reducer = TransitionReducer::default();
    let mut transitions = reducer.advance(LifecycleStage::Processing);
    transitions.extend(reducer.advance(LifecycleStage::Responding));
    transitions.push(Transition::Text {
        unit_id: "antigravity:reply".to_owned(),
        text: output.to_owned(),
    });
    transitions.extend(reducer.advance(LifecycleStage::Completed));
    transitions
}

/// This Agent's normalized transitions for one reported failure.
pub fn failure_transitions(code: &str, stage: &str, message: &str) -> Vec<Transition> {
    let mut reducer = TransitionReducer::default();
    let mut transitions = reducer.advance(LifecycleStage::Accepted);
    if let Some(failure) = reducer.fail(code, stage, message) {
        transitions.push(failure);
    }
    transitions
}

fn failure(
    code: &'static str,
    message: &'static str,
    stage: &'static str,
    session_id: &str,
) -> TerminalFailure {
    TerminalFailure {
        code,
        message,
        stage,
        session_id: session_id.to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hook_and_terminal_parser_reject_session_drift() {
        let receipt = parse_hook_receipt(
            r#"{"hookPayload":"{\"conversationId\":\"11111111-2222-3333-4444-555555555555\"}","environmentConversationId":""}"#,
        )
        .unwrap();
        let result = classify_terminal(TerminalFacts {
            requested_session: "aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee",
            receipt_session: Some(&receipt),
            output: "ok",
            timed_out: false,
            exit_success: true,
        });
        assert_eq!(result.unwrap_err().code, "antigravity_cli_session_drift");

        let success = classify_terminal(TerminalFacts {
            requested_session: "",
            receipt_session: Some(&receipt),
            output: "ok",
            timed_out: false,
            exit_success: true,
        })
        .unwrap();
        assert_eq!(
            success.transitions.last(),
            Some(&Transition::Lifecycle(LifecycleStage::Completed))
        );
    }

    #[test]
    fn hook_receipt_parser_accepts_direct_vendor_receipt_with_aliases() {
        let direct = r#"{"conversationId":"11111111-2222-3333-4444-555555555555"}"#;
        assert_eq!(
            parse_hook_receipt(direct).as_deref(),
            Some("11111111-2222-3333-4444-555555555555")
        );
        for alias in ["conversation_id", "sessionId", "session_id"] {
            let receipt = format!(r#"{{"{alias}":"11111111-2222-3333-4444-555555555555"}}"#);
            assert_eq!(
                parse_hook_receipt(&receipt).as_deref(),
                Some("11111111-2222-3333-4444-555555555555"),
                "alias {alias} must be accepted at the top level"
            );
        }
    }

    #[test]
    fn hook_receipt_parser_retains_wrapped_and_environment_compatibility() {
        let wrapped = r#"{"hookPayload":"{\"conversationId\":\"11111111-2222-3333-4444-555555555555\"}","environmentConversationId":""}"#;
        assert_eq!(
            parse_hook_receipt(wrapped).as_deref(),
            Some("11111111-2222-3333-4444-555555555555")
        );
        let env_only = r#"{"hookPayload":"","environmentConversationId":"11111111-2222-3333-4444-555555555555"}"#;
        assert_eq!(
            parse_hook_receipt(env_only).as_deref(),
            Some("11111111-2222-3333-4444-555555555555")
        );
        // A direct vendor receipt is primary even when an environment fallback
        // disagrees, so writer order never changes the recovered conversation.
        let direct = r#"{"conversationId":"11111111-2222-3333-4444-555555555555","environmentConversationId":"aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee"}"#;
        assert_eq!(
            parse_hook_receipt(direct).as_deref(),
            Some("11111111-2222-3333-4444-555555555555")
        );
    }

    #[test]
    fn hook_receipt_parser_rejects_malformed_identifiers() {
        for receipt in [
            r#"{"conversationId":""}"#,
            r#"{"conversationId":"short"}"#,
            r#"{"conversationId":"11111111-2222-3333-4444-5555555555!5"}"#,
            r#"{"conversationId":42}"#,
            r#"{"hookPayload":"not-json"}"#,
            "not json at all",
        ] {
            assert_eq!(parse_hook_receipt(receipt), None, "receipt: {receipt:?}");
        }
    }

    #[test]
    fn resume_binds_exact_identity_and_rejects_drift() {
        let requested = "aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee";
        let received =
            parse_hook_receipt(&format!(r#"{{"conversationId":"{requested}"}}"#)).unwrap();
        let success = classify_terminal(TerminalFacts {
            requested_session: requested,
            receipt_session: Some(&received),
            output: "ok",
            timed_out: false,
            exit_success: true,
        })
        .unwrap();
        assert_eq!(success.session_id, requested);
        let drifted_id = "11111111-2222-3333-4444-555555555555";
        let drifted_receipt =
            parse_hook_receipt(&format!(r#"{{"conversationId":"{drifted_id}"}}"#)).unwrap();
        let drifted = classify_terminal(TerminalFacts {
            requested_session: requested,
            receipt_session: Some(&drifted_receipt),
            output: "ok",
            timed_out: false,
            exit_success: true,
        })
        .unwrap_err();
        assert_eq!(drifted.code, "antigravity_cli_session_drift");
    }

    #[test]
    fn pty_parser_strips_terminal_control_and_emits_text_effects() {
        let mut parser = PtyOutputParser::new();
        assert_eq!(parser.push(b"\x1b[31mhello"), Some("hello".to_owned()));
        assert_eq!(parser.push(b"\x1b[0m\n"), Some("\n".to_owned()));
        let (output, tail) = parser.finish();
        assert_eq!(output, "hello");
        assert_eq!(tail, None);
    }
}
