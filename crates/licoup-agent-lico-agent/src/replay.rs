//! Replay arm for the `lico-agent` adapter.
//!
//! One real [`RpcParser`] — the same line parser the driver's read loop runs —
//! consumes the recorded `lf-jsonl-jsonrpc` frames, so every projection is the
//! parser's own report and never a re-derivation of the payload. The arm travels
//! with the parser: a program that composes this package's parser set gets the
//! arm that can only fail when *this* parser regresses.
//!
//! A rejected frame is reported as the parser's own closed error kind rather
//! than as a `Err`, because the boundary *can* consume it: the parser classified
//! the line and its classification is the fact. `Err` is reserved for frames
//! this boundary cannot consume at all — a frame recorded on another channel.

use licoup_agent_adapter_sdk::adapters::NativeLineParser;
use licoup_agent_adapter_sdk::replay::{FrameReplay, RecordedFrame};
use serde_json::{Value, json};

use crate::parser::{FrameError, RpcEffect, RpcParser};
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
    Ok(Box::new(Replay::new()))
}

/// The parser this package's own driver constructs, driven from a transcript.
pub struct Replay {
    parser: RpcParser,
}

impl Replay {
    pub fn new() -> Self {
        Self { parser: RpcParser }
    }
}

impl Default for Replay {
    fn default() -> Self {
        Self::new()
    }
}

impl FrameReplay for Replay {
    fn feed(&mut self, frame: &RecordedFrame) -> Result<Vec<Value>, String> {
        // The framing is the adapter's own declaration, so a frame recorded
        // under another channel cannot pass here.
        if frame.channel != FRAMING {
            return Err(format!(
                "lico-agent frames cross the {FRAMING:?} channel; {:?} is not this boundary's framing",
                frame.channel
            ));
        }
        // The driver's own read loop parses one line with this parser, so the
        // arm does the same; the payload is the recorded line, newline excluded.
        match self.parser.parse_line(frame.payload.as_bytes()) {
            Ok(effect) => Ok(vec![effect_json(effect)]),
            Err(FrameError::Empty) => Ok(vec![json!({"error": "empty"})]),
            Err(FrameError::InvalidJson) => Ok(vec![json!({"error": "invalid_json"})]),
        }
    }
}

/// The parser's report, named by its own variant. `sessionId` and `code` are the
/// report's own `Option` fields and are omitted when the frame carried none, so
/// the projection never invents a value.
fn effect_json(effect: RpcEffect) -> Value {
    match effect {
        RpcEffect::Handshake {
            accepted,
            session_id,
        } => {
            let mut entry = json!({"effect": "handshake", "accepted": accepted});
            if let Some(session_id) = session_id {
                entry["sessionId"] = json!(session_id);
            }
            entry
        }
        RpcEffect::Text { delta } => json!({"effect": "text", "delta": delta}),
        RpcEffect::Processing => json!({"effect": "processing"}),
        RpcEffect::Control { method } => json!({"effect": "control", "method": method}),
        RpcEffect::Completed => json!({"effect": "completed"}),
        RpcEffect::Failed { code } => {
            let mut entry = json!({"effect": "failed"});
            if let Some(code) = code {
                entry["code"] = json!(code);
            }
            entry
        }
        RpcEffect::Ignored => json!({"effect": "ignored"}),
    }
}
