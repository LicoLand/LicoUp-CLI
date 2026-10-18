// Shared bounded stdio framing feeding the Hermes parser exactly once.
use licoup_foundation::core::acp;
use std::io::BufRead;
use std::sync::mpsc::Sender;

#[derive(Debug)]
pub enum TransportEvent {
    Message {
        message: serde_json::Value,
        bytes: usize,
    },
    InvalidJson,
    LineLimitExceeded,
    StdoutReadFailed,
    StdoutClosed,
}

#[derive(Debug)]
pub enum ConversationTransportEvent {
    Frame { line: Vec<u8>, bytes: usize },
    LineLimitExceeded,
    StdoutReadFailed,
    StdoutClosed,
}

pub fn read_protocol_messages<R: BufRead>(reader: R, sender: Sender<TransportEvent>) {
    read_protocol_lines(reader, sender)
}

pub fn read_conversation_frames<R: BufRead>(
    mut reader: R,
    sender: Sender<ConversationTransportEvent>,
) {
    let mut line = Vec::new();
    loop {
        let available = match reader.fill_buf() {
            Ok(bytes) => bytes,
            Err(_) => {
                let _ = sender.send(ConversationTransportEvent::StdoutReadFailed);
                return;
            }
        };
        if available.is_empty() {
            if !line.is_empty() && send_conversation_line(&line, &sender).is_err() {
                return;
            }
            let _ = sender.send(ConversationTransportEvent::StdoutClosed);
            return;
        }
        let consumed = available
            .iter()
            .position(|byte| *byte == b'\n')
            .map(|index| index + 1)
            .unwrap_or(available.len());
        if line.len().saturating_add(consumed) > acp::MAX_JSON_LINE_BYTES {
            let _ = sender.send(ConversationTransportEvent::LineLimitExceeded);
            return;
        }
        let completed_line = available.get(consumed.saturating_sub(1)) == Some(&b'\n');
        line.extend_from_slice(&available[..consumed]);
        reader.consume(consumed);
        if completed_line {
            while matches!(line.last(), Some(b'\n' | b'\r')) {
                line.pop();
            }
            if !line.is_empty() && send_conversation_line(&line, &sender).is_err() {
                return;
            }
            line.clear();
        }
    }
}

fn send_conversation_line(
    line: &[u8],
    sender: &Sender<ConversationTransportEvent>,
) -> Result<(), ()> {
    sender
        .send(ConversationTransportEvent::Frame {
            line: line.to_vec(),
            bytes: line.len(),
        })
        .map_err(|_| ())
}

fn read_protocol_lines<R: BufRead>(mut reader: R, sender: Sender<TransportEvent>) {
    let mut line = Vec::new();
    loop {
        let available = match reader.fill_buf() {
            Ok(bytes) => bytes,
            Err(_) => {
                let _ = sender.send(TransportEvent::StdoutReadFailed);
                return;
            }
        };
        if available.is_empty() {
            if !line.is_empty() && send_protocol_line(&line, &sender).is_err() {
                return;
            }
            let _ = sender.send(TransportEvent::StdoutClosed);
            return;
        }
        let consumed = available
            .iter()
            .position(|byte| *byte == b'\n')
            .map(|index| index + 1)
            .unwrap_or(available.len());
        if line.len().saturating_add(consumed) > acp::MAX_JSON_LINE_BYTES {
            let _ = sender.send(TransportEvent::LineLimitExceeded);
            return;
        }
        let completed_line = available.get(consumed.saturating_sub(1)) == Some(&b'\n');
        line.extend_from_slice(&available[..consumed]);
        reader.consume(consumed);
        if completed_line {
            while matches!(line.last(), Some(b'\n' | b'\r')) {
                line.pop();
            }
            if !line.is_empty() && send_protocol_line(&line, &sender).is_err() {
                return;
            }
            line.clear();
        }
    }
}

pub fn send_protocol_line(line: &[u8], sender: &Sender<TransportEvent>) -> Result<(), ()> {
    match acp::decode_json_line(line) {
        Ok(message) => sender
            .send(TransportEvent::Message {
                message,
                bytes: line.len(),
            })
            .map_err(|_| ()),
        Err(acp::AcpError::MessageTooLarge) => sender
            .send(TransportEvent::LineLimitExceeded)
            .map_err(|_| ()),
        Err(_) => sender.send(TransportEvent::InvalidJson).map_err(|_| ()),
    }
}

pub fn request_id_matches(message: &serde_json::Value, expected: i64) -> bool {
    message.get("id").is_some_and(|id| {
        id.as_i64() == Some(expected)
            || id
                .as_str()
                .is_some_and(|value| value == expected.to_string())
    })
}
