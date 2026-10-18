//! The OpenCode package's own program: the native entry its manifest declares.
//!
//! This is the process an extension host starts. It links no client crate and
//! reads no client state: it answers from the package's own protocol facts.
//!
//! Two bounded request shapes are served, one JSON document per line on stdin,
//! one JSON document per line on stdout:
//!
//! - `{"method":"describe"}` reports this package's identity, the adapter it
//!   carries, and the adapter declaration its parser reports.
//! - `{"method":"shutdown"}` acknowledges and ends the process.
//!
//! Anything else is refused with a stable code rather than ignored, because a
//! request this program does not implement is not a request it may silently
//! drop. `--describe` answers the same description once and exits, which is what
//! the release tooling and a package inspection use.
//!
//! The program performs no I/O beyond its own two streams, reaches no network,
//! starts no child process, and writes no file. In particular it does not start
//! or attach to an `opencode serve` endpoint: that engine is the client's, and
//! this entry reports the package's protocol facts until the extension host
//! drives a turn through it.

use std::io::{self, BufRead, Write};

use licoup_agent_opencode::registration::{ADAPTER_ID, CONTRACT, FRAMING, PROTOCOL_FORMAT};
use serde_json::{Value, json};

const PACKAGE_ID: &str = "org.licoland.adapter.opencode";
const PACKAGE_VERSION: &str = env!("CARGO_PKG_VERSION");
const HOST_PROTOCOL_MAJOR: u64 = 1;
const HOST_PROTOCOL_MINIMUM_MINOR: u64 = 0;

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
        "protocolFormat": PROTOCOL_FORMAT,
        "contract": CONTRACT.inventory_json(),
    })
}

fn refusal(code: &str) -> Value {
    json!({ "ok": false, "code": code })
}

fn answer(request: &Value) -> (Value, bool) {
    match request.get("method").and_then(Value::as_str) {
        Some("describe") => (json!({ "ok": true, "result": describe() }), false),
        Some("shutdown") => (json!({ "ok": true, "result": { "stopped": true } }), true),
        Some(_) => (refusal("opencode_package_method_unsupported"), false),
        None => (refusal("opencode_package_request_invalid"), false),
    }
}

fn run_stream(reader: impl BufRead, mut writer: impl Write) -> io::Result<()> {
    for line in reader.lines() {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }
        let (response, stopping) = match serde_json::from_str::<Value>(&line) {
            Ok(request) => answer(&request),
            Err(_) => (refusal("opencode_package_request_invalid"), false),
        };
        serde_json::to_writer(&mut writer, &response)?;
        writer.write_all(b"\n")?;
        writer.flush()?;
        if stopping {
            return Ok(());
        }
    }
    Ok(())
}

fn main() -> io::Result<()> {
    let mut args = std::env::args().skip(1);
    match args.next().as_deref() {
        Some("--describe") => {
            let stdout = io::stdout();
            let mut writer = stdout.lock();
            serde_json::to_writer(&mut writer, &describe())?;
            writer.write_all(b"\n")?;
            writer.flush()
        }
        None => {
            let stdin = io::stdin();
            let stdout = io::stdout();
            run_stream(stdin.lock(), stdout.lock())
        }
        Some(_) => {
            let stdout = io::stdout();
            let mut writer = stdout.lock();
            serde_json::to_writer(&mut writer, &refusal("opencode_package_option_unsupported"))?;
            writer.write_all(b"\n")?;
            writer.flush()
        }
    }
}
