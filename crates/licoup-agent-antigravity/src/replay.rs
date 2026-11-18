//! Replay arm for the `antigravity` adapter.
//!
//! This adapter has no line parser. Its real boundary is the three entries the
//! driver calls directly: the vendor Stop-hook receipt reader
//! (`parse_hook_receipt`), the PTY stdout parser (`PtyOutputParser`), and the
//! terminal classification over the collected turn (`classify_terminal`). The
//! arm drives exactly those, so no vendor frame is re-interpreted here.
//!
//! A recorded antigravity frame is one of three shapes, disambiguated in this
//! order so that no frame is skipped silently:
//!
//! 1. a hook receipt — `parse_hook_receipt` accepts the payload and yields the
//!    native conversation id, exactly as the driver reads the Stop-hook file;
//! 2. otherwise the terminal frame — the PTY lane's process outcome
//!    (`exitSuccess`, `timedOut`) together with the launch-time
//!    `requestedSession` the turn resumed on. These are the `TerminalFacts` the
//!    driver supplies from the process it supervises; they never appear on the
//!    vendor wire, so the transcript records them as the turn's terminal frame;
//! 3. otherwise the payload is raw PTY stdout, ANSI control included, fed to
//!    `PtyOutputParser::push` and accumulated until the terminal frame.
//!
//! A frame carrying both a receipt and a process outcome runs the receipt first
//! and the terminal classification second, which is the order the driver reads
//! them in. Nothing is timed or derived from the clock.
//!
//! The arm travels with the parser: a program that composes this package's
//! parser set gets the arm that can only fail when *this* parser regresses. The
//! framing it accepts is read from this package's own declaration, so a frame
//! recorded under another channel cannot pass.

use licoup_agent_adapter_sdk::replay::{FrameReplay, RecordedFrame};
use serde_json::{Value, json};

use crate::parser::{
    CONTRACT, PtyOutputParser, TerminalFacts, classify_terminal, parse_hook_receipt,
};
use crate::registration::ADAPTER_ID;
use licoup_agent_adapter_sdk::Transition;

pub struct Replay {
    /// The live stdout parser: strips ANSI control and accumulates the turn's
    /// output exactly as the driver's PTY lane does.
    pty: PtyOutputParser,
    /// The conversation the caller asked to resume (`--conversation`), which is
    /// a launch fact the terminal frame records.
    requested_session: String,
    /// The native conversation the Stop-hook receipt bound this turn to.
    receipt_session: Option<String>,
}

impl Replay {
    pub fn new() -> Result<Self, String> {
        Ok(Self {
            pty: PtyOutputParser::new(),
            requested_session: String::new(),
            receipt_session: None,
        })
    }

    /// The framing this boundary consumes, taken from this package's own
    /// declaration so a frame recorded under another channel cannot pass.
    fn framing() -> &'static str {
        CONTRACT.framing
    }

    /// Classify the turn from the state this boundary accumulated and the
    /// process outcome the terminal frame recorded. `entries` already holds the
    /// facts reported earlier in the same frame — a receipt binds the native
    /// identity before the turn is classified, which is the order the driver
    /// reads them in.
    fn finish_turn(
        &mut self,
        outcome: ProcessOutcome,
        mut entries: Vec<Value>,
    ) -> Result<Vec<Value>, String> {
        if let Some(requested_session) = outcome.requested_session {
            self.requested_session = requested_session;
        }
        // The driver finishes the stdout parser at end of stream and classifies
        // the turn over what it collected.
        let (output, tail) = std::mem::replace(&mut self.pty, PtyOutputParser::new()).finish();
        match classify_terminal(TerminalFacts {
            requested_session: &self.requested_session,
            receipt_session: self.receipt_session.as_deref(),
            output: &output,
            timed_out: outcome.timed_out,
            exit_success: outcome.exit_success,
        }) {
            // The terminal classification rejected the turn: its own failure,
            // reported as that rejection alone.
            Err(failure) => Ok(vec![json!({
                "error": failure.code,
                "message": failure.message,
                "stage": failure.stage,
                "sessionId": failure.session_id,
            })]),
            Ok(success) => {
                // A partial escape sequence at end of stream is flushed as one
                // final chunk, exactly as the driver emits it.
                if let Some(tail) = tail.filter(|tail| !tail.is_empty()) {
                    entries.push(json!({"effect": "pty_text", "text": tail}));
                }
                entries.push(json!({
                    "effect": "terminal",
                    "sessionId": success.session_id,
                    "output": success.output,
                    "transitions": success
                        .transitions
                        .iter()
                        .map(Transition::to_json)
                        .collect::<Vec<Value>>(),
                }));
                Ok(entries)
            }
        }
    }
}

impl FrameReplay for Replay {
    fn feed(&mut self, frame: &RecordedFrame) -> Result<Vec<Value>, String> {
        if frame.channel != Self::framing() {
            return Err(format!(
                "antigravity frames cross the {:?} channel; {:?} is not this boundary's framing",
                Self::framing(),
                frame.channel
            ));
        }
        if let Some(session_id) = parse_hook_receipt(&frame.payload) {
            self.receipt_session = Some(session_id.clone());
            let identity = json!({"effect": "session_identity", "sessionId": session_id});
            return match process_outcome(&frame.payload) {
                Some(outcome) => self.finish_turn(outcome, vec![identity]),
                None => Ok(vec![identity]),
            };
        }
        if let Some(outcome) = process_outcome(&frame.payload) {
            return self.finish_turn(outcome, Vec::new());
        }
        Ok(self
            .pty
            .push(frame.payload.as_bytes())
            .map(|text| vec![json!({"effect": "pty_text", "text": text})])
            .unwrap_or_default())
    }
}

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

/// The PTY lane's process outcome, recorded as the turn's terminal frame: the
/// facts `classify_terminal` consumes that the vendor wire cannot carry. A frame
/// that records no deadline is a turn that was not timed out; a frame that
/// records no exit status is treated as a failed turn.
struct ProcessOutcome {
    requested_session: Option<String>,
    timed_out: bool,
    exit_success: bool,
}

fn process_outcome(payload: &str) -> Option<ProcessOutcome> {
    let value: Value = serde_json::from_str(payload).ok()?;
    let object = value.as_object()?;
    if !object.contains_key("exitSuccess") && !object.contains_key("timedOut") {
        return None;
    }
    Some(ProcessOutcome {
        requested_session: object
            .get("requestedSession")
            .and_then(Value::as_str)
            .filter(|value| !value.is_empty())
            .map(str::to_owned),
        timed_out: object
            .get("timedOut")
            .and_then(Value::as_bool)
            .unwrap_or(false),
        exit_success: object
            .get("exitSuccess")
            .and_then(Value::as_bool)
            .unwrap_or(false),
    })
}
