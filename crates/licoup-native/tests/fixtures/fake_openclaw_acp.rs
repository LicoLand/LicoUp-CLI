// A minimal fake OpenClaw ACP bridge for the host's port-answer suite.
//
// It speaks the same Gateway ACP lane the real bridge does — initialize, one
// session, one prompt, one streamed chunk — and nothing else, because the
// claim it exists for is about the events an OpenClaw turn emits, not about
// the package's own framing, which the package's suite drives in full with its
// own fixture. It is a host fixture for the same reason: the answer under test
// is the client's, and the client's suite owns its own copy of the program it
// starts.
//
// It is read by `crates/licoup-native/src/platform/openclaw_host/tests.rs`.
use std::io::{self, BufRead, Write};

fn main() {
    let args = std::env::args().skip(1).collect::<Vec<_>>();
    assert_eq!(args, ["acp", "--url", "ws://127.0.0.1:9"]);
    let stdin = io::stdin();
    let mut lines = stdin.lock().lines();

    let initialize = lines.next().unwrap().unwrap();
    assert!(initialize.contains("\"method\":\"initialize\""));
    assert!(!initialize.contains("private-openclaw-prompt"));
    println!(
        r#"{{"jsonrpc":"2.0","id":1,"result":{{"protocolVersion":1,"agentCapabilities":{{"loadSession":true}},"agentInfo":{{"name":"openclaw-acp","version":"test"}}}}}}"#
    );
    io::stdout().flush().unwrap();

    let session = lines.next().unwrap().unwrap();
    assert!(session.contains("\"method\":\"session/new\""));
    assert!(!session.contains("private-openclaw-prompt"));
    // The Gateway conversation key arrives on the opening update, before the
    // session response: it is the identity every later event must carry.
    println!(
        r#"{{"jsonrpc":"2.0","method":"session/update","params":{{"sessionId":"protocol-session","update":{{"sessionUpdate":"session_info_update","_meta":{{"sessionKey":"agent:main:acp:host-suite-session"}}}}}}}}"#
    );
    println!(r#"{{"jsonrpc":"2.0","id":2,"result":{{"sessionId":"protocol-session"}}}}"#);
    io::stdout().flush().unwrap();

    let prompt = lines.next().unwrap().unwrap();
    assert!(prompt.contains("\"method\":\"session/prompt\""));
    assert!(prompt.contains("private-openclaw-prompt"));
    assert!(prompt.contains("\"sessionId\":\"protocol-session\""));
    let hidden_metadata = ["must", "not", "project"].join("-");
    println!(
        r#"{{"jsonrpc":"2.0","method":"session/update","params":{{"sessionId":"protocol-session","update":{{"sessionUpdate":"agent_message_chunk","content":{{"type":"text","text":"host answer"}},"_meta":{{"secret":"{hidden_metadata}"}}}}}}}}"#
    );
    println!(r#"{{"jsonrpc":"2.0","id":4,"result":{{"stopReason":"end_turn"}}}}"#);
    io::stdout().flush().unwrap();
}
