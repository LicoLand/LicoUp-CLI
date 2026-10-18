//! Replay arm for the `deepseek_harness` adapter.
//!
//! The production path is two stages: the byte-line [`FrameParser`] produces an
//! opaque [`ProtocolFrame`], and the turn's [`TurnParser`] attributes those
//! frames to the prompt it was constructed for. The arm runs both stages over
//! every recorded frame and `ProtocolFrame` stays opaque here too, so nothing
//! indexes into a decoded frame.
//!
//! A transcript records the agent's side of the session, so the turn's identity
//! is bound from the frames themselves: the native session is the
//! `params.sessionId` the session notifications carry, and the request id is
//! the one the prompt response admits the turn under. The driver owns both
//! before it constructs the parser — it wrote the prompt and holds the session
//! it resumes — so the frames read before the identity is bound are held and
//! applied to the parser the moment it exists, in arrival order, which is what
//! the driver's parser had already consumed by then. A transcript whose turn
//! was never admitted records no request id: such a turn is the transport's
//! first, which the driver numbers `prompt-1` (`format!("prompt-{}",
//! next_request_id)`, counted from one).

use crate::parser::{
    FrameError, FrameParser, ProtocolFrame, TurnParseError, TurnParser, initialize_accepted,
};
use licoup_agent_adapter_sdk::Transition;
use licoup_agent_adapter_sdk::adapters::NativeLineParser;
use licoup_agent_adapter_sdk::replay::{FrameReplay, RecordedFrame};
use serde_json::{Value, json};

use crate::registration::{ADAPTER_ID, FRAMING};

/// The request id a transport's first turn is sent as.
const FIRST_TURN_REQUEST_ID: &str = "prompt-1";

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

struct Replay {
    parser: FrameParser,
    /// The request id the turn was admitted under, when the transcript admitted
    /// one. Launch context only: it is never projected.
    request_id: Option<String>,
    /// The native session the turn belongs to. Launch context only.
    session_id: Option<String>,
    /// Frames read before the turn parser exists, in arrival order.
    pending: Vec<ProtocolFrame>,
    turn: Option<TurnParser>,
}

impl Replay {
    fn new() -> Result<Self, String> {
        Ok(Self {
            parser: FrameParser,
            request_id: None,
            session_id: None,
            pending: Vec::new(),
            turn: None,
        })
    }

    /// The framing this boundary consumes, taken from this package's own
    /// adapter declaration so a frame recorded under another channel cannot
    /// pass.
    fn framing() -> &'static str {
        FRAMING
    }

    /// Binds the turn the driver is about to replay, then constructs the parser
    /// for it. Nothing bound here contributes a projection: the request id and
    /// the native session are the two values `TurnParser::new` takes, and the
    /// driver takes them from the turn it dispatched rather than from the wire.
    fn bind_turn(&mut self, payload: &str) -> Result<(), String> {
        if self.turn.is_some() {
            return Ok(());
        }
        if let Ok(value) = serde_json::from_str::<Value>(payload) {
            let admits_turn = value
                .pointer("/result/messageId")
                .and_then(Value::as_str)
                .is_some();
            if self.request_id.is_none() && admits_turn {
                if let Some(request_id) = value
                    .get("id")
                    .and_then(Value::as_str)
                    .filter(|id| !id.is_empty())
                {
                    self.request_id = Some(request_id.to_owned());
                }
            }
            if let Some(session_id) = value
                .pointer("/params/sessionId")
                .and_then(Value::as_str)
                .filter(|id| !id.is_empty())
            {
                self.session_id = Some(session_id.to_owned());
            }
        }
        let Some(session_id) = self.session_id.clone() else {
            return Ok(());
        };
        let request_id = self
            .request_id
            .clone()
            .unwrap_or_else(|| FIRST_TURN_REQUEST_ID.to_owned());
        let mut turn = TurnParser::new(&request_id, &session_id);
        for pending in std::mem::take(&mut self.pending) {
            match turn.ingest(pending) {
                Ok(None) => {}
                Ok(Some(result)) => {
                    return Err(format!(
                        "deepseek-harness transcript reports the terminal result {:?} before it \
                         names the turn it belongs to",
                        result.turn_id
                    ));
                }
                Err(error) => {
                    return Err(format!(
                        "deepseek-harness transcript is rejected by the real parser before it \
                         names the turn it belongs to: {error:?}"
                    ));
                }
            }
        }
        self.turn = Some(turn);
        Ok(())
    }
}

impl FrameReplay for Replay {
    fn feed(&mut self, frame: &RecordedFrame) -> Result<Vec<Value>, String> {
        if frame.channel != Self::framing() {
            return Err(format!(
                "deepseek-harness frames cross the {:?} channel; {:?} is not this boundary's \
                 framing",
                Self::framing(),
                frame.channel
            ));
        }
        // Every recorded frame crosses the framing stage, exactly as the
        // driver's reader thread feeds it.
        let parsed = match self.parser.parse_line(frame.payload.as_bytes()) {
            Ok(parsed) => parsed,
            Err(error) => return Ok(vec![json!({"error": frame_error_json(error)})]),
        };
        let mut entries = vec![json!({"effect": "frame", "wireBytes": parsed.wire_bytes()})];
        if let Some(accepted) = initialize_accepted(&parsed) {
            entries.push(json!({"effect": "handshake", "accepted": accepted}));
        }
        self.bind_turn(&frame.payload)?;
        let Some(turn) = self.turn.as_mut() else {
            self.pending.push(parsed);
            return Ok(entries);
        };
        match turn.ingest(parsed) {
            Ok(None) => Ok(entries),
            Ok(Some(result)) => {
                entries.push(json!({
                    "effect": "turn",
                    "turnId": result.turn_id,
                    "output": result.output,
                    "transitions": result
                        .transitions
                        .iter()
                        .map(Transition::to_json)
                        .collect::<Vec<Value>>(),
                }));
                Ok(entries)
            }
            // The turn stage rejected the frame, so the frame is reported as
            // that rejection alone.
            Err(error) => Ok(vec![json!({"error": turn_error_json(error)})]),
        }
    }
}

fn frame_error_json(error: FrameError) -> &'static str {
    match error {
        FrameError::InvalidJson => "invalid_json",
        FrameError::OutputLimit => "output_limit",
    }
}

fn turn_error_json(error: TurnParseError) -> &'static str {
    match error {
        TurnParseError::Incomplete => "incomplete",
        TurnParseError::PromptRejected => "prompt_rejected",
        TurnParseError::SessionMismatch => "session_mismatch",
    }
}
