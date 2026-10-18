//! The Codex package's own program: the native entry its manifest declares.
//!
//! This is the process an extension host starts. It links no client crate and
//! reads no client state: it answers from the package's own protocol facts and
//! runs the app-server this package owns.
//!
//! # What it serves
//!
//! One JSON document per line on stdin, one JSON document per line on stdout —
//! the published C09 line protocol, which is what the extension host's
//! `IsolatedProcessCarrier` speaks:
//!
//! - `extension.initialize` negotiates the host protocol and the profile set,
//!   and publishes `extension.ready` once the profile is answered.
//! - `agent.describe` reports this package's identity, the adapter it carries,
//!   and the optional abilities it really has.
//! - `agent.execute` admits one Codex turn and reports *admission*: the app-server
//!   runs on a worker thread, the answer is the receipt, and the end of the work
//!   is the terminal `agent.event` that follows it.
//! - `agent.cancel` forwards an interrupt to the live turn through the same
//!   control registry the driver binds.
//! - `extension.shutdown` answers and ends the process.
//!
//! `--describe` answers the package description once and exits, which is what the
//! release tooling and a package inspection use.
//!
//! # The turn this program runs
//!
//! The execution request names the installed Codex client and the turn's facts;
//! everything else about the turn is the package's own. The program is the
//! launcher the extension host resolved — it adds no second launch path, no
//! daemon and no side channel. It reaches no network of its own, and the only
//! child process it starts is the app-server the request named.

use std::io::{self, BufRead, Write};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::thread;

use licoup_agent_codex::app_server::contract::PROTOCOL_FORMAT;
use licoup_agent_codex::app_server::driver::{self, active_control};
use licoup_agent_codex::port::turn_event::{self, TurnEventPort};
use licoup_agent_codex::registration::{ADAPTER_ID, CONTRACT, FRAMING};
use serde_json::{Value, json};

const PACKAGE_ID: &str = "org.licoland.adapter.codex";
const PACKAGE_VERSION: &str = env!("CARGO_PKG_VERSION");
const HOST_PROTOCOL_MAJOR: u64 = 1;
const HOST_PROTOCOL_MINIMUM_MINOR: u64 = 0;
const AGENT_EXECUTION_PROFILE: &str = "agent-execution";
const AGENT_EXECUTION_CAPABILITY: &str = "agent-execution.v1";
const METHOD_NOT_FOUND: i64 = -32601;
const INVALID_PARAMS: i64 = -32602;

/// The thread the live turn is bound to, as the parser reported it.
///
/// The driver's control registry is keyed by the app-server's own thread id, so
/// a cancellation needs the id the current turn is running under. The package
/// learns it from its own turn events — the same events the host reads — rather
/// than by re-deriving it from the protocol.
static ACTIVE_THREAD: OnceLock<Mutex<Option<String>>> = OnceLock::new();

fn active_thread() -> &'static Mutex<Option<String>> {
    ACTIVE_THREAD.get_or_init(|| Mutex::new(None))
}

fn record_turn_event(kind: &str, session_id: &str, _turn_id: &str, _payload: Value) {
    if kind == "agent.turn.accepted" && !session_id.is_empty() {
        *active_thread()
            .lock()
            .unwrap_or_else(|poison| poison.into_inner()) = Some(session_id.to_owned());
    }
}

/// The stdout half of the wire, shared with the worker thread that settles a
/// turn.
struct Wire {
    out: Mutex<io::Stdout>,
    sequence: AtomicU64,
}

impl Wire {
    fn send(&self, frame: Value) {
        let mut out = self.out.lock().unwrap_or_else(|poison| poison.into_inner());
        if serde_json::to_writer(&mut *out, &frame).is_ok() {
            let _ = out.write_all(b"\n");
            let _ = out.flush();
        }
    }

    fn result(&self, id: &Value, result: Value) {
        self.send(json!({"jsonrpc": "2.0", "id": id, "result": result}));
    }

    fn error(&self, id: &Value, code: i64, message: &str) {
        self.send(json!({
            "jsonrpc": "2.0",
            "id": id,
            "error": {"code": code, "message": message},
        }));
    }

    fn notify(&self, method: &str, params: Value) {
        self.send(json!({"jsonrpc": "2.0", "method": method, "params": params}));
    }

    /// One terminal event for an admitted invocation.
    fn settle(&self, invocation_ref: &str, body: Value) {
        let sequence = self.sequence.fetch_add(1, Ordering::SeqCst) + 1;
        self.notify(
            "agent.event",
            json!({
                "invocationRef": invocation_ref,
                "sequence": sequence,
                "kind": "terminal",
                "body": body,
            }),
        );
    }
}

fn describe() -> Value {
    json!({
        "packageId": PACKAGE_ID,
        "packageVersion": PACKAGE_VERSION,
        "hostProtocol": {
            "major": HOST_PROTOCOL_MAJOR,
            "minimumMinor": HOST_PROTOCOL_MINIMUM_MINOR,
        },
        "adapterId": ADAPTER_ID,
        "framing": FRAMING,
        "format": PROTOCOL_FORMAT,
        "contract": CONTRACT.inventory_json(),
    })
}

/// This package's answer to `agent.describe`.
///
/// It is the package's own description of the one Agent it carries: the adapter
/// id, the input it accepts, its one capability, and the optional abilities it
/// really has. Cancellation is supported because a live app-server turn takes an
/// interrupt; resume is not, because an app-server turn is settled by its own
/// terminal event and this program keeps no record to resume from.
fn agent_description() -> Value {
    json!({
        "id": ADAPTER_ID,
        "instanceKind": "executable",
        "inputKinds": ["text", "json"],
        "capabilities": [AGENT_EXECUTION_CAPABILITY],
        "interfaceVersion": PACKAGE_VERSION,
        "usage": "unavailable",
        "cancel": "supported",
        "resume": "unsupported",
    })
}

/// One turn as the execution request describes it.
#[derive(Debug)]
struct CodexRun {
    executable: String,
    params: Value,
    prompt: String,
    session_id: String,
    cwd: Option<PathBuf>,
    timeout_ms: u64,
    max_stdout: Option<usize>,
    max_stderr: usize,
}

impl CodexRun {
    /// Read one execution request.
    ///
    /// The host's published call shape carries the input as a string, so a
    /// structured request arrives as the JSON text of an object and a bare
    /// prompt arrives as itself. Both are ordinary answers: a bare prompt has no
    /// executable to start and is refused rather than guessed at.
    fn from_input(input: &Value) -> Result<Self, String> {
        let owned;
        let object = match input {
            Value::Object(map) => map,
            Value::String(text) => {
                owned = serde_json::from_str::<Value>(text)
                    .ok()
                    .filter(Value::is_object)
                    .unwrap_or_else(|| json!({ "prompt": text }));
                owned.as_object().expect("an object was constructed")
            }
            _ => return Err("codex_package_input_unsupported".to_owned()),
        };
        let executable = object
            .get("executable")
            .and_then(Value::as_str)
            .filter(|value| !value.is_empty())
            .ok_or_else(|| "codex_package_executable_missing".to_owned())?;
        let prompt = object
            .get("prompt")
            .and_then(Value::as_str)
            .ok_or_else(|| "codex_package_prompt_missing".to_owned())?;
        Ok(Self {
            executable: executable.to_owned(),
            params: object.get("params").cloned().unwrap_or_else(|| json!({})),
            prompt: prompt.to_owned(),
            session_id: object
                .get("sessionId")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_owned(),
            cwd: object
                .get("cwd")
                .and_then(Value::as_str)
                .filter(|value| !value.is_empty())
                .map(PathBuf::from),
            timeout_ms: object
                .get("timeoutMs")
                .and_then(Value::as_u64)
                .unwrap_or_default(),
            max_stdout: object
                .get("maxStdout")
                .and_then(Value::as_u64)
                .map(|value| value as usize),
            max_stderr: object
                .get("maxStderr")
                .and_then(Value::as_u64)
                .map(|value| value as usize)
                .unwrap_or(64 * 1024),
        })
    }
}

/// Run one admitted turn and settle it with its own terminal event.
///
/// Admission is never completion: the receipt was already written, and this is
/// the separate fact that says what the turn produced.
fn run_turn(wire: Arc<Wire>, invocation_ref: String, run: CodexRun) {
    let result = driver::execute(
        &run.executable,
        &run.params,
        &run.prompt,
        &run.session_id,
        run.cwd.as_deref(),
        run.timeout_ms,
        run.max_stdout,
        run.max_stderr,
        None,
    );
    let body = match result.error {
        Some(failure) if !result.ok => json!({
            "outcome": "failed",
            "code": failure.code,
            "stage": failure.stage,
            "message": failure.message,
            "threadId": failure.thread_id,
            "turnId": failure.turn_id,
            "turnStatus": failure.turn_status,
        }),
        _ => json!({
            "outcome": "succeeded",
            "text": result.output,
            "sessionId": result.session_id,
            "threadId": result.thread_id,
            "turnId": result.turn_id,
            "turnStatus": result.turn_status,
        }),
    };
    wire.settle(&invocation_ref, body);
    *active_thread()
        .lock()
        .unwrap_or_else(|poison| poison.into_inner()) = None;
}

/// Answer one request. Returns `true` when the process should stop.
fn answer(wire: &Arc<Wire>, request: &Value) -> bool {
    let id = request.get("id").cloned().unwrap_or(Value::Null);
    match request.get("method").and_then(Value::as_str) {
        Some("extension.initialize") => {
            let max_frame_bytes = request
                .get("params")
                .and_then(|params| params.get("maxFrameBytes"))
                .cloned()
                .unwrap_or_else(|| json!(1024 * 1024));
            wire.result(
                &id,
                json!({
                    "protocol": {
                        "major": HOST_PROTOCOL_MAJOR,
                        "minimumMinor": HOST_PROTOCOL_MINIMUM_MINOR,
                    },
                    "maxFrameBytes": max_frame_bytes,
                    "profiles": [AGENT_EXECUTION_PROFILE],
                }),
            );
            wire.notify(
                "extension.ready",
                json!({"profiles": [AGENT_EXECUTION_PROFILE]}),
            );
            false
        }
        Some("agent.describe") => {
            wire.result(&id, agent_description());
            false
        }
        Some("agent.execute") => {
            let params = request.get("params").cloned().unwrap_or(Value::Null);
            let invocation_ref = params
                .get("invocationRef")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_owned();
            if invocation_ref.is_empty() {
                wire.error(&id, INVALID_PARAMS, "codex_package_invocation_missing");
                return false;
            }
            let input = params.get("input").cloned().unwrap_or(Value::Null);
            match CodexRun::from_input(&input) {
                Ok(run) => {
                    wire.result(
                        &id,
                        json!({"invocationRef": invocation_ref, "outcome": "accepted"}),
                    );
                    let wire = Arc::clone(wire);
                    thread::spawn(move || run_turn(wire, invocation_ref, run));
                }
                Err(code) => wire.error(&id, INVALID_PARAMS, &code),
            }
            false
        }
        Some("agent.cancel") => {
            let thread_id = active_thread()
                .lock()
                .unwrap_or_else(|poison| poison.into_inner())
                .clone();
            let outcome = match thread_id {
                Some(thread_id) => match active_control::interrupt(&thread_id) {
                    active_control::ControlDisposition::Accepted => "requested",
                    active_control::ControlDisposition::NoActiveTurn => "unknown",
                    active_control::ControlDisposition::SessionUnavailable
                    | active_control::ControlDisposition::TransportUnavailable => "unsupported",
                },
                None => "unsupported",
            };
            wire.result(&id, json!({"outcome": outcome}));
            false
        }
        Some("extension.shutdown") => {
            wire.result(&id, json!({"outcome": "stopped"}));
            true
        }
        Some(_) => {
            wire.error(&id, METHOD_NOT_FOUND, "codex_package_method_unsupported");
            false
        }
        None => {
            wire.error(&id, INVALID_PARAMS, "codex_package_request_invalid");
            false
        }
    }
}

fn run_stream(reader: impl BufRead) -> io::Result<()> {
    let wire = Arc::new(Wire {
        out: Mutex::new(io::stdout()),
        sequence: AtomicU64::new(0),
    });
    for line in reader.lines() {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }
        let request = match serde_json::from_str::<Value>(&line) {
            Ok(request) => request,
            Err(_) => {
                wire.error(
                    &Value::Null,
                    INVALID_PARAMS,
                    "codex_package_request_invalid",
                );
                continue;
            }
        };
        if answer(&wire, &request) {
            return Ok(());
        }
    }
    Ok(())
}

fn write_once(value: &Value) -> io::Result<()> {
    let stdout = io::stdout();
    let mut writer = stdout.lock();
    serde_json::to_writer(&mut writer, value)?;
    writer.write_all(b"\n")?;
    writer.flush()
}

fn main() -> io::Result<()> {
    // This process is the host of its own turn events: it reads them to learn
    // the app-server thread a live turn is bound to. The port is installed once
    // and never invented: before it is installed the package emits nothing.
    let _ = turn_event::install(TurnEventPort {
        emit: record_turn_event,
    });
    let mut args = std::env::args().skip(1);
    match args.next().as_deref() {
        Some("--describe") => write_once(&describe()),
        None => {
            let stdin = io::stdin();
            run_stream(stdin.lock())
        }
        Some(_) => write_once(&json!({"ok": false, "code": "codex_package_option_unsupported"})),
    }
}
