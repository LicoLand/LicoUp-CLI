// Shared bounded LF framing for the Copilot and Kimi ACP parser policies.
use licoup_foundation::core::acp;
use std::io::BufRead;
use std::sync::mpsc::SyncSender;

pub const ACP_EVENT_CHANNEL_CAPACITY: usize = 64;

#[derive(Debug)]
pub enum TransportEvent {
    Frame(Vec<u8>),
    StdoutLimitExceeded,
    StdoutReadFailed,
    StdoutClosed,
}

pub fn read_protocol_messages<R: BufRead>(
    mut reader: R,
    max_bytes: Option<usize>,
    sender: SyncSender<TransportEvent>,
) {
    let mut total_bytes = 0usize;
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
        if max_bytes.is_some_and(|max_bytes| total_bytes.saturating_add(consumed) > max_bytes) {
            let _ = sender.send(TransportEvent::StdoutLimitExceeded);
            return;
        }
        if line.len().saturating_add(consumed) > acp::MAX_JSON_LINE_BYTES {
            let _ = sender.send(TransportEvent::StdoutLimitExceeded);
            return;
        }
        let completed_line = available.get(consumed.saturating_sub(1)) == Some(&b'\n');
        line.extend_from_slice(&available[..consumed]);
        reader.consume(consumed);
        total_bytes += consumed;
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

pub fn send_protocol_line(line: &[u8], sender: &SyncSender<TransportEvent>) -> Result<(), ()> {
    sender
        .send(TransportEvent::Frame(line.to_vec()))
        .map_err(|_| ())
}
