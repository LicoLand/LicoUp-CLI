use licoup_foundation::platform::process_supervisor::BoundedStdinWriter;
use licoup_foundation::platform::raw_execution::{
    RawExecutionBinding, RawExecutionDirection, RawExecutionObserver, RawExecutionReader,
};
use serde_json::Value;
use std::io::{self, BufRead, BufReader, Read};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::Sender;
use std::time::Instant;

#[derive(Debug)]
pub(super) enum TransportEvent {
    Line { line: String, received_at: Instant },
    StdoutLimitExceeded,
    StdoutReadFailed,
    StdoutClosed,
}

pub(super) fn write_message(stdin: &mut BoundedStdinWriter, message: &Value) -> io::Result<()> {
    let mut payload = serde_json::to_vec(message).map_err(io::Error::other)?;
    payload.push(b'\n');
    if let Some(observer) = RawExecutionObserver::current() {
        observer.record_bytes("pi.stdin", RawExecutionDirection::Sent, &payload);
    }
    stdin
        .enqueue(payload)
        .map_err(|_| io::Error::other("native agent protocol write failed"))
}

pub(super) fn read_protocol_messages<R: Read>(
    reader: R,
    max_stdout: Option<usize>,
    sender: Sender<TransportEvent>,
) {
    let binding = RawExecutionBinding::default();
    let _raw_binding = binding.bind_current();
    let mut reader = BufReader::new(RawExecutionReader::new(
        reader,
        binding,
        "pi.stdout",
        RawExecutionDirection::Received,
    ));
    let mut total = 0usize;
    let mut line = Vec::new();
    loop {
        line.clear();
        let result = reader.read_until(b'\n', &mut line);
        match result {
            Ok(0) => {
                let _ = sender.send(TransportEvent::StdoutClosed);
                return;
            }
            Ok(read) => {
                if let Some(max_stdout) = max_stdout {
                    total = total.saturating_add(read);
                    if total > max_stdout {
                        let _ = sender.send(TransportEvent::StdoutLimitExceeded);
                        return;
                    }
                }
                let Ok(text) = String::from_utf8(std::mem::take(&mut line)) else {
                    let _ = sender.send(TransportEvent::StdoutReadFailed);
                    return;
                };
                if sender
                    .send(TransportEvent::Line {
                        line: text,
                        received_at: Instant::now(),
                    })
                    .is_err()
                {
                    return;
                }
            }
            Err(_) => {
                let _ = sender.send(TransportEvent::StdoutReadFailed);
                return;
            }
        }
    }
}

pub(super) fn drain_stderr<R: Read>(reader: R, max_bytes: usize, truncated: &Arc<AtomicBool>) {
    let mut reader = reader;
    let mut buffer = [0_u8; 8 * 1024];
    let mut kept = 0usize;
    let raw_observer = RawExecutionObserver::current();
    loop {
        match reader.read(&mut buffer) {
            Ok(0) => return,
            Ok(read) => {
                if let Some(observer) = raw_observer.as_ref() {
                    observer.record_bytes(
                        "pi.stderr",
                        RawExecutionDirection::Stderr,
                        &buffer[..read],
                    );
                }
                kept = kept.saturating_add(read);
                if kept > max_bytes {
                    truncated.store(true, Ordering::Relaxed);
                }
            }
            Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
            Err(_) => {
                truncated.store(true, Ordering::Relaxed);
                return;
            }
        }
    }
}
