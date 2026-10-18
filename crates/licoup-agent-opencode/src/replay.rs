//! The recorded-transcript replay arm for `opencode`.
//!
//! The `serve` protocol consumes three real document kinds: the session identity
//! document returned by the session endpoint (through [`parser::session_id`]),
//! the SSE `data:` payload of the event stream (through
//! [`parser::ServeEventParser::observe`]), and the whole-message document
//! returned by the message endpoint (through [`parser::message`]). A recorded
//! frame is routed to the same entry point the production driver uses for it,
//! and the projection is that entry point's own answer. Nothing here re-parses a
//! vendor frame, and nothing here is a second implementation of the protocol:
//! the arm only decides *which* of this package's three entry points a recorded
//! frame reaches.
//!
//! The projection, the corpus resolution and the fail-closed properties stay in
//! the SDK's harness; this module is the one thing the harness cannot know — how
//! to build *this* Agent's arm.

use licoup_agent_adapter_sdk::replay::{FrameReplay, RecordedFrame};

use crate::parser::{self, ServeEventParser};
use crate::registration::ADAPTER_ID;
use serde_json::{Value, json};

/// Build the replay arm for this package's adapter.
///
/// An adapter id this package does not carry is refused rather than defaulted,
/// so a fixture can never pass against a parser that was never constructed.
pub fn replay_arm(adapter_id: &str) -> Result<Box<dyn FrameReplay>, String> {
    if adapter_id != ADAPTER_ID {
        return Err(format!(
            "no replayable parser is registered for adapter {adapter_id}"
        ));
    }
    Ok(Box::new(Replay::new()))
}

struct Replay {
    parser: ServeEventParser,
}

impl Replay {
    fn new() -> Self {
        // The driver builds this parser once the session identity is known. The
        // arm rebinds it from the recorded identity document before any stream
        // frame is fed, so the initial unbound binding is never observed.
        Self {
            parser: ServeEventParser::new(""),
        }
    }
}

impl FrameReplay for Replay {
    fn feed(&mut self, frame: &RecordedFrame) -> Result<Vec<Value>, String> {
        let document = serde_json::from_str::<Value>(&frame.payload);
        if let Ok(value) = &document
            && let Some(identity) = parser::session_id(value)
        {
            // The driver binds the stream parser to the identity carried by the
            // session endpoint's own document.
            self.parser = ServeEventParser::new(identity);
            return Ok(vec![json!({"effect": "session", "sessionId": identity})]);
        }
        match self.parser.observe(&frame.payload) {
            Ok(Some(text)) => Ok(vec![json!({"effect": "text", "text": text})]),
            // The parser's own rejection: `ServeEventFailure::InvalidJson` is a
            // unit, so the fact it carries is only that the payload did not
            // decode. The driver's protocol-failure code for the same event is
            // deliberately not projected: the fixture must fail when the parser
            // changes, not when the driver renames a code.
            Err(_) => Ok(vec![json!({"error": "invalid_json"})]),
            Ok(None) => Ok(document
                .ok()
                .and_then(|value| parser::message(&value))
                .map(|report| {
                    vec![json!({
                        "effect": "message",
                        "output": report.output,
                        "transitions": report
                            .transitions
                            .iter()
                            .map(|transition| transition.to_json())
                            .collect::<Vec<Value>>(),
                    })]
                })
                // `message` answering `None` is the parser's literal answer, and
                // this adapter has no in-band error terminal: a serve turn fails
                // out of band (HTTP status, the abort control lane), so a
                // whole-message document without assistant text leaves the
                // parser with nothing to report. The frame is recorded, its
                // projection is empty.
                .unwrap_or_default()),
        }
    }
}
