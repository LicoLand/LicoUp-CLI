//! The example extension's declared converter entry, as a package carries it.
//!
//! The package manifest names exactly one native program that performs its
//! declared conversion, and this file is that program's whole source. It is
//! compiled by the verification entry that imports the sample — a package
//! carries no build system, and the host starts what the payload contains:
//!
//! ```text
//! rustc --edition=2021 -C debuginfo=0 -o bin/licoup-sample-state-converter converter/main.rs
//! ```
//!
//! The invocation protocol is the contract's, and it is deliberately small:
//!
//! ```text
//! <entry> --source <dir> --target <dir> \
//!         --source-format <identity> --target-format <identity> \
//!         --result <file> [--resume]
//! ```
//!
//! `licoup-migrate` is the caller. It has already stopped the writers, holds the
//! data root's admission, and hands this program a staged copy of the root plus
//! the format pair it selected the package for.
//!
//! Three of the caller's rules are the program's obligations:
//!
//! * **The source is read, never written.** This program opens source documents
//!   for reading only. The caller verifies the data root's digest after the run
//!   and refuses a converter that wrote inside it.
//! * **A run is continued, never restarted.** On `--resume` this program appends
//!   to the target it already has instead of clearing it. Nothing here deletes
//!   the target.
//! * **The run's outcome is the result document, not the exit status.** A zero
//!   exit without `--result` reported as `complete` is an unfinished run.
//!
//! The exit statuses are the program's own: `0` for a run that reported, `2` for
//! an invocation this program cannot answer, and `3` for a source it cannot read.
//! A caller that stops the process group mid-run gets no result document, which
//! is exactly how an interrupted run stays resumable.

use std::env;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::exit;
use std::time::Duration;

/// The result document's schema identity, as the contract publishes it.
const RESULT_SCHEMA: &str = "licoup.package-conversion-result.v1";

/// The directory the persisted client state lives in, inside either root.
const STATE_DIRECTORY: &str = "client-state";

/// A sample-only control document, read when the operator writes one.
///
/// It exists so the interruption path can be exercised deliberately rather than
/// by racing a fast process: the run states how long it will hold before it
/// writes its result, and a caller that stops it in that window observes the
/// interrupted run the resume contract is about.
const HOLD_DOCUMENT: &str = "client-state/sample-conversion-hold-millis.txt";

/// Exit statuses this program reports.
const EXIT_USAGE: i32 = 2;
const EXIT_SOURCE_UNREADABLE: i32 = 3;

struct Invocation {
    source: PathBuf,
    target: PathBuf,
    source_format: String,
    target_format: String,
    result: PathBuf,
    resume: bool,
}

fn main() {
    let Some(invocation) = parse(env::args().skip(1)) else {
        eprintln!(
            "usage: <entry> --source <dir> --target <dir> --source-format <identity> \
             --target-format <identity> --result <file> [--resume]"
        );
        exit(EXIT_USAGE);
    };
    match convert(&invocation) {
        Ok(converted_records) => {
            if !write_result(&invocation, converted_records) {
                exit(EXIT_SOURCE_UNREADABLE);
            }
        }
        Err(message) => {
            eprintln!("{message}");
            exit(EXIT_SOURCE_UNREADABLE);
        }
    }
}

/// Read the documented arguments, or `None` for an invocation to refuse.
fn parse(arguments: impl Iterator<Item = String>) -> Option<Invocation> {
    let mut source = None;
    let mut target = None;
    let mut source_format = None;
    let mut target_format = None;
    let mut result = None;
    let mut resume = false;
    let arguments: Vec<String> = arguments.collect();
    let mut index = 0;
    while index < arguments.len() {
        let value = || arguments.get(index + 1).cloned();
        match arguments[index].as_str() {
            "--source" => {
                source = value();
                index += 2;
            }
            "--target" => {
                target = value();
                index += 2;
            }
            "--source-format" => {
                source_format = value();
                index += 2;
            }
            "--target-format" => {
                target_format = value();
                index += 2;
            }
            "--result" => {
                result = value();
                index += 2;
            }
            "--resume" => {
                resume = true;
                index += 1;
            }
            // An argument this program does not know is not one it can answer:
            // guessing would let a caller's typo become a silent no-op run.
            _ => return None,
        }
    }
    Some(Invocation {
        source: PathBuf::from(source?),
        target: PathBuf::from(target?),
        source_format: source_format?,
        target_format: target_format?,
        result: PathBuf::from(result?),
        resume,
    })
}

/// Perform the declared move and report how many documents it converted.
///
/// The sample's move is deliberately trivial — it re-encodes one document per
/// source document and counts them — because the point of the sample is the
/// packaging, import and invocation contract, not a domain migration. A real
/// converter replaces this function and keeps everything around it.
fn convert(invocation: &Invocation) -> Result<u64, String> {
    if !invocation.source.is_dir() {
        return Err(format!(
            "source is not a directory: {}",
            invocation.source.display()
        ));
    }
    let target_state = invocation.target.join(STATE_DIRECTORY);
    fs::create_dir_all(&target_state)
        .map_err(|error| format!("target is not writable: {error}"))?;

    // The attempt log is the sample's own evidence that a resumed run continued
    // the target instead of starting it over; the contract does not require it.
    let attempt = attempts(invocation) + 1;
    append(
        &invocation.target.join("attempts.log"),
        &format!("attempt={attempt} resume={}\n", invocation.resume),
    )?;
    // Created once and never rewritten: a continued run leaves the first
    // attempt's marker exactly as it found it, which is what "the target was not
    // cleared" means on disk.
    mark(&invocation.target.join("started"), "started\n")?;
    if !invocation.resume {
        hold(invocation);
    }

    let documents = read_documents(&invocation.source)?;
    for (name, bytes) in &documents {
        // Grown, never shrunk: a resumed run continues the target it already has.
        let destination = target_state.join(name);
        fs::write(&destination, bytes)
            .map_err(|error| format!("target document is not writable: {error}"))?;
    }
    Ok(documents.len() as u64)
}

/// Every `client-state/*.json` document of one root, in a stable order.
fn read_documents(root: &Path) -> Result<Vec<(String, Vec<u8>)>, String> {
    let directory = root.join(STATE_DIRECTORY);
    if !directory.is_dir() {
        return Ok(Vec::new());
    }
    let mut documents = Vec::new();
    let entries = fs::read_dir(&directory)
        .map_err(|error| format!("source state is unreadable: {error}"))?;
    for entry in entries {
        let entry = entry.map_err(|error| format!("source state is unreadable: {error}"))?;
        let path = entry.path();
        let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
            continue;
        };
        if !path.is_file() || !name.ends_with(".json") {
            continue;
        }
        // Read-only, and the only open this program performs on the source.
        let bytes = fs::read(&path).map_err(|error| format!("source is unreadable: {error}"))?;
        documents.push((name.to_owned(), bytes));
    }
    documents.sort_by(|left, right| left.0.cmp(&right.0));
    Ok(documents)
}

/// How many attempts the target already records.
fn attempts(invocation: &Invocation) -> u32 {
    fs::read_to_string(invocation.target.join("attempts.log"))
        .map(|text| text.lines().count() as u32)
        .unwrap_or(0)
}

fn append(path: &Path, text: &str) -> Result<(), String> {
    let mut file = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .map_err(|error| format!("target is not writable: {error}"))?;
    file.write_all(text.as_bytes())
        .map_err(|error| format!("target is not writable: {error}"))
}

/// Write one marker only when the target does not already carry it.
fn mark(path: &Path, text: &str) -> Result<(), String> {
    match fs::OpenOptions::new().create_new(true).write(true).open(path) {
        Ok(mut file) => file
            .write_all(text.as_bytes())
            .map_err(|error| format!("target is not writable: {error}")),
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => Ok(()),
        Err(error) => Err(format!("target is not writable: {error}")),
    }
}

/// Wait for as long as the operator's control document states.
///
/// Sample scope only: it exists so the stop path is observable instead of raced,
/// and it applies to a first attempt — a run that was asked to continue has
/// already been held once.
fn hold(invocation: &Invocation) {
    let Ok(text) = fs::read_to_string(invocation.source.join(HOLD_DOCUMENT)) else {
        return;
    };
    let Ok(millis) = text.trim().parse::<u64>() else {
        return;
    };
    std::thread::sleep(Duration::from_millis(millis.min(60_000)));
}

/// Write the one document the caller reads as this run's outcome.
fn write_result(invocation: &Invocation, converted_records: u64) -> bool {
    let document = format!(
        "{{\"schema\":\"{RESULT_SCHEMA}\",\"sourceFormat\":\"{}\",\"targetFormat\":\"{}\",\
         \"complete\":true,\"convertedRecords\":{converted_records}}}",
        invocation.source_format, invocation.target_format
    );
    if let Some(parent) = invocation.result.parent() {
        if fs::create_dir_all(parent).is_err() {
            return false;
        }
    }
    fs::write(&invocation.result, document).is_ok()
}
