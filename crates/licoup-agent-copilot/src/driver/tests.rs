//! The claims this package's own driver makes about a running Copilot.
//!
//! The declaration claims live in [`crate::tests`]; these drive the two bounded
//! entry points against a fake Copilot the test compiles from source, so the
//! canonical entry point, the native session binding, the pre-binding session
//! updates real Copilot 1.0.46 sends and the active-turn cancel are established
//! on the process the package actually starts rather than on a description of
//! it.

use super::*;
use serde_json::json;
use std::fs;
use std::process::Command;
use std::sync::mpsc;
use std::time::{SystemTime, UNIX_EPOCH};

enum CancelTurnProgress {
    Bound,
    Finished,
}

#[test]
fn fake_child_transport_proves_native_session_and_final_message() {
    let dir = std::env::temp_dir().join(format!("lico-copilot-acp-fake-{}", timestamp()));
    fs::create_dir_all(&dir).unwrap();
    let source = dir.join("fake_agent.rs");
    let executable = dir.join(format!("fake-agent{}", std::env::consts::EXE_SUFFIX));
    fs::write(&source, FAKE_AGENT_SOURCE).unwrap();
    let status = Command::new("rustc")
        .args(["--edition", "2021"])
        .arg(&source)
        .arg("-o")
        .arg(&executable)
        .status()
        .unwrap();
    assert!(status.success());
    let result = execute(
        executable.to_string_lossy().as_ref(),
        &json!({}),
        "private-stdin-prompt",
        "",
        Some(dir.as_path()),
        10_000,
        Some(1024 * 1024),
        1024,
    );
    assert!(result.ok, "fake Copilot ACP failure: {:?}", result.error);
    assert_eq!(result.output, "copilot final");
    assert_eq!(result.session_id, "copilot-native-session");
    assert_eq!(result.turn_status, "end_turn");
    assert_eq!(result.runtime_protocol, RUNTIME_PROTOCOL);
    assert_eq!(result.driver_id, "copilot-acp");
    assert!(result.stderr_truncated);
    let _ = fs::remove_dir_all(dir);
}

#[test]
fn pre_binding_session_updates_from_real_copilot_do_not_kill_the_turn() {
    let dir = std::env::temp_dir().join(format!("lico-copilot-prebind-fake-{}", timestamp()));
    fs::create_dir_all(&dir).unwrap();
    let source = dir.join("fake_prebind_agent.rs");
    let executable = dir.join(format!(
        "fake-prebind-agent{}",
        std::env::consts::EXE_SUFFIX
    ));
    fs::write(&source, FAKE_PREBIND_AGENT_SOURCE).unwrap();
    let status = Command::new("rustc")
        .args(["--edition", "2021"])
        .arg(&source)
        .arg("-o")
        .arg(&executable)
        .status()
        .unwrap();
    assert!(status.success());
    let result = execute(
        executable.to_string_lossy().as_ref(),
        &json!({}),
        "private-stdin-prompt",
        "",
        Some(dir.as_path()),
        10_000,
        Some(1024 * 1024),
        1024,
    );
    assert!(result.ok, "pre-bind Copilot failure: {:?}", result.error);
    assert_eq!(result.output, "copilot prebind final");
    assert_eq!(result.session_id, "copilot-prebind-session");
    assert_eq!(result.turn_status, "end_turn");
    let _ = fs::remove_dir_all(dir);
}

#[test]
fn capability_probe_uses_the_same_canonical_acp_entrypoint() {
    let dir = std::env::temp_dir().join(format!("lico-copilot-probe-fake-{}", timestamp()));
    fs::create_dir_all(&dir).unwrap();
    let source = dir.join("fake_probe.rs");
    let executable = dir.join(format!("fake-probe{}", std::env::consts::EXE_SUFFIX));
    fs::write(&source, FAKE_PROBE_SOURCE).unwrap();
    let status = Command::new("rustc")
        .args(["--edition", "2021"])
        .arg(&source)
        .arg("-o")
        .arg(&executable)
        .status()
        .unwrap();
    assert!(status.success());
    let probe = capability_probe(
        executable.to_string_lossy().as_ref(),
        dir.as_path(),
        5_000,
        Some(64 * 1024),
        1024,
    )
    .unwrap();
    assert_eq!(probe.protocol_version, Some(1));
    assert!(probe.load_session);
    assert!(!probe.resume_session);
    let _ = fs::remove_dir_all(dir);
}

#[test]
fn active_acp_turn_accepts_cancel_before_exact_session_resume() {
    let dir = std::env::temp_dir().join(format!("lico-copilot-cancel-fake-{}", timestamp()));
    fs::create_dir_all(&dir).unwrap();
    let source = dir.join("fake_cancel_agent.rs");
    let executable = dir.join(format!("fake-cancel-agent{}", std::env::consts::EXE_SUFFIX));
    fs::write(&source, FAKE_CANCEL_AGENT_SOURCE).unwrap();
    let status = Command::new("rustc")
        .args(["--edition", "2021"])
        .arg(&source)
        .arg("-o")
        .arg(&executable)
        .status()
        .unwrap();
    assert!(status.success());

    let (progress_sender, progress_receiver) = mpsc::channel();
    let bound_sender = progress_sender.clone();
    let run_dir = dir.clone();
    let run_executable = executable.clone();
    let run = std::thread::spawn(move || {
        licoup_foundation::platform::turn_event_emit::install_stream_sink(Box::new(move |event| {
            if event.get("event").and_then(Value::as_str) == Some("dispatch.turn.bound") {
                let _ = bound_sender.send(CancelTurnProgress::Bound);
            }
        }));
        let _sink = licoup_foundation::platform::turn_event_emit::StreamSinkGuard;
        let result = execute(
            run_executable.to_string_lossy().as_ref(),
            &json!({}),
            "cancel-me",
            "",
            Some(run_dir.as_path()),
            10_000,
            Some(1024 * 1024),
            1024,
        );
        let _ = progress_sender.send(CancelTurnProgress::Finished);
        result
    });

    let mut bound = false;
    let mut cancellation = None;
    while let Ok(progress) = progress_receiver.recv() {
        match progress {
            CancelTurnProgress::Bound if !bound => {
                bound = true;
                cancellation = Some(cancel("copilot-cancel-session"));
            }
            CancelTurnProgress::Bound => {}
            CancelTurnProgress::Finished => break,
        }
    }
    let result = run.join().unwrap();
    assert!(
        bound,
        "Copilot turn finished before its exact session bound"
    );
    assert_eq!(
        cancellation,
        Some(licoup_agent_drivers::acp_driver_runtime::ControlDisposition::Accepted)
    );
    assert!(!result.ok);
    assert_eq!(result.session_id, "copilot-cancel-session");
    assert_eq!(result.turn_status, "cancelled");
    let _ = fs::remove_dir_all(dir);
}

fn timestamp() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos()
}

const FAKE_AGENT_SOURCE: &str = r#"
use std::io::{self, BufRead, Write};
fn id(line: &str) -> i64 {
    let marker = "\"id\":";
    let start = line.find(marker).unwrap() + marker.len();
    line[start..].chars().take_while(|c| c.is_ascii_digit()).collect::<String>().parse().unwrap()
}
fn main() {
    let args = std::env::args().skip(1).collect::<Vec<_>>();
    assert_eq!(args, vec!["--acp", "--stdio", "--no-auto-update"]);
    std::thread::spawn(|| { let mut e=io::stderr(); let _=e.write_all(&vec![b'x'; 128*1024]); });
    let stdin = io::stdin();
    let mut lines = stdin.lock().lines();
    let first = lines.next().unwrap().unwrap();
    assert!(first.contains("\"method\":\"initialize\""));
    println!("{{\"jsonrpc\":\"2.0\",\"id\":{},\"result\":{{\"protocolVersion\":1,\"agentCapabilities\":{{\"loadSession\":true}}}}}}", id(&first));
    io::stdout().flush().unwrap();
    let second = lines.next().unwrap().unwrap();
    assert!(second.contains("\"method\":\"session/new\""));
    println!("{{\"jsonrpc\":\"2.0\",\"id\":{},\"result\":{{\"sessionId\":\"copilot-native-session\",\"configOptions\":[]}}}}", id(&second));
    io::stdout().flush().unwrap();
    let third = lines.next().unwrap().unwrap();
    assert!(third.contains("private-stdin-prompt"));
    assert!(third.contains("\"method\":\"session/prompt\""));
    println!("{{\"jsonrpc\":\"2.0\",\"method\":\"session/update\",\"params\":{{\"sessionId\":\"copilot-native-session\",\"update\":{{\"sessionUpdate\":\"agent_message_chunk\",\"content\":{{\"type\":\"text\",\"text\":\"copilot final\"}}}}}}}}");
    println!("{{\"jsonrpc\":\"2.0\",\"id\":{},\"result\":{{\"stopReason\":\"end_turn\"}}}}", id(&third));
    io::stdout().flush().unwrap();
    std::thread::sleep(std::time::Duration::from_secs(1));
}
"#;

const FAKE_PREBIND_AGENT_SOURCE: &str = r#"
use std::io::{self, BufRead, Write};
fn id(line: &str) -> i64 {
    let marker = "\"id\":";
    let start = line.find(marker).unwrap() + marker.len();
    line[start..].chars().take_while(|c| c.is_ascii_digit()).collect::<String>().parse().unwrap()
}
fn main() {
    let args = std::env::args().skip(1).collect::<Vec<_>>();
    assert_eq!(args, vec!["--acp", "--stdio", "--no-auto-update"]);
    let stdin = io::stdin();
    let mut lines = stdin.lock().lines();
    let first = lines.next().unwrap().unwrap();
    assert!(first.contains("\"method\":\"initialize\""));
    println!("{{\"jsonrpc\":\"2.0\",\"id\":{},\"result\":{{\"protocolVersion\":1,\"agentCapabilities\":{{\"loadSession\":true}},\"agentInfo\":{{\"name\":\"Copilot\",\"version\":\"1.0.46\"}},\"authMethods\":[]}}}}", id(&first));
    io::stdout().flush().unwrap();
    let second = lines.next().unwrap().unwrap();
    assert!(second.contains("\"method\":\"session/new\""));
    // Real Copilot 1.0.46 announces session updates for the conversation that
    // is still being created, before the session/new response arrives.
    for _ in 0..2 {
        println!("{{\"jsonrpc\":\"2.0\",\"method\":\"session/update\",\"params\":{{\"sessionId\":\"copilot-prebind-session\",\"update\":{{\"sessionUpdate\":\"available_commands_update\",\"availableCommands\":[{{\"name\":\"compact\",\"description\":\"Summarize conversation history\"}}]}}}}}}");
    }
    io::stdout().flush().unwrap();
    println!("{{\"jsonrpc\":\"2.0\",\"id\":{},\"result\":{{\"sessionId\":\"copilot-prebind-session\",\"models\":{{\"availableModels\":[],\"currentModelId\":\"gpt-5-mini\"}},\"modes\":{{\"availableModes\":[],\"currentModeId\":\"agent\"}},\"configOptions\":[]}}}}", id(&second));
    io::stdout().flush().unwrap();
    let third = lines.next().unwrap().unwrap();
    assert!(third.contains("private-stdin-prompt"));
    assert!(third.contains("\"method\":\"session/prompt\""));
    println!("{{\"jsonrpc\":\"2.0\",\"method\":\"session/update\",\"params\":{{\"sessionId\":\"copilot-prebind-session\",\"update\":{{\"sessionUpdate\":\"agent_message_chunk\",\"content\":{{\"type\":\"text\",\"text\":\"copilot prebind final\"}}}}}}}}");
    println!("{{\"jsonrpc\":\"2.0\",\"id\":{},\"result\":{{\"stopReason\":\"end_turn\"}}}}", id(&third));
    io::stdout().flush().unwrap();
    std::thread::sleep(std::time::Duration::from_secs(1));
}
"#;

const FAKE_PROBE_SOURCE: &str = r#"
use std::io::{self, BufRead, Write};
fn main() {
    let args = std::env::args().skip(1).collect::<Vec<_>>();
    assert_eq!(args, vec!["--acp", "--stdio", "--no-auto-update"]);
    let stdin = io::stdin();
    let first = stdin.lock().lines().next().unwrap().unwrap();
    assert!(first.contains("\"method\":\"initialize\""));
    println!("{{\"jsonrpc\":\"2.0\",\"id\":1,\"result\":{{\"protocolVersion\":1,\"agentCapabilities\":{{\"loadSession\":true}}}}}}");
    io::stdout().flush().unwrap();
    std::thread::sleep(std::time::Duration::from_secs(1));
}
"#;

const FAKE_CANCEL_AGENT_SOURCE: &str = r#"
use std::io::{self, BufRead, Write};
fn id(line: &str) -> i64 {
    let marker = "\"id\":";
    let start = line.find(marker).unwrap() + marker.len();
    line[start..].chars().take_while(|c| c.is_ascii_digit()).collect::<String>().parse().unwrap()
}
fn main() {
    let stdin = io::stdin();
    let mut lines = stdin.lock().lines();
    let initialize = lines.next().unwrap().unwrap();
    println!("{{\"jsonrpc\":\"2.0\",\"id\":{},\"result\":{{\"protocolVersion\":1,\"agentCapabilities\":{{\"loadSession\":true}}}}}}", id(&initialize));
    io::stdout().flush().unwrap();
    let session = lines.next().unwrap().unwrap();
    println!("{{\"jsonrpc\":\"2.0\",\"id\":{},\"result\":{{\"sessionId\":\"copilot-cancel-session\",\"configOptions\":[]}}}}", id(&session));
    io::stdout().flush().unwrap();
    let prompt = lines.next().unwrap().unwrap();
    assert!(prompt.contains("\"method\":\"session/prompt\""));
    let cancel = lines.next().unwrap().unwrap();
    assert!(cancel.contains("\"method\":\"session/cancel\""));
    assert!(cancel.contains("copilot-cancel-session"));
    println!("{{\"jsonrpc\":\"2.0\",\"id\":{},\"result\":{{\"stopReason\":\"cancelled\"}}}}", id(&prompt));
    io::stdout().flush().unwrap();
    std::thread::sleep(std::time::Duration::from_secs(1));
}
"#;
