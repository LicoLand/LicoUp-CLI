//! The DeepSeek Harness package's own program: the native entry its manifest
//! declares.
//!
//! This is the process an extension host starts. It links no client crate and
//! reads no client state: it answers from the package's own protocol and session
//! facts.
//!
//! Three bounded request shapes are served, one JSON document per line on
//! stdin, one JSON document per line on stdout:
//!
//! - `{"method":"describe"}` reports this package's identity, the adapter it
//!   carries, the adapter declaration its parser reports, and the session-log
//!   generation its reader implements.
//! - `{"method":"usage","path":"<artifact>"}` folds one session artifact and
//!   answers with its samples. This is the package's declared native converter:
//!   the vendor's own row format in, LicoUp's usage samples out, with no Node
//!   runtime and no vendor library involved.
//! - `{"method":"shutdown"}` acknowledges and ends the process.
//!
//! Anything else is refused with a stable code rather than ignored, because a
//! request this program does not implement is not a request it may silently
//! drop. `--describe` answers the same description once and exits, which is what
//! the release tooling and a package inspection use.
//!
//! The program performs no I/O beyond its own two streams and the one artifact a
//! `usage` request names, reaches no network, starts no child process, and
//! writes no file.

use std::io::{self, BufRead, Write};
use std::path::Path;

use licoup_agent_deepseek::registration::{ADAPTER_ID, CONTRACT, FRAMING};
use licoup_agent_deepseek::session_store::{
    COMPRESSED_SUFFIX, CURRENT_FORMAT_VERSION, SessionReadError, read_usage_samples,
};
use serde_json::{Value, json};

const PACKAGE_ID: &str = "org.licoland.adapter.deepseek";
const PACKAGE_VERSION: &str = env!("CARGO_PKG_VERSION");
const HOST_PROTOCOL_MAJOR: u64 = 1;
const HOST_PROTOCOL_MINIMUM_MINOR: u64 = 0;
/// The format a usage request reads, as this package's release declaration
/// names it.
const SOURCE_FORMAT: &str = "deepseek-harness-session-jsonl";
/// The format a usage answer reports, which is the canonical usage projection
/// the client already reads from every Agent.
const TARGET_FORMAT: &str = "licoup.usage-samples.v1";

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
        "usageReader": {
            "sourceFormat": SOURCE_FORMAT,
            "targetFormat": TARGET_FORMAT,
            "formatVersion": CURRENT_FORMAT_VERSION,
            "compressedSuffix": COMPRESSED_SUFFIX,
        },
    })
}

fn refusal(code: &str) -> Value {
    json!({ "ok": false, "code": code })
}

/// The refusal code for one session artifact this reader will not fold.
fn read_failure(error: &SessionReadError) -> &'static str {
    match error {
        SessionReadError::Io(_) => "deepseek_package_artifact_unreadable",
        SessionReadError::Malformed(_) => "deepseek_package_artifact_malformed",
        SessionReadError::UnsupportedFormatVersion(_) => "deepseek_package_artifact_unsupported",
    }
}

/// Answer one usage request, never echoing the artifact's own bytes back.
fn usage(request: &Value) -> Value {
    let Some(path) = request.get("path").and_then(Value::as_str) else {
        return refusal("deepseek_package_request_invalid");
    };
    match read_usage_samples(Path::new(path)) {
        Ok(samples) => json!({ "ok": true, "result": { "samples": samples } }),
        Err(error) => {
            let mut answer = refusal(read_failure(&error));
            // The declared generation travels with the refusal, because "this is
            // a generation I do not read" is actionable and "this failed" is not.
            if let SessionReadError::UnsupportedFormatVersion(version) = error {
                answer["declaredVersion"] = json!(version);
                answer["readVersion"] = json!(CURRENT_FORMAT_VERSION);
            }
            answer
        }
    }
}

fn answer(request: &Value) -> (Value, bool) {
    match request.get("method").and_then(Value::as_str) {
        Some("describe") => (json!({ "ok": true, "result": describe() }), false),
        Some("usage") => (usage(request), false),
        Some("shutdown") => (json!({ "ok": true, "result": { "stopped": true } }), true),
        Some(_) => (refusal("deepseek_package_method_unsupported"), false),
        None => (refusal("deepseek_package_request_invalid"), false),
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
            Err(_) => (refusal("deepseek_package_request_invalid"), false),
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
            serde_json::to_writer(&mut writer, &refusal("deepseek_package_option_unsupported"))?;
            writer.write_all(b"\n")?;
            writer.flush()
        }
    }
}
