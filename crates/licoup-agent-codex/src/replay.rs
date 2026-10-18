//! Replay arm for the `codex` adapter.
//!
//! One real [`CodexParser`] — the same parser a live app-server turn drives —
//! consumes the recorded `stdio-jsonrpc` frames, so every projection is the
//! parser's own report and never a re-derivation of the payload. The arm travels
//! with the parser: a program that composes this package's parser set gets the
//! arm that can only fail when *this* parser regresses.
//!
//! The parser is built hermetically: the config is a struct literal with a
//! synthetic prompt, no model, and an empty requested session, which is the
//! fresh-thread client context. The transcript itself records the agent's side
//! of the session, so it cannot carry the client's choice between `thread/start`
//! and `thread/resume`: that request is written before the first response is
//! consumed, and a replay arm has no lookahead with which to apply it
//! retroactively. The resumed identity still arrives in band in the thread
//! handshake, which is what the parser binds and reports.
//!
//! `CodexEffect::Protocol` is a pass-through wrapper around `ProtocolEffect`,
//! so a projection names the wrapped report variant the driver actually
//! consumes alongside the wrapper that reported it.

use licoup_agent_adapter_sdk::replay::{FrameReplay, RecordedFrame};
use serde_json::{Map, Value, json};

use crate::app_server::config::ProtocolConfig;
use crate::app_server::model::{
    EffectiveSettings, ProtocolEffect, ProtocolFailure, ProtocolOutcome,
};
use crate::parser::{CodexEffect, CodexParser};
use crate::registration::{ADAPTER_ID, FRAMING};

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
    Ok(Box::new(Replay::new()?))
}

/// The parser this package's own driver constructs, built for one transcript.
pub struct Replay {
    parser: CodexParser,
}

impl Replay {
    pub fn new() -> Result<Self, String> {
        Ok(Self {
            parser: CodexParser::new(ProtocolConfig {
                prompt: "synthetic-user-prompt".to_owned(),
                private_instructions: None,
                requested_session_id: String::new(),
                session_path: None,
                local_images: Vec::new(),
                cwd: Some("/workspace/project".to_owned()),
                model: None,
                reasoning_effort: None,
                sandbox: None,
                approval_policy: None,
            }),
        })
    }
}

impl FrameReplay for Replay {
    fn feed(&mut self, frame: &RecordedFrame) -> Result<Vec<Value>, String> {
        if frame.channel != FRAMING {
            return Err(format!(
                "the codex app-server transcript records the {:?} channel; this parser speaks \
                 {FRAMING}",
                frame.channel
            ));
        }
        if frame.direction != "agent-to-client" {
            return Err(format!(
                "the codex app-server transcript records the agent's side of the session; a {:?} \
                 frame cannot be applied to a parser that has already consumed a response",
                frame.direction
            ));
        }
        match self.parser.parse_line(frame.payload.as_bytes()) {
            Ok(effects) => Ok(effects.into_iter().map(effect_json).collect()),
            Err(failure) => Ok(vec![failure_json(&failure)]),
        }
    }
}

fn effect_json(effect: CodexEffect) -> Value {
    match effect {
        CodexEffect::Protocol(effect) => match effect {
            ProtocolEffect::Send(message) => json!({
                "effect": "protocol",
                "send": message,
            }),
            ProtocolEffect::Complete(outcome) => json!({
                "effect": "protocol",
                "complete": outcome_json(&outcome),
            }),
            ProtocolEffect::Fail(failure) => json!({
                "effect": "protocol",
                "fail": failure_json(&failure),
            }),
        },
        CodexEffect::SteerResponse {
            request_id,
            accepted,
        } => json!({
            "effect": "steer_response",
            "requestId": request_id,
            "accepted": accepted,
        }),
    }
}

fn outcome_json(outcome: &ProtocolOutcome) -> Value {
    json!({
        "output": outcome.output,
        "sessionId": outcome.session_id,
        "threadId": outcome.thread_id,
        "turnId": outcome.turn_id,
        "turnStatus": outcome.turn_status,
        "effective": effective_json(&outcome.effective),
    })
}

fn effective_json(effective: &EffectiveSettings) -> Value {
    json!({
        "cwd": effective.cwd,
        "model": effective.model,
        "reasoningEffort": effective.reasoning_effort,
        "sandbox": effective.sandbox,
        "approvalPolicy": effective.approval_policy,
    })
}

/// A rejected frame is reported as the parser's own failure: its closed code,
/// its static classification, and the identity the parser had bound.
fn failure_json(failure: &ProtocolFailure) -> Value {
    let mut entry = Map::new();
    entry.insert("error".to_owned(), json!(failure.code));
    entry.insert("message".to_owned(), json!(failure.message));
    entry.insert("stage".to_owned(), json!(failure.stage));
    entry.insert(
        "userInteractionRequired".to_owned(),
        json!(failure.user_interaction_required),
    );
    for (key, value) in [
        ("component", failure.component),
        ("recovery", failure.recovery),
        ("requestMethod", failure.request_method.as_deref()),
        ("sessionId", failure.session_id.as_deref()),
        ("threadId", failure.thread_id.as_deref()),
        ("turnId", failure.turn_id.as_deref()),
        ("turnStatus", failure.turn_status.as_deref()),
    ] {
        if let Some(value) = value {
            entry.insert(key.to_owned(), json!(value));
        }
    }
    if let Some(retryable) = failure.retryable {
        entry.insert("retryable".to_owned(), json!(retryable));
    }
    Value::Object(entry)
}
