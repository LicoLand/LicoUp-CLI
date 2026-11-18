use super::*;

#[test]
fn protocol_reader_is_line_bounded_and_leaves_decoding_to_the_parser() {
    let (sender, receiver) = mpsc::channel();
    read_protocol_messages(Cursor::new(b"not-json\n"), Some(64), sender);
    assert!(matches!(
        receiver.recv().unwrap(),
        TransportEvent::Frame(line) if line == b"not-json"
    ));

    let (sender, receiver) = mpsc::channel();
    read_protocol_messages(Cursor::new(b"{\"id\":1}\n"), Some(4), sender);
    assert!(matches!(
        receiver.recv().unwrap(),
        TransportEvent::StdoutLimitExceeded
    ));
}

#[test]
fn stderr_is_fully_drained_without_retaining_bytes() {
    let truncated = Arc::new(AtomicBool::new(false));
    drain_stderr(Cursor::new(vec![b'x'; 128]), 16, &truncated);
    assert!(truncated.load(std::sync::atomic::Ordering::Relaxed));
}

#[test]
fn raw_execution_keeps_protocol_bytes_before_trimming_and_rejection() {
    use licoup_foundation::platform::raw_execution::{
        RawExecutionBinding, RawExecutionDirection, RawExecutionObserver, RawExecutionReader,
    };
    let records = Arc::new(std::sync::Mutex::new(Vec::new()));
    let captured = Arc::clone(&records);
    let observer = RawExecutionObserver::new(move |source, direction, text| {
        captured
            .lock()
            .unwrap()
            .push((source.to_owned(), direction, text.to_owned()));
        Ok(())
    });
    let binding = RawExecutionBinding::default();
    let _guard = binding.bind(Some(observer));
    let raw = b" { \"future\": {\"arguments\":\"unaltered\"} } \r\nnot-json\n";
    let (sender, receiver) = mpsc::channel();
    let reader = RawExecutionReader::new(
        Cursor::new(raw),
        binding.clone(),
        "openclaw",
        RawExecutionDirection::Received,
    );
    read_protocol_messages(std::io::BufReader::new(reader), None, sender);
    assert!(
        matches!(receiver.recv().unwrap(), TransportEvent::Frame(line) if line.ends_with(b"} "))
    );
    assert_eq!(
        records
            .lock()
            .unwrap()
            .iter()
            .map(|record| record.2.as_str())
            .collect::<String>(),
        String::from_utf8(raw.to_vec()).unwrap()
    );
    records.lock().unwrap().clear();
    let (sender, receiver) = mpsc::channel();
    let reader = RawExecutionReader::new(
        Cursor::new(b"oversized\r\n"),
        binding,
        "openclaw",
        RawExecutionDirection::Received,
    );
    read_protocol_messages(std::io::BufReader::new(reader), Some(2), sender);
    assert!(matches!(
        receiver.recv().unwrap(),
        TransportEvent::StdoutLimitExceeded
    ));
    assert_eq!(
        records.lock().unwrap()[0],
        (
            "openclaw".to_owned(),
            RawExecutionDirection::Received,
            "oversized\r\n".to_owned()
        )
    );
}
