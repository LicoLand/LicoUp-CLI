//! Replay arm for the `cursor` adapter.
//!
//! One real [`CursorParser`] — the same parser a live turn drives — consumes the
//! recorded strict-NDJSON frames, so every projection is the parser's own report
//! and never a re-derivation of the payload. The arm travels with the parser: a
//! program that composes this package's parser set gets the arm that can only
//! fail when *this* parser regresses.
//!
//! A replay boundary has no invocation, so the parser's launch context comes
//! from the transcript's own handshake: Cursor always runs a turn against an
//! already-bound native chat (`--resume <id>`), and the identity its init frame
//! carries is that binding. It is fixed when the parser is built, so a later
//! frame naming another conversation is still rejected by the parser's own
//! identity check instead of being silently relabeled.

use licoup_agent_adapter_sdk::replay::{FrameReplay, RecordedFrame};
use serde_json::{Value, json};

use crate::errors::CursorFailureKind;
use crate::model::EffectiveSettings;
use crate::parser::{CursorEffect, CursorOutcome, CursorParseFailure, CursorParser};
use crate::registration::{ADAPTER_ID, FRAMING};

/// The turn being replayed delivers exactly this synthetic prompt. The parser's
/// acknowledgement check stays real because the expectation is fixed here
/// rather than read back out of the transcript it is checking.
const REPLAY_PROMPT: &str = "synthetic-user-prompt";

/// Build the replay arm of this package's parser.
///
/// An adapter this package does not carry is refused rather than defaulted, so a
/// fixture can never pass against a parser that was never constructed.
pub fn replay_arm(adapter_id: &str) -> Result<Box<dyn FrameReplay>, String> {
    if adapter_id != ADAPTER_ID {
        return Err(format!(
            "no replayable parser is registered for adapter {adapter_id}"
        ));
    }
    Ok(Box::new(Replay::new()))
}

/// The parser this package's own driver constructs, built for one transcript.
pub struct Replay {
    parser: Option<CursorParser>,
}

impl Replay {
    pub fn new() -> Self {
        Self { parser: None }
    }
}

impl Default for Replay {
    fn default() -> Self {
        Self::new()
    }
}

impl FrameReplay for Replay {
    fn feed(&mut self, frame: &RecordedFrame) -> Result<Vec<Value>, String> {
        // The strict-NDJSON channel is Cursor's agent-to-client turn stream.
        // Nothing the driver writes crosses it, so an outbound frame here
        // cannot be consumed rather than silently mis-parsed.
        if frame.channel != FRAMING {
            return Err(format!(
                "the cursor turn transcript records the {:?} channel; this parser speaks {FRAMING}",
                frame.channel
            ));
        }
        if frame.direction != "agent-to-client" {
            return Err(format!(
                "cursor turn frames arrive on its agent-to-client NDJSON stream; a {:?} frame is \
                 not consumed by this boundary",
                frame.direction
            ));
        }
        let parser = match self.parser.as_mut() {
            Some(parser) => parser,
            None => {
                // A Cursor turn stream opens with the init frame that names the
                // chat it was launched against. A transcript that opens with
                // anything else cannot be bound, and binding it to a guess
                // would let a drifted frame be attributed to a later index.
                let session = launch_identity(&frame.payload).ok_or_else(|| {
                    format!(
                        "frame {} does not open the turn with the init frame naming its native \
                         chat, so the parser cannot be built as the driver builds it",
                        frame.index
                    )
                })?;
                self.parser = Some(CursorParser::new(
                    &session,
                    REPLAY_PROMPT,
                    EffectiveSettings::default(),
                ));
                self.parser
                    .as_mut()
                    .expect("the parser was just constructed")
            }
        };
        match parser.parse_line(frame.payload.as_bytes()) {
            Ok(effects) => Ok(effects.into_iter().map(effect_json).collect()),
            Err(failure) => Ok(vec![failure_json(failure)]),
        }
    }
}

/// Launch context only: the native chat the init frame binds this turn to. This
/// never contributes to a projection.
fn launch_identity(payload: &str) -> Option<String> {
    let message: Value = serde_json::from_str(payload).ok()?;
    message
        .get("session_id")
        .or_else(|| message.get("sessionId"))
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .map(str::to_owned)
}

fn effect_json(effect: CursorEffect) -> Value {
    match effect {
        CursorEffect::Accepted {
            session_id,
            turn_id,
        } => json!({
            "effect": "accepted",
            "sessionId": session_id,
            "turnId": turn_id,
        }),
        CursorEffect::Text {
            session_id,
            turn_id,
            text,
        } => json!({
            "effect": "text",
            "sessionId": session_id,
            "turnId": turn_id,
            "text": text,
        }),
        CursorEffect::Tool {
            session_id,
            turn_id,
            tool_name,
        } => json!({
            "effect": "tool",
            "sessionId": session_id,
            "turnId": turn_id,
            "toolName": tool_name,
        }),
        CursorEffect::ToolError {
            session_id,
            turn_id,
            tool_name,
            error_code,
        } => json!({
            "effect": "tool_error",
            "sessionId": session_id,
            "turnId": turn_id,
            "toolName": tool_name,
            "errorCode": error_code,
        }),
        CursorEffect::Complete(outcome) => complete_json(&outcome),
    }
}

fn complete_json(outcome: &CursorOutcome) -> Value {
    json!({
        "effect": "complete",
        "output": outcome.output,
        "sessionId": outcome.session_id,
        "turnId": outcome.turn_id,
        "effective": effective_json(&outcome.effective),
    })
}

fn effective_json(effective: &EffectiveSettings) -> Value {
    json!({
        "cwd": effective.cwd,
        "model": effective.model,
        "reasoningEffort": effective.reasoning_effort,
        "permissionMode": effective.permission_mode,
        "sandbox": effective.sandbox,
        "approvalPolicy": effective.approval_policy,
    })
}

fn failure_json(failure: CursorParseFailure) -> Value {
    match failure {
        CursorParseFailure::InvalidJson => json!({"error": "invalid_json"}),
        CursorParseFailure::IdentityMismatch => json!({"error": "identity_mismatch"}),
        CursorParseFailure::PromptAcknowledgementMissing => {
            json!({"error": "prompt_acknowledgement_missing"})
        }
        CursorParseFailure::PromptAcknowledgementMismatch => {
            json!({"error": "prompt_acknowledgement_mismatch"})
        }
        CursorParseFailure::TurnFailed(kind) => json!({
            "error": "turn_failed",
            "kind": failure_kind(kind),
        }),
    }
}

fn failure_kind(kind: CursorFailureKind) -> &'static str {
    match kind {
        CursorFailureKind::AuthenticationRequired => "authentication_required",
        CursorFailureKind::UsageLimitExceeded => "usage_limit_exceeded",
        CursorFailureKind::RateLimited => "rate_limited",
        CursorFailureKind::ModelUnavailable => "model_unavailable",
        CursorFailureKind::ExecutionFailed => "execution_failed",
        CursorFailureKind::TurnFailed => "turn_failed",
    }
}
