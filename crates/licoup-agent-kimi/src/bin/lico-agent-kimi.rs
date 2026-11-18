//! The Kimi Code package's own program: the native entry its manifest declares.
//!
//! This is the process an extension host starts. It links no client crate and
//! reads no client state: it answers from the package's own protocol facts.
//!
//! Two bounded request shapes are served, one JSON document per line on stdin,
//! one JSON document per line on stdout:
//!
//! - `{"method":"describe"}` reports this package's identity, the adapter it
//!   carries, the adapter declaration its parser reports, and the Kimi launch
//!   metadata the shared ACP engine runs.
//! - `{"method":"shutdown"}` acknowledges and ends the process.
//!
//! Anything else is refused with a stable code rather than ignored, because a
//! request this program does not implement is not a request it may silently
//! drop. `--describe` answers the same description once and exits, which is what
//! the release tooling and a package inspection use.
//!
//! The program performs no I/O beyond its own two streams, reaches no network,
//! starts no child process, and writes no file. Starting `kimi acp` on a turn's
//! behalf is the shared ACP engine's job, reached through the agent-execution
//! port; this entry point only reports what that execution would be.

use std::io::{self, BufRead, Write};

use licoup_agent_kimi::dialect::DRIVER_ID;
use licoup_agent_kimi::driver::{DRIVER, PROTOCOL_FORMAT, RUNTIME_PROTOCOL};
use licoup_agent_kimi::registration::{ADAPTER_ID, CONTRACT, FRAMING};
use serde_json::{Value, json};

const PACKAGE_ID: &str = "org.licoland.adapter.kimi";
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
        "driver": {
            "driverId": DRIVER_ID,
            "runtimeProtocol": RUNTIME_PROTOCOL,
            "protocolFormat": PROTOCOL_FORMAT,
            "launchArgs": DRIVER.launch_args,
            "modelArgument": DRIVER.launch_model_arg,
            "reasoningEnvironment": DRIVER.launch_reasoning_env,
            "reasoningValues": DRIVER.launch_reasoning_values,
            "allowAllArgument": DRIVER.launch_allow_all_arg,
        },
    })
}

fn refusal(code: &str) -> Value {
    json!({ "ok": false, "code": code })
}

fn answer(request: &Value) -> (Value, bool) {
    match request.get("method").and_then(Value::as_str) {
        Some("describe") => (json!({ "ok": true, "result": describe() }), false),
        Some("shutdown") => (json!({ "ok": true, "result": { "stopped": true } }), true),
        Some(_) => (refusal("kimi_package_method_unsupported"), false),
        None => (refusal("kimi_package_request_invalid"), false),
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
            Err(_) => (refusal("kimi_package_request_invalid"), false),
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
            serde_json::to_writer(&mut writer, &refusal("kimi_package_option_unsupported"))?;
            writer.write_all(b"\n")?;
            writer.flush()
        }
    }
}
