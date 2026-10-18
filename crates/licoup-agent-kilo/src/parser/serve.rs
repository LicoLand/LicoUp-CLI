//! The Kilo Code `serve` event stream, classified once.
//!
//! One turn reports its progress as an SSE stream of OpenCode-compatible
//! documents. Only two of them carry assistant text, and only in one shape: a
//! `message.updated` document naming an assistant message, followed by
//! `message.part.updated` documents whose part is that message's `text`. The
//! parser binds the session identity it was constructed with and keeps the
//! assistant message ids it has seen, so a reasoning part, another session's
//! part, or a user's own part is never projected as the reply.
//!
//! A document that does not decode is the parser's own failure. Every other
//! document — including a kind this parser does not know — is reported as
//! nothing to say, because a stream that carries progress this Agent does not
//! project is not a malformed stream.

use serde_json::Value;
use std::collections::HashSet;

/// Why one stream document could not be classified.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ServeEventFailure {
    /// The document did not decode as JSON.
    InvalidJson,
}

/// The event parser one serve turn drives.
///
/// It is built once the session identity is known and is exact about it: a
/// document belonging to any other session is not this turn's progress.
pub struct ServeEventParser {
    session_id: String,
    assistant_messages: HashSet<String>,
}

impl ServeEventParser {
    /// Bind a parser to one session identity.
    pub fn new(session_id: &str) -> Self {
        Self {
            session_id: session_id.to_owned(),
            assistant_messages: HashSet::new(),
        }
    }

    /// The session identity this parser is bound to.
    pub fn session_id(&self) -> &str {
        &self.session_id
    }

    /// Classify one stream document, reporting assistant text when it carries
    /// some.
    pub fn observe(&mut self, frame: &str) -> Result<Option<String>, ServeEventFailure> {
        let event =
            serde_json::from_str::<Value>(frame).map_err(|_| ServeEventFailure::InvalidJson)?;
        let Some(event_type) = event.get("type").and_then(Value::as_str) else {
            return Ok(None);
        };
        let Some(properties) = event.get("properties") else {
            return Ok(None);
        };
        if event_type == "message.updated" {
            // The assistant message identity arrives before its parts do. Only
            // this session's assistant messages are remembered, so a part naming
            // an unknown or foreign message is later ignored.
            if let Some(info) = properties.get("info")
                && session_id(info) == Some(self.session_id.as_str())
                && info.get("role").and_then(Value::as_str) == Some("assistant")
                && let Some(message_id) = info.get("id").and_then(Value::as_str)
            {
                self.assistant_messages.insert(message_id.to_owned());
            }
            return Ok(None);
        }
        if event_type != "message.part.updated"
            || session_id(properties) != Some(self.session_id.as_str())
        {
            return Ok(None);
        }
        let Some(part) = properties.get("part") else {
            return Ok(None);
        };
        if part.get("type").and_then(Value::as_str) != Some("text") {
            return Ok(None);
        }
        let Some(message_id) = message_id(part) else {
            return Ok(None);
        };
        if !self.assistant_messages.contains(message_id) {
            return Ok(None);
        }
        Ok(part
            .get("text")
            .and_then(Value::as_str)
            .filter(|text| !text.is_empty())
            .map(str::to_owned))
    }
}

fn session_id(frame: &Value) -> Option<&str> {
    frame
        .get("sessionID")
        .or_else(|| frame.get("sessionId"))
        .or_else(|| frame.get("id"))
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
}

fn message_id(value: &Value) -> Option<&str> {
    value
        .get("messageID")
        .or_else(|| value.get("messageId"))
        .and_then(Value::as_str)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn serve_event_parser_is_exact_session_and_assistant_only() {
        let mut parser = ServeEventParser::new("kilo-1");
        assert_eq!(parser.session_id(), "kilo-1");
        let assistant = json!({
            "type": "message.updated",
            "properties": {"info": {
                "id": "agent", "role": "assistant", "sessionID": "kilo-1"
            }}
        });
        assert_eq!(parser.observe(&assistant.to_string()), Ok(None));
        let part = json!({
            "type": "message.part.updated",
            "properties": {
                "sessionID": "kilo-1",
                "part": {"messageID": "agent", "type": "text", "text": "delta"}
            }
        });
        assert_eq!(parser.observe(&part.to_string()), Ok(Some("delta".into())));
        assert_eq!(parser.observe("{"), Err(ServeEventFailure::InvalidJson));
    }

    #[test]
    fn foreign_sessions_user_parts_and_reasoning_are_not_the_reply() {
        let mut parser = ServeEventParser::new("kilo-1");
        let text_part = |session: &str, message: &str, kind: &str| {
            json!({
                "type": "message.part.updated",
                "properties": {
                    "sessionID": session,
                    "part": {"messageID": message, "type": kind, "text": "leak"}
                }
            })
            .to_string()
        };
        // A text part for an assistant message that was never announced.
        assert_eq!(
            parser.observe(&text_part("kilo-1", "agent", "text")),
            Ok(None)
        );
        // Announce an assistant message on a *different* session: it must not
        // become this parser's.
        let foreign = json!({
            "type": "message.updated",
            "properties": {"info": {
                "id": "other", "role": "assistant", "sessionID": "kilo-2"
            }}
        });
        assert_eq!(parser.observe(&foreign.to_string()), Ok(None));
        assert_eq!(
            parser.observe(&text_part("kilo-1", "other", "text")),
            Ok(None)
        );
        // A user message of this session is announced but is never the reply.
        let user = json!({
            "type": "message.updated",
            "properties": {"info": {
                "id": "mine", "role": "user", "sessionID": "kilo-1"
            }}
        });
        assert_eq!(parser.observe(&user.to_string()), Ok(None));
        assert_eq!(
            parser.observe(&text_part("kilo-1", "mine", "text")),
            Ok(None)
        );
        // Documents without a kind, without properties, and unknown kinds are
        // progress this parser does not project — not malformed input.
        assert_eq!(parser.observe(&json!({}).to_string()), Ok(None));
        assert_eq!(
            parser.observe(&json!({"type": "session.idle"}).to_string()),
            Ok(None)
        );
        assert_eq!(
            parser.observe(&json!({"type": "session.idle", "properties": {}}).to_string()),
            Ok(None)
        );
        // A reasoning part of an announced assistant message is not text, and an
        // empty text part says nothing rather than reporting an empty chunk.
        let announced = json!({
            "type": "message.updated",
            "properties": {"info": {
                "id": "mine2", "role": "assistant", "sessionID": "kilo-1"
            }}
        });
        assert_eq!(parser.observe(&announced.to_string()), Ok(None));
        assert_eq!(
            parser.observe(&text_part("kilo-1", "mine2", "reasoning")),
            Ok(None)
        );
        let empty = json!({
            "type": "message.part.updated",
            "properties": {
                "sessionID": "kilo-1",
                "part": {"messageID": "mine2", "type": "text", "text": ""}
            }
        });
        assert_eq!(parser.observe(&empty.to_string()), Ok(None));
    }
}
