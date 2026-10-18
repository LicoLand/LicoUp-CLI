//! What this host's consumer receives from one Pi execution.
//!
//! The Pi driver — the `pi --mode rpc` process, the frames it classifies and the
//! outcome it reports — is the Pi adapter package's, and every claim about the
//! driver itself moved beside it. What the host keeps is *where* the events of
//! one Pi turn go, because the host owns the consumer. Each suite below installs
//! this host's answer for the package's turn-event port, drives the package's
//! own execution or parser, and observes what reached the consumer; the
//! package's port is fail-closed without that answer, so the observation cannot
//! be made from the package's own suite.

use super::super::super::native_agent_interaction;
use super::super::super::turn_event_emit::{self, StreamSinkGuard};
use crate::platform::pi_turn_event_port;
use licoup_agent_pi::driver::params::ProtocolConfig;
use licoup_agent_pi::driver::{ControlDisposition, execute, steer};
use licoup_agent_pi::parser::{PiProtocol, ProtocolEffect};
use serde_json::{Value, json};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;

/// The fake `pi --mode rpc --offline` program these suites run.
///
/// It answers the two turns the host's consumer claims are about: a streamed
/// reply whose chunks carry the session Pi reported, and a turn that stays open
/// until the native `steer` frame reaches the child.
const FAKE_PI_SOURCE: &str = r#"
use std::io::{self, BufRead, Write};

fn request_id(line: &str) -> &str {
    let marker = "\"id\":\"";
    let start = line.find(marker).unwrap() + marker.len();
    let tail = &line[start..];
    &tail[..tail.find('"').unwrap()]
}

fn main() {
    let args = std::env::args().skip(1).collect::<Vec<_>>();
    assert_eq!(args, vec!["--mode", "rpc", "--offline"]);
    let fixture = std::env::current_dir()
        .ok()
        .and_then(|path| path.file_name().map(|name| name.to_string_lossy().into_owned()))
        .unwrap_or_default();
    let session_id = if fixture.contains("steer") {
        "pi-native-steer-1"
    } else {
        "pi-native-stream-1"
    };
    let mut awaiting_steer = false;
    let mut guided = false;
    let mut streamed = false;
    let mut stdout = io::stdout();
    for line in io::stdin().lock().lines() {
        let line = line.unwrap();
        let id = request_id(&line);
        if line.contains("\"type\":\"get_state\"") {
            println!("{{\"id\":\"{id}\",\"type\":\"response\",\"command\":\"get_state\",\"success\":true,\"data\":{{\"sessionId\":\"{session_id}\"}}}}");
            stdout.flush().unwrap();
        } else if line.contains("\"type\":\"prompt\"") {
            println!("{{\"id\":\"{id}\",\"type\":\"response\",\"command\":\"prompt\",\"success\":true}}");
            if line.contains("steer-case") {
                awaiting_steer = true;
            } else {
                streamed = true;
                println!("{{\"type\":\"message_update\",\"assistantMessageEvent\":{{\"type\":\"text_delta\",\"delta\":\"one\"}}}}");
                println!("{{\"type\":\"message_update\",\"assistantMessageEvent\":{{\"type\":\"text_delta\",\"delta\":\"-two\"}}}}");
                println!("{{\"type\":\"agent_settled\"}}");
            }
            stdout.flush().unwrap();
        } else if line.contains("\"type\":\"steer\"") {
            if !awaiting_steer || !line.contains("pi-native-steer-guidance") {
                std::process::exit(4);
            }
            awaiting_steer = false;
            guided = true;
            println!("{{\"id\":\"{id}\",\"type\":\"response\",\"command\":\"steer\",\"success\":true}}");
            println!("{{\"type\":\"message_update\",\"assistantMessageEvent\":{{\"type\":\"text_delta\",\"delta\":\"pi-guided\"}}}}");
            println!("{{\"type\":\"agent_settled\"}}");
            stdout.flush().unwrap();
        } else if line.contains("\"type\":\"get_last_assistant_text\"") {
            let text = if guided {
                "pi-guided"
            } else if streamed {
                "one-two"
            } else {
                "pi-ok"
            };
            println!("{{\"id\":\"{id}\",\"type\":\"response\",\"command\":\"get_last_assistant_text\",\"success\":true,\"data\":{{\"text\":\"{text}\"}}}}");
            stdout.flush().unwrap();
        }
    }
}
"#;

/// Install this host's answer for the Pi adapter package's turn-event port.
///
/// The package owns what one Pi turn emits; this host owns where it goes. A
/// suite that captures those events must install the host's answer, or every
/// event the package produces would be silently dropped and the capture would
/// only ever see the host's own emissions. Installation is process-wide and
/// first-wins, so a later call is a no-op.
fn install_pi_turn_event_port() {
    let _ = licoup_agent_pi::port::turn_event::install(pi_turn_event_port());
}

fn compile_fake_pi(prefix: &str) -> (PathBuf, PathBuf) {
    let nonce = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let directory = std::env::temp_dir().join(format!("{prefix}-{nonce}"));
    fs::create_dir_all(&directory).unwrap();
    let source = directory.join("fake_pi.rs");
    let executable = directory.join(format!("fake-pi{}", std::env::consts::EXE_SUFFIX));
    fs::write(&source, FAKE_PI_SOURCE).unwrap();
    let status = std::process::Command::new("rustc")
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
fn the_consumer_receives_each_streamed_pi_chunk_with_its_bound_session() {
    install_pi_turn_event_port();
    let (directory, executable) = compile_fake_pi("lico-pi-host-stream");
    let captured = std::sync::Arc::new(std::sync::Mutex::new(Vec::<Value>::new()));
    let target = std::sync::Arc::clone(&captured);
    turn_event_emit::install_stream_sink(Box::new(move |event| {
        target.lock().unwrap().push(event);
    }));
    let _guard = StreamSinkGuard;
    let result = execute(
        executable.to_string_lossy().as_ref(),
        &json!({}),
        "stream-case",
        "",
        Some(directory.as_path()),
        10_000,
        Some(1024 * 1024),
        1024,
    );
    assert!(result.ok, "pi rpc failure: {:?}", result.error);
    let events = captured.lock().unwrap();
    let chunks = events
        .iter()
        .filter(|event| event["event"] == "agent.message.chunk")
        .collect::<Vec<_>>();
    assert_eq!(chunks.len(), 2);
    assert!(
        chunks
            .iter()
            .all(|event| event["sessionId"] == "pi-native-stream-1")
    );
    assert_eq!(chunks[0]["payload"]["text"], "one");
    assert_eq!(chunks[1]["payload"]["text"], "-two");
    drop(events);
    let _ = fs::remove_dir_all(directory);
}

#[test]
fn the_consumer_receives_the_bound_pi_turn_before_native_guidance() {
    install_pi_turn_event_port();
    let (directory, executable) = compile_fake_pi("lico-pi-host-steer");
    let (binding_sender, binding_receiver) = std::sync::mpsc::sync_channel(1);
    let working_directory = directory.clone();
    let executable = executable.to_string_lossy().to_string();
    let run = std::thread::spawn(move || {
        turn_event_emit::install_stream_sink(Box::new(move |event| {
            if event["event"] == "dispatch.turn.bound" {
                let _ = binding_sender.try_send((
                    event["sessionId"].as_str().unwrap_or_default().to_string(),
                    event["turnId"].as_str().unwrap_or_default().to_string(),
                ));
            }
        }));
        let _guard = StreamSinkGuard;
        execute(
            &executable,
            &json!({}),
            "steer-case",
            "",
            Some(working_directory.as_path()),
            10_000,
            Some(1024 * 1024),
            1024,
        )
    });
    let (session_id, turn_id) = binding_receiver
        .recv_timeout(Duration::from_secs(5))
        .expect("Pi should publish its exact active turn binding");
    assert_eq!(session_id, "pi-native-steer-1");
    assert_eq!(
        steer(&session_id, &turn_id, "pi-native-steer-guidance"),
        ControlDisposition::Accepted
    );
    let result = run.join().unwrap();
    assert!(result.ok, "Pi RPC failure: {:?}", result.error);
    assert_eq!(result.output, "pi-guided");
    assert_eq!(result.session_id, session_id);
    assert_eq!(result.turn_id, turn_id);
    let _ = fs::remove_dir_all(directory);
}

#[test]
fn the_consumer_receives_one_parked_pi_interaction_and_resolves_it_once() {
    install_pi_turn_event_port();
    let captured = std::sync::Arc::new(std::sync::Mutex::new(Vec::<Value>::new()));
    let target = std::sync::Arc::clone(&captured);
    turn_event_emit::install_stream_sink(Box::new(move |event| {
        target.lock().unwrap().push(event);
    }));
    let _guard = StreamSinkGuard;
    let config = ProtocolConfig::from_params(
        &json!({}),
        "hello",
        "",
        Some(Path::new("/workspace/project")),
    )
    .unwrap();
    let mut protocol = PiProtocol::new(config);
    protocol.session_id = Some("synthetic-session".to_string());
    let effects = protocol.handle_message(json!({
        "type": "extension_ui_request",
        "id": "ui-1",
        "method": "confirm",
        "title": "Synthetic confirmation"
    }));
    let ProtocolEffect::Interact(interaction) = effects.into_iter().next().unwrap() else {
        panic!("dialog request must park");
    };
    assert_eq!(interaction.exact_request()["id"], "ui-1");
    assert_eq!(interaction.exact_request()["method"], "confirm");
    let events = captured.lock().unwrap();
    assert_eq!(events.len(), 1);
    assert_eq!(events[0]["event"], "agent.interaction.needed");
    assert_eq!(events[0]["sessionId"], "synthetic-session");
    assert_eq!(events[0]["turnId"], protocol.config.turn_id);
    assert_eq!(events[0]["payload"]["agentId"], "pi");
    assert_eq!(
        events[0]["payload"]["adapterCallbackTokenRef"],
        interaction.callback_token()
    );
    drop(events);
    native_agent_interaction::resolve_scoped(
        interaction.callback_token(),
        Some("synthetic-session"),
        Some(&protocol.config.turn_id),
        json!({"confirmed": true}),
    )
    .unwrap();
    assert_eq!(
        interaction.response(&protocol).unwrap(),
        json!({
            "type": "extension_ui_response",
            "id": "ui-1",
            "confirmed": true,
        })
    );
}
