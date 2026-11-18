use crate::app_server::driver::io::{TransportEvent, drain_stderr, read_protocol_messages};
use std::io::Cursor;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;

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
    let first = " {\"method\":\"future/tool\", \"params\":{\"arguments\":\"完整参数\",\"output\":{\"newField\":42}}} \r\n";
    let malformed = "{\"malformed\": true";
    let (sender, receiver) = mpsc::channel();
    let binding = RawExecutionBinding::default();
    let _binding_scope = binding.bind_current();
    let reader = RawExecutionReader::new(
        Cursor::new(format!("{first}{malformed}")),
        binding,
        "codex-app-server",
        RawExecutionDirection::Received,
    );
    read_protocol_messages(std::io::BufReader::new(reader), None, sender);

    assert_eq!(
        *records.lock().unwrap(),
        vec![(
            "codex-app-server".to_owned(),
            RawExecutionDirection::Received,
            format!("{first}{malformed}")
        ),]
    );
    assert!(matches!(receiver.recv().unwrap(), TransportEvent::Line(_)));
    assert!(matches!(receiver.recv().unwrap(), TransportEvent::Line(_)));
}

#[test]
fn stdout_reader_is_line_framed_and_enforces_total_limit() {
    let input = b"{\"id\":1,\"result\":{}}\n{\"method\":\"initialized\"}\n";
    let (sender, receiver) = mpsc::channel();
    read_protocol_messages(Cursor::new(input), Some(input.len()), sender);
    assert!(matches!(receiver.recv().unwrap(), TransportEvent::Line(_)));
    assert!(matches!(receiver.recv().unwrap(), TransportEvent::Line(_)));
    assert!(matches!(
        receiver.recv().unwrap(),
        TransportEvent::StdoutClosed
    ));

    let (sender, receiver) = mpsc::channel();
    read_protocol_messages(Cursor::new(input), Some(input.len() - 1), sender);
    assert!(matches!(receiver.recv().unwrap(), TransportEvent::Line(_)));
    assert!(matches!(
        receiver.recv().unwrap(),
        TransportEvent::StdoutLimitExceeded
    ));
}

#[test]
fn stderr_drain_retains_no_content_and_marks_truncation() {
    let truncated = AtomicBool::new(false);
    drain_stderr(Cursor::new(vec![b'x'; 64 * 1024]), 1024, &truncated);
    assert!(truncated.load(Ordering::Relaxed));
}
