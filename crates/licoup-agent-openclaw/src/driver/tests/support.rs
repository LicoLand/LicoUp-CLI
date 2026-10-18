use super::*;

pub(super) fn absolute_test_cwd() -> PathBuf {
    std::env::current_dir().expect("test working directory")
}

pub(super) fn config(params: Value, prompt: &str, session_id: &str) -> ProtocolConfig {
    // Protocol fixtures do not exercise installation-backed MCP registration.
    ProtocolConfig::from_params_without_local_mcp(
        &params,
        prompt,
        session_id,
        Some(absolute_test_cwd().as_path()),
    )
    .unwrap()
}

pub(super) fn sent_messages(effects: Vec<ProtocolEffect>) -> Vec<Value> {
    effects
        .into_iter()
        .filter_map(|effect| match effect {
            ProtocolEffect::Send(message) => Some(message),
            ProtocolEffect::Complete(_) | ProtocolEffect::Fail(_) => None,
        })
        .collect()
}

pub(super) fn initialize(protocol: &mut OpenClawProtocol) -> Vec<ProtocolEffect> {
    protocol.handle_message(json!({
        "jsonrpc": "2.0",
        "id": INITIALIZE_REQUEST_ID,
        "result": {
            "protocolVersion": acp::PROTOCOL_VERSION,
            "agentCapabilities": {"loadSession": true, "sessionCapabilities": {"resume": {}}},
            "agentInfo": {"name": "openclaw-acp", "version": "test"}
        }
    }))
}

pub(super) fn validated_update(update: Value) -> acp::AcpSessionUpdate {
    acp::validate_session_update(
        &json!({
            "jsonrpc": "2.0",
            "method": "session/update",
            "params": {"sessionId": "protocol-session", "update": update}
        }),
        Some("protocol-session"),
    )
    .unwrap()
}

pub(super) fn temporary_directory(prefix: &str) -> PathBuf {
    let nonce = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let path = std::env::temp_dir().join(format!("{prefix}-{nonce}"));
    fs::create_dir_all(&path).unwrap();
    path
}

pub(super) fn compile_fake_openclaw(prefix: &str) -> (PathBuf, PathBuf) {
    compile_fake_openclaw_source(prefix, FAKE_OPENCLAW_SOURCE)
}

pub(super) fn compile_fake_openclaw_source(prefix: &str, source_text: &str) -> (PathBuf, PathBuf) {
    let directory = temporary_directory(prefix);
    let source = directory.join("fake_openclaw.rs");
    let executable = directory.join(format!("fake-openclaw{}", std::env::consts::EXE_SUFFIX));
    fs::write(&source, source_text).unwrap();
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

pub(super) const FAKE_OPENCLAW_SOURCE: &str = r###"
use std::io::{self, BufRead, Write};

fn main() {
    let args = std::env::args().skip(1).collect::<Vec<_>>();
    if args == ["acp", "--help"] {
        println!("Run an ACP bridge backed by the Gateway");
        return;
    }
    if args == ["--version"] {
        println!("OpenClaw test-version");
        return;
    }
    assert_eq!(args.len(), 3);
    assert_eq!(&args[..2], ["acp", "--url"]);
    assert_eq!(args[2], "ws://127.0.0.1:9");
    assert!(!args.iter().any(|arg| arg.contains("private") || arg.contains("session-key")));

    std::thread::spawn(|| {
        let mut stderr = io::stderr();
        let _ = stderr.write_all(&vec![b'x'; 128 * 1024]);
    });
    let stdin = io::stdin();
    let mut lines = stdin.lock().lines();

    let initialize = lines.next().unwrap().unwrap();
    assert!(initialize.contains("\"method\":\"initialize\""));
    assert!(!initialize.contains("private-openclaw-prompt"));
    println!(r#"{{"jsonrpc":"2.0","id":1,"result":{{"protocolVersion":1,"agentCapabilities":{{"loadSession":true}},"agentInfo":{{"name":"openclaw-acp","version":"test"}}}}}}"#);
    io::stdout().flush().unwrap();

    let session = lines.next().unwrap().unwrap();
    assert!(session.contains("\"method\":\"session/new\""));
    assert!(!session.contains("private-openclaw-prompt"));
    println!(r#"{{"jsonrpc":"2.0","method":"session/update","params":{{"sessionId":"protocol-session","update":{{"sessionUpdate":"session_info_update","_meta":{{"sessionKey":"agent:main:acp:native-session"}}}}}}}}"#);
    println!(r#"{{"jsonrpc":"2.0","id":2,"result":{{"sessionId":"protocol-session","modes":{{"currentModeId":"medium","availableModes":[]}}}}}}"#);
    io::stdout().flush().unwrap();

    let mode = lines.next().unwrap().unwrap();
    assert!(mode.contains("\"method\":\"session/set_mode\""));
    assert!(mode.contains("\"sessionId\":\"protocol-session\""));
    println!(r#"{{"jsonrpc":"2.0","id":3,"result":{{}}}}"#);
    io::stdout().flush().unwrap();

    let prompt = lines.next().unwrap().unwrap();
    assert!(prompt.contains("private-openclaw-prompt"));
    assert!(prompt.contains("\"sessionId\":\"protocol-session\""));
    let hidden_metadata = ["must", "not", "project"].join("-");
    println!(r#"{{"jsonrpc":"2.0","method":"session/update","params":{{"sessionId":"protocol-session","update":{{"sessionUpdate":"agent_message_chunk","content":{{"type":"text","text":"native "}},"_meta":{{"secret":"{hidden_metadata}"}}}}}}}}"#);
    println!(r#"{{"jsonrpc":"2.0","method":"session/update","params":{{"sessionId":"protocol-session","update":{{"sessionUpdate":"agent_message_chunk","content":{{"type":"text","text":"answer"}}}}}}}}"#);
    println!(r#"{{"jsonrpc":"2.0","id":4,"result":{{"stopReason":"end_turn"}}}}"#);
    io::stdout().flush().unwrap();
}
"###;
