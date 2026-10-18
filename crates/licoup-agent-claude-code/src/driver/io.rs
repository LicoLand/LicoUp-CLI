use licoup_foundation::platform::process_supervisor::BoundedStdinWriter;
use licoup_foundation::platform::raw_execution::{
    RawExecutionBinding, RawExecutionDirection, RawExecutionObserver,
};
use serde_json::Value;
use std::io::{self, BufRead, Read};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::Sender;

pub(super) const MAX_PROTOCOL_LINE_BYTES: usize = 8 * 1024 * 1024;

#[derive(Debug)]
pub(super) enum TransportEvent {
    Line(Vec<u8>),
    LineLimitExceeded,
    StdoutReadFailed,
    StdoutClosed,
}

pub(super) fn write_message(stdin: &mut BoundedStdinWriter, message: &Value) -> io::Result<()> {
    let bytes = crate::protocol::parser::encode_message(message)?;
    if let Some(observer) = RawExecutionObserver::current() {
        observer.record_bytes("claude-code", RawExecutionDirection::Sent, &bytes);
    }
    stdin
        .enqueue(bytes)
        .map_err(|_| io::Error::other("Claude Code protocol write failed"))
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
            if !line.is_empty() && send_protocol_line(&line, &sender).is_err() {
                return;
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

pub(super) fn send_protocol_line(line: &[u8], sender: &Sender<TransportEvent>) -> Result<(), ()> {
    sender
        .send(TransportEvent::Line(line.to_vec()))
        .map_err(|_| ())
}

pub(super) fn drain_stderr(
    mut stderr: impl Read,
    max_bytes: usize,
    truncated: &AtomicBool,
    observer: &RawExecutionBinding,
) {
    let mut retained = 0usize;
    let mut buffer = [0u8; 8192];
    loop {
        match stderr.read(&mut buffer) {
            Ok(0) => return,
            Ok(read) => {
                observer.record_bytes(
                    "claude-code",
                    RawExecutionDirection::Stderr,
                    &buffer[..read],
                );
                let keep = max_bytes.saturating_sub(retained).min(read);
                retained = retained.saturating_add(keep);
                if keep < read {
                    truncated.store(true, Ordering::Relaxed);
                }
            }
            Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
            Err(_) => return,
        }
    }
}

pub(super) fn read_bounded(mut reader: impl Read, max_bytes: usize) -> bool {
    let mut observed = 0usize;
    let mut truncated = false;
    let mut buffer = [0u8; 8192];
    loop {
        match reader.read(&mut buffer) {
            Ok(0) => return truncated,
            Ok(read) => {
                observed = observed.saturating_add(read);
                truncated |= observed > max_bytes;
            }
            Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
            Err(_) => return true,
        }
    }
}
