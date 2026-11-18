//! Replay arm for the `pi` adapter.
//!
//! Pi frames are JSONL lines on the stdio RPC channel. One real [`PiProtocol`] —
//! the same parser the production driver builds — consumes the recorded lines
//! through the transport's own framing helper ([`decode_jsonl_line`]), so every
//! projection is the parser's own report and never a re-derivation of the
//! payload. The arm travels with the parser: a program that composes this
//! package's parser set gets the arm that can only fail when *this* parser
//! regresses.
//!
//! The protocol is built hermetically: the invocation is a synthetic struct
//! literal reconstructed from Pi's own `switch_session` response, which is the
//! one recorded frame that identifies a resumed conversation. None of these
//! values is a machine fact.

use licoup_agent_adapter_sdk::replay::{FrameReplay, RecordedFrame};
use serde_json::{Value, json};
use std::path::PathBuf;

use crate::driver::model::EffectiveSettings;
use crate::driver::params::ProtocolConfig;
use crate::parser::{PiProtocol, ProtocolEffect, decode_jsonl_line};
use crate::registration::{ADAPTER_ID, FRAMING};

/// Synthetic invocation identity. The driver decides resume-versus-new before
/// the first inbound frame, so the arm reconstructs that decision from the
/// protocol (see [`configuration`]). None of these values is a machine fact.
const SYNTHETIC_PROMPT: &str = "synthetic-pi-prompt";
const SYNTHETIC_CWD: &str = "synthetic-workspace";
const SYNTHETIC_TURN_ID: &str = "synthetic-turn";
const RESUMED_SESSION_ID: &str = "pi-native-resume-1";
const RESUMED_SESSION_PATH: &str = "synthetic-pi-native-resume.jsonl";

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
    protocol: Option<PiProtocol>,
}

impl Replay {
    pub fn new() -> Self {
        Self { protocol: None }
    }
}

impl Default for Replay {
    fn default() -> Self {
        Self::new()
    }
}

impl FrameReplay for Replay {
    fn feed(&mut self, frame: &RecordedFrame) -> Result<Vec<Value>, String> {
        // The corpus records the channel this parser speaks and the agent's side
        // of the session; a frame from another channel or the client's side is
        // refused rather than applied to a parser that already consumed a
        // response.
        if frame.channel != FRAMING {
            return Err(format!(
                "the pi transcript records the {:?} channel; this parser speaks {FRAMING}",
                frame.channel
            ));
        }
        if frame.direction != "agent-to-client" {
            return Err(format!(
                "the pi transcript records the agent's side of the session; a {:?} frame cannot \
                 be applied to a parser that has already consumed a response",
                frame.direction
            ));
        }
        let message = match decode_jsonl_line(&frame.payload) {
            Ok(Some(message)) => message,
            Ok(None) => return Ok(Vec::new()),
            // The transport's own framing rejection. `decode_jsonl_line`
            // answers a unit error, so `invalid_json` is the whole fact: this
            // line did not decode. The driver's protocol-failure code for the
            // same event is deliberately not projected, so the fixture fails
            // when the framing helper changes rather than when a code is
            // renamed.
            Err(()) => return Ok(vec![json!({"error": "invalid_json"})]),
        };
        let protocol = self
            .protocol
            .get_or_insert_with(|| PiProtocol::new(configuration(&message)));
        Ok(protocol
            .handle_message(message)
            .iter()
            .map(effect_fact)
            .collect())
    }
}

/// Rebuild the invocation the driver would have started this transcript with.
/// Only Pi's own `switch_session` response belongs to a resumed conversation
/// (`PiProtocol::initial_request` issues the switch only when a resume path was
/// supplied), so that frame is the one document that identifies the resume
/// invocation; every other transcript starts a new session.
fn configuration(message: &Value) -> ProtocolConfig {
    let resuming = message.get("type").and_then(Value::as_str) == Some("response")
        && message.get("command").and_then(Value::as_str) == Some("switch_session");
    ProtocolConfig {
        prompt: SYNTHETIC_PROMPT.to_string(),
        requested_session_id: if resuming {
            RESUMED_SESSION_ID.to_string()
        } else {
            String::new()
        },
        resume_session_path: resuming.then(|| PathBuf::from(RESUMED_SESSION_PATH)),
        cwd: SYNTHETIC_CWD.to_string(),
        model: None,
        model_provider: None,
        model_id: None,
        thinking_level: None,
        turn_id: SYNTHETIC_TURN_ID.to_string(),
    }
}

fn effect_fact(effect: &ProtocolEffect) -> Value {
    match effect {
        // The request the parser emitted, carried verbatim under its own names.
        ProtocolEffect::Send(request) => {
            let mut fact = request.clone();
            fact["effect"] = json!("send");
            fact
        }
        // A parked dialog's callback token is process-unique and is therefore
        // never projected; the parser's own request identity stays on the
        // `extension_ui_request` frame the corpus records.
        ProtocolEffect::Interact(_) => json!({"effect": "interact"}),
        ProtocolEffect::Complete(outcome) => json!({
            "effect": "complete",
            "output": outcome.output,
            "sessionId": outcome.session_id,
            "turnId": outcome.turn_id,
            "turnStatus": outcome.turn_status,
            "effective": effective_settings(&outcome.effective),
        }),
        ProtocolEffect::Fail(failure) => json!({
            "effect": "fail",
            "code": failure.code,
            "message": failure.message,
            "stage": failure.stage,
            "userInteractionRequired": failure.user_interaction_required,
            "requestMethod": failure.request_method,
            "sessionId": failure.session_id,
            "turnId": failure.turn_id,
            "turnStatus": failure.turn_status,
        }),
    }
}

fn effective_settings(effective: &EffectiveSettings) -> Value {
    json!({
        "cwd": effective.cwd,
        "model": effective.model,
        "reasoningEffort": effective.reasoning_effort,
        "permissionMode": effective.permission_mode,
        "sandbox": effective.sandbox,
        "approvalPolicy": effective.approval_policy,
    })
}
