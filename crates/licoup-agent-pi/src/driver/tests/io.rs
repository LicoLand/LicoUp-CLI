use super::*;
use licoup_foundation::platform::raw_execution::{RawExecutionObserver, RawExecutionScope};
use std::io::{Cursor, Read};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::mpsc;

struct CountingReader {
    inner: Cursor<Vec<u8>>,
    consumed: Arc<AtomicUsize>,
}

impl Read for CountingReader {
    fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
        let read = self.inner.read(buffer)?;
        self.consumed.fetch_add(read, Ordering::Relaxed);
        Ok(read)
    }
}

#[test]
fn stderr_is_fully_drained_after_the_truncation_bit_is_set() {
    let payload = vec![b'x'; 24 * 1024];
    let consumed = Arc::new(AtomicUsize::new(0));
    let truncated = Arc::new(AtomicBool::new(false));
    drain_stderr(
        CountingReader {
            inner: Cursor::new(payload.clone()),
            consumed: Arc::clone(&consumed),
        },
        16,
        &truncated,
    );
    assert!(truncated.load(Ordering::Relaxed));
    assert_eq!(consumed.load(Ordering::Relaxed), payload.len());
}

#[test]
fn stdout_jsonl_reader_enforces_the_aggregate_capacity() {
    let (sender, receiver) = mpsc::channel();
    read_protocol_messages(
        std::io::BufReader::new(Cursor::new(b"{\"type\":\"event\"}\n")),
        Some(4),
        sender,
    );
    assert!(matches!(
        receiver.recv().unwrap(),
        TransportEvent::StdoutLimitExceeded
    ));
}

#[test]
fn stdout_reader_leaves_jsonl_decoding_to_the_parser() {
    let (sender, receiver) = mpsc::channel();
    read_protocol_messages(
        std::io::BufReader::new(Cursor::new(b"not-json\n")),
        None,
        sender,
    );
    assert!(matches!(
        receiver.recv().unwrap(),
        TransportEvent::Line { line, .. } if line == "not-json\n"
    ));
}

#[test]
fn parser_accepts_lf_or_crlf_without_splitting_unicode_separators() {
    let separator = char::from_u32(0x2028).unwrap();
    assert_eq!(
        decode_jsonl_line(&format!("{{\"text\":\"left{separator}right\"}}\n"))
            .unwrap()
            .unwrap()["text"],
        format!("left{separator}right")
    );
    assert_eq!(
        decode_jsonl_line("{\"type\":\"event\"}\r\n")
            .unwrap()
            .unwrap()["type"],
        "event"
    );
    assert!(decode_jsonl_line("{not-json}\n").is_err());
}

#[test]
fn raw_stdout_retains_whole_over_limit_frame_and_invalid_utf8() {
    use std::sync::Mutex;
    let records = Arc::new(Mutex::new(Vec::new()));
    let sink = Arc::clone(&records);
    let observer = RawExecutionObserver::new(move |source, _, raw| {
        sink.lock()
            .unwrap()
            .push((source.to_owned(), raw.to_owned()));
        Ok(())
    });
    let _scope = RawExecutionScope::enter(Some(observer));
    let frame = b" {\"type\":\"tool_execution_end\",\"unknown\":[1,2]}\r\n{\"prefetched\":true}\n";
    let (sender, receiver) = mpsc::channel();
    read_protocol_messages(std::io::BufReader::new(Cursor::new(frame)), Some(4), sender);
    assert!(matches!(
        receiver.recv().unwrap(),
        TransportEvent::StdoutLimitExceeded
    ));
    assert_eq!(records.lock().unwrap()[0].1.as_bytes(), frame);
    let (sender, receiver) = mpsc::channel();
    read_protocol_messages(
        std::io::BufReader::new(Cursor::new(b"\xff\n")),
        None,
        sender,
    );
    assert!(matches!(
        receiver.recv().unwrap(),
        TransportEvent::StdoutReadFailed
    ));
    assert_eq!(
        records.lock().unwrap()[1],
        ("pi.stdout.base64".to_owned(), "/wo=".to_owned())
    );
}
