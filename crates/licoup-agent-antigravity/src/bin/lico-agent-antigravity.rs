//! The Antigravity package's own program: the native entry its manifest declares.
//!
//! This is the process an extension host starts, and it is also the process the
//! vendor client's Agent Hooks configuration starts. It links no client crate
//! and reads no client state: it answers from the package's own protocol facts.
//!
//! Three bounded request shapes are served:
//!
//! - `--describe` (or `{"method":"describe"}` on stdin) reports this package's
//!   identity, the adapter it carries, and the adapter declaration its parser
//!   reports.
//! - `{"method":"shutdown"}` acknowledges and ends the process.
//! - `receipt` is the Agent Hooks subcommand: it reads one Stop-hook payload from
//!   stdin and records the native conversation identity in the receipt file the
//!   launching driver named. It replaces the generated `/bin/sh` + `python3`
//!   script the bridge used to install, so the hook path needs no interpreter.
//!
//! In stream mode one JSON document is read per line on stdin and one is written
//! per line on stdout. Anything else is refused with a stable code rather than
//! ignored, because a request this program does not implement is not a request
//! it may silently drop.
//!
//! Beyond its own two streams and the receipt file the hook subcommand writes,
//! the program reaches no network, starts no child process, and writes no other
//! file.

use std::io::{self, BufRead, Write};

use licoup_agent_antigravity::hook;
use licoup_agent_antigravity::registration::{ADAPTER_ID, CONTRACT, FRAMING};
use serde_json::{Value, json};

const PACKAGE_ID: &str = "org.licoland.adapter.antigravity";
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
        Some(_) => (refusal("antigravity_package_method_unsupported"), false),
        None => (refusal("antigravity_package_request_invalid"), false),
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
            Err(_) => (refusal("antigravity_package_request_invalid"), false),
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
        // The Agent Hooks receipt writer. The vendor client's Stop hook starts
        // this with the environment the launching driver exported.
        Some("receipt") => hook::main(),
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
            serde_json::to_writer(
                &mut writer,
                &refusal("antigravity_package_option_unsupported"),
            )?;
            writer.write_all(b"\n")?;
            writer.flush()
        }
    }
}
