//! The claims this package makes about one Cursor turn with no host at all.
//!
//! These drive the transport reader, the PTY isolation, the parser binding, the
//! closed failure vocabulary and the launch validation directly, in a process
//! that answers none of the package's ports.
//!
//! The turn's process claims — the composed turn a fake Cursor drives and the
//! events it emits — live in `tests/driver.rs`, whose process answers the
//! turn-event port the way the client does; the pty it runs on is the shared
//! primitive both halves launch on.

use super as cursor_driver;
use super::io::{TransportEvent, read_protocol_messages};
use serde_json::json;
use std::io::Cursor;
use std::sync::{Arc, Mutex, mpsc};

#[test]
fn complete_json_tail_without_newline_is_delivered_before_close() {
    let input = br#"{"type":"result","subtype":"success","result":"ok"}"#;
    let (sender, receiver) = mpsc::channel();
    read_protocol_messages(Cursor::new(input), sender);
    let TransportEvent::Line(line) = receiver.recv().unwrap() else {
        panic!("complete JSON tail must be delivered as a protocol line");
    };
    assert!(serde_json::from_slice::<serde_json::Value>(&line).is_ok());
    assert!(matches!(
        receiver.recv().unwrap(),
        TransportEvent::StdoutClosed
    ));
}

#[test]
fn partial_json_tail_remains_an_unterminated_protocol_failure() {
    let (sender, receiver) = mpsc::channel();
    read_protocol_messages(Cursor::new(br#"{"type":"result""#), sender);
    assert!(matches!(
        receiver.recv().unwrap(),
        TransportEvent::UnterminatedLine
    ));
    assert!(matches!(
        receiver.recv().unwrap(),
        TransportEvent::StdoutClosed
    ));
}

#[test]
fn raw_execution_pipe_capture_precedes_pty_isolation_and_failed_tail() {
    use licoup_foundation::platform::raw_execution::{
        RawExecutionBinding, RawExecutionDirection, RawExecutionObserver, RawExecutionReader,
    };
    let records = Arc::new(Mutex::new(Vec::new()));
    let captured = Arc::clone(&records);
    let observer = RawExecutionObserver::new(move |_, _, text| {
        captured.lock().unwrap().push(text.to_owned());
        Ok(())
    });
    let binding = RawExecutionBinding::default();
    let _guard = binding.bind(Some(observer));
    let raw = b"\x1b[31m{\"future\":\"raw arguments\"}\x1b[0m\r\n{invalid";
    let (sender, receiver) = mpsc::channel();
    let reader = RawExecutionReader::new(
        Cursor::new(raw),
        binding,
        "cursor",
        RawExecutionDirection::Received,
    );
    read_protocol_messages(std::io::BufReader::new(reader), sender);
    assert!(
        matches!(receiver.recv().unwrap(), TransportEvent::Line(line) if line == b"{\"future\":\"raw arguments\"}\r\n")
    );
    assert!(matches!(
        receiver.recv().unwrap(),
        TransportEvent::UnterminatedLine
    ));
    assert_eq!(records.lock().unwrap().concat().as_bytes(), raw);
}

#[test]
fn canonical_protocol_is_cli_only() {
    assert_eq!(cursor_driver::RUNTIME_PROTOCOL, "cursor-agent-cli-v1");
    assert_eq!(cursor_driver::DRIVER_ID, "cursor-cli");
}

#[test]
fn capability_probe_requires_noninteractive_mcp_approval() {
    use crate::model::CapabilityProbe;

    let base = "create-chat --print --resume --output-format stream-json";
    assert!(!CapabilityProbe::official(true, true, base).supported);
    assert!(CapabilityProbe::official(true, true, &format!("{base} --approve-mcps")).supported);
}

#[test]
fn pty_controls_are_isolated_before_strict_ndjson_decoding() {
    use super::io::isolate_pty_protocol_line;
    use crate::model::EffectiveSettings;
    use crate::parser::{CursorParseFailure, CursorParser};

    let isolated = isolate_pty_protocol_line(
        b"\x1b[?25l{\"type\":\"result\",\"subtype\":\"success\",\"result\":\"ok\"}\x1b[0m\r\n",
    );
    let mut parser = CursorParser::new("synthetic-session", "prompt", EffectiveSettings::default());
    let acknowledged = isolate_pty_protocol_line(
        br#"{"type":"user","message":{"role":"user","content":[{"type":"text","text":"prompt"}]}}"#,
    );
    assert!(parser.parse_line(&acknowledged).is_ok());
    assert!(parser.parse_line(&isolated).is_ok());

    let prose = isolate_pty_protocol_line(b"diagnostic prose\r\n");
    assert!(matches!(
        parser.parse_line(&prose),
        Err(CursorParseFailure::InvalidJson)
    ));
    assert!(
        isolate_pty_protocol_line(b"\x1b[?25l\x1b[0m\r\n")
            .iter()
            .all(|byte| byte.is_ascii_whitespace())
    );
}

#[test]
fn stream_identity_must_equal_the_bound_session_before_any_effect() {
    use crate::model::EffectiveSettings;
    use crate::parser::{CursorEffect, CursorParseFailure, CursorParser};

    // Frames without an explicit identity stay bound to the launched session.
    let mut parser = CursorParser::new(
        "bound-session",
        "exact prompt",
        EffectiveSettings::default(),
    );
    let initialized = parser
        .parse_line(br#"{"type":"system","subtype":"init","cwd":"/tmp","model":"fake-model"}"#)
        .unwrap();
    assert!(initialized.is_empty());
    let accepted = parser
        .parse_line(br#"{"type":"user","message":{"role":"user","content":[{"type":"text","text":"exact prompt"}]}}"#)
        .unwrap();
    assert!(accepted.iter().any(|effect| matches!(
        effect,
        CursorEffect::Accepted { session_id, .. } if session_id == "bound-session"
    )));

    // A repeated exact identity stays stable across every frame.
    let exact = parser
        .parse_line(br#"{"type":"assistant","session_id":"bound-session","message":{"role":"assistant","content":[{"type":"text","text":"ok"}]}}"#)
        .unwrap();
    assert!(exact.iter().any(|effect| matches!(
        effect,
        CursorEffect::Text { session_id, text, .. }
            if session_id == "bound-session" && text == "ok"
    )));

    // A changed identity is rejected before the frame exposes any accepted,
    // chunk, or terminal effect.
    let drifted = parser.parse_line(
        br#"{"type":"assistant","session_id":"drifted-session","message":{"role":"assistant","content":[{"type":"text","text":"wrong"}]}}"#,
    );
    assert!(matches!(drifted, Err(CursorParseFailure::IdentityMismatch)));
}

#[test]
fn private_instructions_fail_before_process_launch() {
    let result = cursor_driver::execute(
        "definitely-not-a-real-cursor-agent",
        &json!({"privateInstructions": "synthetic private instruction"}),
        "exact user prompt",
        "",
        Some(std::env::temp_dir().as_path()),
        0,
        None,
        1024,
    );
    assert_eq!(
        result.error.as_ref().map(|failure| failure.code),
        Some("cursor_cli_private_instructions_unsupported")
    );
}

#[test]
fn stderr_classifier_uses_only_the_closed_cursor_failure_vocabulary() {
    use crate::errors::CursorFailureKind;

    let cases = [
        (
            "Please log in before continuing: private account detail",
            CursorFailureKind::AuthenticationRequired,
        ),
        (
            "Quota exceeded for private account detail",
            CursorFailureKind::UsageLimitExceeded,
        ),
        (
            "Too many requests for private account detail",
            CursorFailureKind::RateLimited,
        ),
        (
            "Selected model is not available: private model detail",
            CursorFailureKind::ModelUnavailable,
        ),
    ];
    for (stderr, expected) in cases {
        assert_eq!(CursorFailureKind::from_stderr(stderr), Some(expected));
        let failure = expected.failure(Some("synthetic-session"));
        assert!(!format!("{failure:?}").contains("private"));
    }
    assert_eq!(
        CursorFailureKind::from_stderr("arbitrary private vendor prose"),
        None
    );
}
