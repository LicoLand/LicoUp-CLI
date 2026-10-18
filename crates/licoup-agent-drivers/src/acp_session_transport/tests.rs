use super::capabilities::AcpSessionDriverSpec;
use super::continuity::{SessionKey, TransportKey};
use std::path::Path;

#[test]
fn transport_and_session_keys_are_scoped_by_driver_identity() {
    let first = AcpSessionDriverSpec::new("first-acp", &["acp"]);
    let second = AcpSessionDriverSpec::new("second-acp", &["acp"]);
    let cwd = Path::new("/workspace");

    assert_ne!(
        TransportKey::new(first, "agent", cwd),
        TransportKey::new(second, "agent", cwd)
    );
    assert_ne!(
        SessionKey::new(first, "native-session"),
        SessionKey::new(second, "native-session")
    );
}

#[test]
fn launch_arguments_are_immutable_adapter_metadata() {
    let driver = AcpSessionDriverSpec::new("vendor-acp", &["acp"]);
    assert_eq!(driver.driver_id, "vendor-acp");
    assert_eq!(driver.launch_args, &["acp"]);
}

#[test]
fn raw_protocol_capture_binds_when_received_and_clears_between_persistent_turns() {
    use super::events::{ConversationTransportEvent, read_conversation_frames};
    use licoup_foundation::platform::raw_execution::{
        RawExecutionBinding, RawExecutionDirection, RawExecutionObserver, RawExecutionReader,
    };
    use std::io::{self, BufRead, BufReader, Cursor, Read};
    use std::sync::{Arc, Mutex, mpsc};

    struct PauseBeforeBufferedTail<R> {
        reader: R,
        resume: mpsc::Receiver<()>,
        fills: usize,
    }

    impl<R: Read> Read for PauseBeforeBufferedTail<R> {
        fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
            self.reader.read(buffer)
        }
    }

    impl<R: BufRead> BufRead for PauseBeforeBufferedTail<R> {
        fn fill_buf(&mut self) -> io::Result<&[u8]> {
            if self.fills == 1 {
                self.resume.recv().unwrap();
            }
            self.fills += 1;
            self.reader.fill_buf()
        }

        fn consume(&mut self, count: usize) {
            self.reader.consume(count);
        }
    }

    let binding = RawExecutionBinding::default();
    let records = Arc::new(Mutex::new(Vec::new()));
    let observer = |turn: &'static str| {
        let sink = Arc::clone(&records);
        RawExecutionObserver::new(move |source, direction, text| {
            sink.lock()
                .unwrap()
                .push((turn, source.to_owned(), direction, text.to_owned()));
            Ok(())
        })
    };
    let first = " {\"result\":{\"stopReason\":\"end_turn\"}} \r\n{\"method\":\"unknown/tool\",\"arguments\":{\"text\":\"原样\"},\"result\":{\"extra\":true}}\n";
    let second = "{\"malformedSecondTurn\": [";
    let stderr = "synthetic stderr beyond ordinary limit";
    let (sender, receiver) = mpsc::channel();
    let (resume, paused) = mpsc::channel();
    let first_scope = binding.bind(Some(observer("first")));
    let reader_binding = binding.clone();
    let first_sender = sender.clone();
    let worker = std::thread::spawn(move || {
        let reader = RawExecutionReader::new(
            Cursor::new(first),
            reader_binding,
            "hermes-acp",
            RawExecutionDirection::Received,
        );
        read_conversation_frames(
            PauseBeforeBufferedTail {
                reader: BufReader::new(reader),
                resume: paused,
                fills: 0,
            },
            first_sender,
        );
    });
    assert!(matches!(
        receiver.recv().unwrap(),
        ConversationTransportEvent::Frame { .. }
    ));
    drop(first_scope);
    // An idle reader cannot use the previous turn's sink.
    binding.record_bytes(
        "hermes-acp",
        RawExecutionDirection::Received,
        b"idle frame\n",
    );
    {
        let _scope = binding.bind(Some(observer("second")));
        // The first physical read prefetched both frames. Releasing its buffered
        // tail after the next turn binds must neither lose nor reassign it.
        resume.send(()).unwrap();
        worker.join().unwrap();
        assert!(matches!(
            receiver.recv().unwrap(),
            ConversationTransportEvent::Frame { .. }
        ));
        let reader = RawExecutionReader::new(
            Cursor::new(second),
            binding.clone(),
            "hermes-acp",
            RawExecutionDirection::Received,
        );
        read_conversation_frames(BufReader::new(reader), sender);
        let truncated = std::sync::atomic::AtomicBool::new(false);
        super::io::drain_stderr_observed(Cursor::new(stderr), 1, &truncated, |bytes| {
            binding.record_bytes("hermes-acp", RawExecutionDirection::Stderr, bytes);
        });
        assert!(truncated.load(std::sync::atomic::Ordering::Relaxed));
    }
    assert_eq!(
        *records.lock().unwrap(),
        vec![
            (
                "first",
                "hermes-acp".to_owned(),
                RawExecutionDirection::Received,
                first.to_owned()
            ),
            (
                "second",
                "hermes-acp".to_owned(),
                RawExecutionDirection::Received,
                second.to_owned()
            ),
            (
                "second",
                "hermes-acp".to_owned(),
                RawExecutionDirection::Stderr,
                stderr.to_owned()
            ),
        ]
    );
}
