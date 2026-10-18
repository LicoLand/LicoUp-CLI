//! Replay arm for the `kilo-code` adapter.
//!
//! One real [`ServeEventParser`] — the same parser a live serve turn drives —
//! consumes the recorded `http-sse` frames. The serve driver consumes three real
//! document kinds: the session identity document returned by the session
//! endpoint (through `session_id`), the SSE frame payload of the event stream
//! (through `observe`), and the whole-message document returned by the message
//! endpoint (through `message`). A recorded frame is routed to the same entry
//! point the driver uses for it, and the projection is that entry point's own
//! answer. Nothing here re-parses a vendor frame.
//!
//! The arm travels with the parser: a program that composes this package's
//! parser set gets the arm that can only fail when *this* parser regresses.

use licoup_agent_adapter_sdk::replay::{FrameReplay, RecordedFrame};
use serde_json::{Value, json};

use crate::parser::{self, ServeEventParser};
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

/// The parser this package's own driver constructs, built for one transcript.
pub struct Replay {
    parser: ServeEventParser,
}

impl Replay {
    /// A fresh arm, unbound until the transcript's identity document arrives.
    ///
    /// The driver builds its parser once the session identity is known; the arm
    /// is built before any frame is fed, so it starts unbound and rebinds from
    /// the recorded identity document. The initial unbound binding is therefore
    /// never observed.
    pub fn new() -> Self {
        Self {
            parser: ServeEventParser::new(""),
        }
    }
}

impl Default for Replay {
    fn default() -> Self {
        Self::new()
    }
}

impl FrameReplay for Replay {
    fn feed(&mut self, frame: &RecordedFrame) -> Result<Vec<Value>, String> {
        if frame.channel != FRAMING {
            return Err(format!(
                "the kilo-code serve transcript records the {:?} channel; this parser speaks \
                 {FRAMING}",
                frame.channel
            ));
        }
        if frame.direction != "agent-to-client" {
            return Err(format!(
                "the kilo-code serve transcript records the agent's side of the session; a {:?} \
                 frame cannot be applied to a parser that has already consumed a response",
                frame.direction
            ));
        }
        let document = serde_json::from_str::<Value>(&frame.payload);
        if let Ok(value) = &document
            && let Some(identity) = parser::session_id(value)
        {
            // `open_session` binds the stream parser to the identity carried by
            // the session endpoint's own document.
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
                            .map(licoup_agent_adapter_sdk::Transition::to_json)
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

#[cfg(test)]
mod tests {
    use super::*;

    fn frame(channel: &str, direction: &str, payload: &str) -> RecordedFrame {
        RecordedFrame {
            index: 0,
            direction: direction.to_owned(),
            channel: channel.to_owned(),
            payload: payload.to_owned(),
        }
    }

    #[test]
    fn the_arm_refuses_an_adapter_it_does_not_carry() {
        assert!(replay_arm("codex").is_err());
        assert!(replay_arm(ADAPTER_ID).is_ok());
    }

    #[test]
    fn another_channel_and_the_clients_own_direction_are_refused() {
        let mut replay = Replay::new();
        assert!(
            replay
                .feed(&frame("stdio-jsonrpc", "agent-to-client", "{}"))
                .is_err()
        );
        assert!(
            replay
                .feed(&frame(FRAMING, "client-to-agent", "{}"))
                .is_err()
        );
    }

    #[test]
    fn the_arm_routes_each_document_kind_to_the_entry_point_the_driver_uses() {
        let mut replay = Replay::new();
        let identity = replay
            .feed(&frame(
                FRAMING,
                "agent-to-client",
                r#"{"id":"kilo-1","title":"synthetic-session"}"#,
            ))
            .unwrap();
        assert_eq!(
            identity,
            vec![json!({"effect": "session", "sessionId": "kilo-1"})]
        );

        let announced = replay
            .feed(&frame(
                FRAMING,
                "agent-to-client",
                r#"{"type":"message.updated","properties":{"info":{"id":"m","role":"assistant","sessionID":"kilo-1"}}}"#,
            ))
            .unwrap();
        assert_eq!(announced, Vec::<Value>::new());

        let chunk = replay
            .feed(&frame(
                FRAMING,
                "agent-to-client",
                r#"{"type":"message.part.updated","properties":{"sessionID":"kilo-1","part":{"messageID":"m","type":"text","text":"hi"}}}"#,
            ))
            .unwrap();
        assert_eq!(chunk, vec![json!({"effect": "text", "text": "hi"})]);

        assert_eq!(
            replay
                .feed(&frame(FRAMING, "agent-to-client", "{"))
                .unwrap(),
            vec![json!({"error": "invalid_json"})]
        );

        let message = replay
            .feed(&frame(
                FRAMING,
                "agent-to-client",
                r#"{"parts":[{"type":"text","text":"answer"}]}"#,
            ))
            .unwrap();
        assert_eq!(message[0]["effect"], json!("message"));
        assert_eq!(message[0]["output"], json!("answer"));
        // submitted, accepted, processing, responding, the reply text, completed
        assert_eq!(
            message[0]["transitions"].as_array().unwrap().len(),
            6,
            "the shared lifecycle emits its initial stage before advancing"
        );
    }
}
