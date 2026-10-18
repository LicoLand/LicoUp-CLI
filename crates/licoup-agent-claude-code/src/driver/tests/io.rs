use super::*;

#[test]
fn protocol_lines_and_probe_output_are_bounded() {
    let (sender, receiver) = mpsc::channel();
    read_protocol_messages(
        BufReader::new(Cursor::new(vec![b'x'; MAX_PROTOCOL_LINE_BYTES + 1])),
        sender,
    );
    assert!(matches!(
        receiver.recv().unwrap(),
        TransportEvent::LineLimitExceeded
    ));
    assert!(read_bounded(Cursor::new(vec![b'x'; 2048]), 1024));
}

#[test]
fn stderr_is_drained_without_retaining_or_projecting_bytes() {
    let truncated = Arc::new(AtomicBool::new(false));
    drain_stderr(
        Cursor::new(vec![b'x'; 2048]),
        128,
        &truncated,
        &licoup_foundation::platform::raw_execution::RawExecutionBinding::default(),
    );
    assert!(truncated.load(Ordering::Relaxed));
}

#[test]
fn raw_execution_keeps_prefetched_frames_with_receiving_turn() {
    use licoup_foundation::platform::raw_execution::{
        RawExecutionBinding, RawExecutionDirection, RawExecutionObserver, RawExecutionReader,
    };
    use std::io::BufRead;
    let binding = RawExecutionBinding::default();
    let first = Arc::new(std::sync::Mutex::new(Vec::new()));
    let first_sink = Arc::clone(&first);
    let first_observer = RawExecutionObserver::new(move |_, _, text| {
        first_sink.lock().unwrap().push(text.to_owned());
        Ok(())
    });
    let second = Arc::new(std::sync::Mutex::new(Vec::new()));
    let second_sink = Arc::clone(&second);
    let second_observer = RawExecutionObserver::new(move |_, _, text| {
        second_sink.lock().unwrap().push(text.to_owned());
        Ok(())
    });
    let mut reader = {
        let _guard = binding.bind(Some(first_observer));
        let worker_binding = binding.clone();
        thread::spawn(move || {
            let input = RawExecutionReader::new(
                Cursor::new(b" {\"unknown\":true} \r\nnot-json\n"),
                worker_binding,
                "claude-code",
                RawExecutionDirection::Received,
            );
            let mut reader = BufReader::new(input);
            reader.read_until(b'\n', &mut Vec::new()).unwrap();
            reader
        })
        .join()
        .unwrap()
    };
    let _guard = binding.bind(Some(second_observer));
    let mut second_line = Vec::new();
    reader.read_until(b'\n', &mut second_line).unwrap();
    assert_eq!(second_line, b"not-json\n");
    assert_eq!(
        first.lock().unwrap().concat(),
        " {\"unknown\":true} \r\nnot-json\n"
    );
    assert!(second.lock().unwrap().is_empty());
    let (sender, _receiver) = mpsc::channel();
    let reader = RawExecutionReader::new(
        Cursor::new(b"second invalid frame\n"),
        binding.clone(),
        "claude-code",
        RawExecutionDirection::Received,
    );
    read_protocol_messages(BufReader::new(reader), sender);
    assert_eq!(second.lock().unwrap().concat(), "second invalid frame\n");
}
