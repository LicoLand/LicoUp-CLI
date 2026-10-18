use std::io::BufRead;
use std::sync::mpsc::Sender;

pub(super) const MAX_PROTOCOL_LINE_BYTES: usize = 8 * 1024 * 1024;

#[derive(Debug)]
pub(super) enum TransportEvent {
    Line(Vec<u8>),
    UnterminatedLine,
    LineLimitExceeded,
    StdoutReadFailed,
    StdoutClosed,
}

pub(super) fn read_protocol_messages<R: BufRead>(mut reader: R, sender: Sender<TransportEvent>) {
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
            if !line.is_empty() {
                let clean = isolate_pty_protocol_line(&line);
                if clean.iter().all(|byte| byte.is_ascii_whitespace()) {
                    // Ignore a final PTY-only whitespace tail.
                } else if serde_json::from_slice::<serde_json::Value>(&clean).is_ok() {
                    // EOF is an unambiguous boundary for one complete JSON
                    // value even when the CLI omitted the final NDJSON newline.
                    if sender.send(TransportEvent::Line(clean)).is_err() {
                        return;
                    }
                } else {
                    let _ = sender.send(TransportEvent::UnterminatedLine);
                }
            }
            let _ = sender.send(TransportEvent::StdoutClosed);
            return;
        }
        let consumed = available
            .iter()
            .position(|byte| *byte == b'\n')
            .map_or(available.len(), |index| index + 1);
        if line.len().saturating_add(consumed) > MAX_PROTOCOL_LINE_BYTES {
            let _ = sender.send(TransportEvent::LineLimitExceeded);
            return;
        }
        line.extend_from_slice(&available[..consumed]);
        reader.consume(consumed);
        if line.last() == Some(&b'\n') {
            if send_protocol_line(&line, &sender).is_err() {
                return;
            }
            line.clear();
        }
    }
}

fn send_protocol_line(line: &[u8], sender: &Sender<TransportEvent>) -> Result<(), ()> {
    let line = isolate_pty_protocol_line(line);
    if line.iter().all(|byte| byte.is_ascii_whitespace()) {
        return Ok(());
    }
    sender.send(TransportEvent::Line(line)).map_err(|_| ())
}

/// Remove only recognized PTY control contamination before the strict NDJSON
/// parser sees the line. Nonempty printable prose remains intact and therefore
/// fails protocol decoding instead of being scraped or ignored.
pub(super) fn isolate_pty_protocol_line(input: &[u8]) -> Vec<u8> {
    let mut clean = Vec::with_capacity(input.len());
    let mut index = 0;
    while index < input.len() {
        if input[index] == 0x1b && input.get(index + 1) == Some(&b'[') {
            index += 2;
            while index < input.len() {
                let byte = input[index];
                index += 1;
                if (0x40..=0x7e).contains(&byte) {
                    break;
                }
            }
            continue;
        }
        if matches!(input[index], b'\r' | b'\n' | b'\t') || !input[index].is_ascii_control() {
            clean.push(input[index]);
        }
        index += 1;
    }
    clean
}
