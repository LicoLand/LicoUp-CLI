use crate::parser::codec;
use licoup_foundation::platform::process_supervisor::BoundedStdinWriter;
use licoup_foundation::platform::raw_execution::{RawExecutionDirection, RawExecutionObserver};
use serde_json::Value;
use std::io::{self, BufRead, Read};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::Sender;

#[derive(Debug)]
pub(super) enum TransportEvent {
    Frame(Vec<u8>),
    StdoutLimitExceeded,
    StdoutReadFailed,
    StdoutClosed,
}

pub(super) fn write_message(stdin: &mut BoundedStdinWriter, message: &Value) -> io::Result<()> {
    let bytes = codec::encode_message(message)?;
    if let Some(observer) = RawExecutionObserver::current() {
        observer.record_bytes("openclaw", RawExecutionDirection::Sent, &bytes);
    }
    stdin
        .enqueue(bytes)
        .map_err(|_| io::Error::other("native agent protocol write failed"))
}

pub(super) fn read_protocol_messages<R: BufRead>(
    mut reader: R,
    max_bytes: Option<usize>,
    sender: Sender<TransportEvent>,
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

pub(super) fn send_protocol_line(line: &[u8], sender: &Sender<TransportEvent>) -> Result<(), ()> {
    sender
        .send(TransportEvent::Frame(line.to_vec()))
        .map_err(|_| ())
}

pub(super) fn drain_stderr<R: Read>(mut stderr: R, max_bytes: usize, truncated: &AtomicBool) {
    let mut buffer = [0u8; 8192];
    let mut total_bytes = 0usize;
    loop {
        match stderr.read(&mut buffer) {
            Ok(0) => return,
            Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
            Err(_) => return,
            Ok(read) => {
                if let Some(observer) = RawExecutionObserver::current() {
                    observer.record_bytes(
                        "openclaw",
                        RawExecutionDirection::Stderr,
                        &buffer[..read],
                    );
                }
                total_bytes = total_bytes.saturating_add(read);
                if total_bytes > max_bytes {
                    truncated.store(true, Ordering::Relaxed);
                }
            }
        }
    }
}

pub(super) fn read_bounded<R: Read>(mut reader: R, max_bytes: usize) -> (Vec<u8>, bool) {
    let mut kept = Vec::with_capacity(max_bytes.min(8192));
    let mut buffer = [0u8; 8192];
    let mut total = 0usize;
    loop {
        match reader.read(&mut buffer) {
            Ok(0) => return (kept, total > max_bytes),
            Ok(read) => {
                let remaining = max_bytes.saturating_sub(kept.len());
                kept.extend_from_slice(&buffer[..read.min(remaining)]);
                total = total.saturating_add(read);
            }
            Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
            Err(_) => return (kept, true),
        }
    }
}
