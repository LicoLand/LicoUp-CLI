use super::super::events::TransportEvent;
use super::super::stdio_transport::ProtocolLoopTransport;
use super::*;
use std::collections::VecDeque;
use std::io;
use std::sync::mpsc::RecvTimeoutError;
use std::time::{Duration, Instant};

pub fn absolute_test_cwd() -> PathBuf {
    std::env::current_dir().expect("test working directory")
}

pub fn initialize_response(load: bool, resume: bool) -> Value {
    json!({
        "jsonrpc": "2.0",
        "id": INITIALIZE_REQUEST_ID,
        "result": {
            "protocolVersion": acp::PROTOCOL_VERSION,
            "agentCapabilities": {
                "loadSession": load,
                "sessionCapabilities": if resume { json!({"resume": {}}) } else { json!({}) },
                "promptCapabilities": {"image": true}
            }
        }
    })
}

pub fn new_protocol(params: Value, prompt: &str, session: &str) -> AcpProtocol {
    AcpProtocol::new(
        ProtocolConfig::from_params(
            &params,
            prompt,
            session,
            Some(absolute_test_cwd().as_path()),
        )
        .unwrap(),
        "copilot-acp",
    )
}

pub struct ScriptedProtocolLoopTransport {
    now: Instant,
    events: VecDeque<(Instant, TransportEvent)>,
    writes: Vec<Value>,
}

impl ScriptedProtocolLoopTransport {
    pub fn messages(start: Instant, messages: Vec<(Duration, Value)>) -> Self {
        let events = messages
            .into_iter()
            .map(|(after_start, message)| {
                (
                    start + after_start,
                    TransportEvent::Frame(serde_json::to_vec(&message).unwrap()),
                )
            })
            .collect();
        Self {
            now: start,
            events,
            writes: Vec::new(),
        }
    }

    pub fn now(&self) -> Instant {
        self.now
    }

    pub fn remaining_events(&self) -> usize {
        self.events.len()
    }

    pub fn remaining_messages(&self) -> Vec<Value> {
        self.events
            .iter()
            .filter_map(|(_, event)| match event {
                TransportEvent::Frame(line) => serde_json::from_slice(line).ok(),
                _ => None,
            })
            .collect()
    }

    pub fn writes(&self) -> &[Value] {
        &self.writes
    }
}

impl ProtocolLoopTransport for ScriptedProtocolLoopTransport {
    fn check_health(&mut self) -> io::Result<()> {
        Ok(())
    }

    fn write(&mut self, message: &Value) -> io::Result<()> {
        self.writes.push(message.clone());
        Ok(())
    }

    fn recv_timeout(&mut self, timeout: Duration) -> Result<TransportEvent, RecvTimeoutError> {
        let wait_deadline = self.now + timeout;
        if self
            .events
            .front()
            .is_some_and(|(event_at, _)| *event_at <= wait_deadline)
        {
            let (event_at, event) = self.events.pop_front().expect("scripted event");
            self.now = event_at;
            return Ok(event);
        }
        self.now = wait_deadline;
        Err(RecvTimeoutError::Timeout)
    }

    fn now(&self) -> Instant {
        self.now
    }
}

pub const FAKE_AGENT_SOURCE: &str = r#"
use std::io::{self, BufRead, Write};
fn id(line: &str) -> i64 {
let marker = "\"id\":";
let start = line.find(marker).unwrap() + marker.len();
line[start..].chars().take_while(|c| c.is_ascii_digit()).collect::<String>().parse().unwrap()
}
fn park_forever() -> ! {
loop { std::thread::park(); }
}
fn main() {
let args = std::env::args().skip(1).collect::<Vec<_>>();
assert_eq!(args, vec!["acp"]);
std::thread::spawn(|| { let mut e=io::stderr(); let _=e.write_all(&vec![b'x'; 128*1024]); });
let stdin = io::stdin();
let mut lines = stdin.lock().lines();
let first = lines.next().unwrap().unwrap();
assert!(first.contains("\"method\":\"initialize\""));
println!("{{\"jsonrpc\":\"2.0\",\"id\":{},\"result\":{{\"protocolVersion\":1,\"agentCapabilities\":{{\"loadSession\":true}}}}}}", id(&first));
io::stdout().flush().unwrap();
let second = lines.next().unwrap().unwrap();
assert!(second.contains("\"method\":\"session/new\""));
println!("{{\"jsonrpc\":\"2.0\",\"id\":{},\"result\":{{\"sessionId\":\"native-fake-session\",\"configOptions\":[]}}}}", id(&second));
io::stdout().flush().unwrap();
let third = lines.next().unwrap().unwrap();
assert!(third.contains("private-stdin-prompt"));
assert!(third.contains("\"method\":\"session/prompt\""));
if third.contains("SELFTEST_HARD_DEADLINE") { park_forever(); }
if third.contains("SELFTEST_FLOOD") {
for i in 0..70 {
println!("{{\"jsonrpc\":\"2.0\",\"method\":\"session/update\",\"params\":{{\"sessionId\":\"native-fake-session\",\"update\":{{\"sessionUpdate\":\"agent_message_chunk\",\"content\":{{\"type\":\"text\",\"text\":\"chunk-{}\"}}}}}}}}", i);
}
println!("{{\"jsonrpc\":\"2.0\",\"id\":{},\"result\":{{\"stopReason\":\"end_turn\"}}}}", id(&third));
io::stdout().flush().unwrap();
park_forever();
}
if third.contains("SELFTEST_PROCESS_LOSS") { return; }
if third.contains("SELFTEST_OUTPUT_LIMIT") {
let text = "x".repeat(65536);
println!("{{\"jsonrpc\":\"2.0\",\"method\":\"session/update\",\"params\":{{\"sessionId\":\"native-fake-session\",\"update\":{{\"sessionUpdate\":\"agent_message_chunk\",\"content\":{{\"type\":\"text\",\"text\":\"{}\"}}}}}}}}", text);
io::stdout().flush().unwrap();
park_forever();
}
if third.contains("SELFTEST_EMPTY_OUTPUT") {
println!("{{\"jsonrpc\":\"2.0\",\"id\":{},\"result\":{{\"stopReason\":\"end_turn\"}}}}", id(&third));
io::stdout().flush().unwrap();
park_forever();
}
println!("{{\"jsonrpc\":\"2.0\",\"method\":\"session/update\",\"params\":{{\"sessionId\":\"native-fake-session\",\"update\":{{\"sessionUpdate\":\"agent_message_chunk\",\"content\":{{\"type\":\"text\",\"text\":\"fake \"}}}}}}}}");
io::stdout().flush().unwrap();
println!("{{\"jsonrpc\":\"2.0\",\"method\":\"session/update\",\"params\":{{\"sessionId\":\"native-fake-session\",\"update\":{{\"sessionUpdate\":\"agent_message_chunk\",\"content\":{{\"type\":\"text\",\"text\":\"final\"}}}}}}}}");
io::stdout().flush().unwrap();
println!("{{\"jsonrpc\":\"2.0\",\"id\":{},\"result\":{{\"stopReason\":\"end_turn\"}}}}", id(&third));
io::stdout().flush().unwrap();
park_forever();
}
"#;
