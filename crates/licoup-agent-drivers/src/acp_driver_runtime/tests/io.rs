use super::*;
use std::io::{BufReader, Cursor};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::thread;
use std::time::Duration;

#[test]
fn raw_protocol_capture_preserves_unknown_fields_and_malformed_frames() {
    use licoup_foundation::platform::raw_execution::{
        RawExecutionBinding, RawExecutionDirection, RawExecutionObserver, RawExecutionReader,
        RawExecutionScope,
    };
    use std::sync::{Arc, Mutex};

    let records = Arc::new(Mutex::new(Vec::new()));
    let sink = Arc::clone(&records);
    let observer = RawExecutionObserver::new(move |source, direction, text| {
        sink.lock()
            .unwrap()
            .push((source.to_owned(), direction, text.to_owned()));
        Ok(())
    });
    let _scope = RawExecutionScope::enter(Some(observer));
    let first = " {\"method\":\"unknown/tool\", \"params\":{\"arguments\":\"完整参数\",\"result\":{\"future\":[1,true]}}} \r\n";
    let malformed = "{\"unknown\": [ broken JSON";
    let (sender, receiver) = mpsc::sync_channel(ACP_EVENT_CHANNEL_CAPACITY);
    let binding = RawExecutionBinding::default();
    let _binding_scope = binding.bind_current();
    let reader = RawExecutionReader::new(
        Cursor::new(format!("{first}{malformed}")),
        binding,
        "acp",
        RawExecutionDirection::Received,
    );
    read_protocol_messages(std::io::BufReader::new(reader), None, sender);

    assert_eq!(
        *records.lock().unwrap(),
        vec![(
            "acp".to_owned(),
            RawExecutionDirection::Received,
            format!("{first}{malformed}")
        ),]
    );
    assert!(matches!(
        receiver.recv().unwrap(),
        super::super::events::TransportEvent::Frame(_)
    ));
    assert!(matches!(
        receiver.recv().unwrap(),
        super::super::events::TransportEvent::Frame(_)
    ));
}

#[test]
fn stderr_is_drained_to_a_truncation_bit_without_content_projection() {
    let truncated = AtomicBool::new(false);
    drain_stderr(Cursor::new(b"private-stderr-canary"), 4, &truncated);
    assert!(truncated.load(Ordering::Relaxed));
}

#[test]
fn stdout_reader_enforces_the_configured_aggregate_limit() {
    let (sender, receiver) = mpsc::sync_channel(ACP_EVENT_CHANNEL_CAPACITY);
    read_protocol_messages(Cursor::new(b"{\"id\":1}\n{\"id\":2}\n"), Some(8), sender);
    assert!(matches!(
        receiver.recv().unwrap(),
        super::super::events::TransportEvent::StdoutLimitExceeded
    ));
}

#[test]
fn stdout_reader_rejects_an_oversized_no_newline_frame_before_unbounded_buffering() {
    let (sender, receiver) = mpsc::sync_channel(ACP_EVENT_CHANNEL_CAPACITY);
    let mut frame = b"{\"id\":1,\"text\":\"".to_vec();
    frame.extend(std::iter::repeat(b'x').take(acp::MAX_JSON_LINE_BYTES + 1024));
    frame.extend(b"\"}");
    read_protocol_messages(Cursor::new(frame), None, sender);
    assert!(matches!(
        receiver.recv().unwrap(),
        super::super::events::TransportEvent::StdoutLimitExceeded
    ));
}

#[test]
fn stdout_reader_reconstructs_fragmented_and_exact_boundary_frames() {
    let (sender, receiver) = mpsc::sync_channel(ACP_EVENT_CHANNEL_CAPACITY);
    let mut input = Vec::new();

    // A frame whose newline lands exactly on the BufReader fill boundary.
    let prefix = b"{\"id\":7,\"text\":\"";
    let suffix = b"\"}\n";
    let exact_pad = 8192 - prefix.len() - suffix.len();
    let mut exact_frame = prefix.to_vec();
    exact_frame.extend(std::iter::repeat(b'x').take(exact_pad));
    exact_frame.extend_from_slice(suffix);
    assert_eq!(exact_frame.len(), 8192);
    input.extend_from_slice(&exact_frame);

    // A frame split across several fills right after the boundary.
    let prefix = b"{\"id\":8,\"text\":\"";
    let suffix = b"\"}\n";
    let fragmented_pad = 20_000 - prefix.len() - suffix.len();
    let mut fragmented_frame = prefix.to_vec();
    fragmented_frame.extend(std::iter::repeat(b'y').take(fragmented_pad));
    fragmented_frame.extend_from_slice(suffix);
    input.extend_from_slice(&fragmented_frame);

    read_protocol_messages(BufReader::new(Cursor::new(input)), None, sender);
    let first: Value = match receiver.recv().unwrap() {
        super::super::events::TransportEvent::Frame(line) => serde_json::from_slice(&line).unwrap(),
        other => panic!("expected the exact-boundary frame, got {other:?}"),
    };
    assert_eq!(first["id"], 7);
    assert_eq!(first["text"].as_str().unwrap().len(), exact_pad);
    let second: Value = match receiver.recv().unwrap() {
        super::super::events::TransportEvent::Frame(line) => serde_json::from_slice(&line).unwrap(),
        other => panic!("expected the fragmented frame, got {other:?}"),
    };
    assert_eq!(second["id"], 8);
    assert_eq!(second["text"].as_str().unwrap().len(), fragmented_pad);
}

#[test]
fn stdout_reader_blocks_at_capacity_and_delivers_every_message_in_order() {
    const LINE_COUNT: usize = 65;
    let mut input = Vec::new();
    for id in 0..LINE_COUNT {
        input.extend_from_slice(format!("{{\"id\":{id}}}\n").as_bytes());
    }
    let (sender, receiver) = mpsc::sync_channel(ACP_EVENT_CHANNEL_CAPACITY);
    let reader = thread::spawn(move || read_protocol_messages(Cursor::new(input), None, sender));
    for id in 0..LINE_COUNT {
        let message = match receiver.recv_timeout(Duration::from_secs(2)) {
            Ok(super::super::events::TransportEvent::Frame(line)) => {
                serde_json::from_slice::<Value>(&line).unwrap()
            }
            other => panic!("expected message {id} in order, got {other:?}"),
        };
        assert_eq!(message["id"], id);
    }
    assert!(matches!(
        receiver.recv_timeout(Duration::from_secs(2)),
        Ok(super::super::events::TransportEvent::StdoutClosed)
    ));
    reader.join().unwrap();
}

#[test]
fn a_full_event_queue_blocks_the_reader_until_the_receiver_is_dropped() {
    const LINE_COUNT: usize = 65;
    let mut input = Vec::new();
    for id in 0..LINE_COUNT {
        input.extend_from_slice(format!("{{\"id\":{id}}}\n").as_bytes());
    }
    let (sender, receiver) = mpsc::sync_channel(ACP_EVENT_CHANNEL_CAPACITY);
    let reader = thread::spawn(move || read_protocol_messages(Cursor::new(input), None, sender));
    // The bounded queue is full, so the reader must wait instead of dropping.
    thread::sleep(Duration::from_millis(100));
    assert!(
        !reader.is_finished(),
        "reader must block at queue capacity until the consumer drains"
    );
    drop(receiver);
    reader.join().unwrap();
}
