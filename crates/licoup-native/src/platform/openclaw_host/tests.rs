//! The host's own answers for the OpenClaw adapter package, under test.
//!
//! The package owns what one OpenClaw turn emits; [`super`] owns where those
//! events go. This suite states the host's half: every event a fresh OpenClaw
//! session produces, as the client's installed answer reports it, carries the
//! identity a stream reader requires — the stdio RPC conversation server drops
//! a whole turn to `stream_protocol_failed` when any single event lacks a
//! non-empty `sessionId`, `turnId` or event kind, so the answer, not only the
//! producer, is what this claim rests on.

use std::path::PathBuf;
use std::process::Command;
use std::sync::{Arc, Mutex};

use serde_json::{Value, json};

/// The fake bridge the client's answer is observed against.
const FAKE_OPENCLAW_SOURCE: &str = include_str!("../../../tests/fixtures/fake_openclaw_acp.rs");

fn compile_fake_openclaw(prefix: &str) -> (PathBuf, PathBuf) {
    let nonce = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let directory = std::env::temp_dir().join(format!("{prefix}-{nonce}"));
    std::fs::create_dir_all(&directory).unwrap();
    let source = directory.join("fake_openclaw.rs");
    let executable = directory.join(format!("fake-openclaw{}", std::env::consts::EXE_SUFFIX));
    std::fs::write(&source, FAKE_OPENCLAW_SOURCE).unwrap();
    let status = Command::new("rustc")
        .args(["--edition", "2021"])
        .arg(&source)
        .arg("-o")
        .arg(&executable)
        .status()
        .unwrap();
    assert!(status.success());
    (directory, executable)
}

#[test]
fn every_event_a_fresh_openclaw_session_emits_is_stream_writable() {
    let captured = Arc::new(Mutex::new(Vec::<Value>::new()));
    let sink_target = Arc::clone(&captured);
    // The host's answers, exactly as composition installs them: the package is
    // observed through the port this host really answers.
    let _ = licoup_agent_openclaw::port::turn_event::install(super::turn_event_port());
    crate::platform::turn_event_emit::install_stream_sink(Box::new(move |event| {
        sink_target.lock().unwrap().push(event);
    }));
    let _sink = crate::platform::turn_event_emit::StreamSinkGuard;

    let (directory, executable) = compile_fake_openclaw("lico-openclaw-host-port");
    let result = licoup_agent_openclaw::driver::execute_with_connection(
        executable.to_string_lossy().as_ref(),
        None,
        || Ok(Vec::new()),
        &json!({"gatewayWsUrl": "ws://127.0.0.1:9"}),
        "private-openclaw-prompt",
        "",
        Some(directory.as_path()),
        10_000,
        Some(128 * 1024),
        8 * 1024,
    );
    assert!(result.ok, "OpenClaw fake failure: {:?}", result.error);
    let events = captured.lock().unwrap().clone();
    assert!(!events.is_empty());
    for event in &events {
        let session_id = event
            .get("sessionId")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let turn_id = event
            .get("turnId")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let kind = event
            .get("event")
            .and_then(Value::as_str)
            .unwrap_or_default();
        assert!(!session_id.is_empty(), "event missing sessionId: {event}");
        assert!(!turn_id.is_empty(), "event missing turnId: {event}");
        assert!(!kind.is_empty(), "event missing kind: {event}");
    }
    let _ = std::fs::remove_dir_all(directory);
}
