//! The package's native converter entry.
//!
//! One invocation converts one data root's appearance state through the client's own
//! migration owner and prints one JSON report. The exit code is the report's own verdict,
//! so a caller cannot read a refusal as a completed conversion.
//!
//! Usage: `licoup-appearance-convert --data-root <path>`
//!
//! `--manifest` prints the package's published manifest document from the same code that
//! performs the conversion, so the packaging carrier never transcribes a format name by
//! hand.

use std::path::PathBuf;
use std::process::ExitCode;

use licoup_appearance::converter::convert;
use licoup_appearance::{ENTRY, PACKAGE_ID, PACKAGE_VERSION};

const USAGE: &str = "\
usage: licoup-appearance-convert --data-root <path>
       licoup-appearance-convert --manifest

Converts one LicoUp data root's appearance state through the client's own migration
owner. The command prints one JSON report and exits 0 only when the appearance domain
reached its target (converted or already current), 3 when the run was refused with a
stable code, 4 when the client's owners could not report the state, and 2 on a usage
error.

--manifest prints the package's published manifest document, including the conversion
declaration this package owns.";

/// What the command line asked for.
enum Invocation {
    Convert(PathBuf),
    Manifest,
    Help,
    Version,
    Usage(&'static str),
}

fn parse(arguments: &[String]) -> Invocation {
    let mut data_root: Option<PathBuf> = None;
    let mut index = 0;
    while index < arguments.len() {
        let argument = arguments[index].as_str();
        match argument {
            "-h" | "--help" => return Invocation::Help,
            "--version" => return Invocation::Version,
            "--manifest" => return Invocation::Manifest,
            "--data-root" => {
                index += 1;
                let Some(value) = arguments.get(index) else {
                    return Invocation::Usage("option_value_missing --data-root");
                };
                data_root = Some(PathBuf::from(value));
            }
            _ => match argument.strip_prefix("--data-root=") {
                Some(value) if !value.is_empty() => data_root = Some(PathBuf::from(value)),
                Some(_) => return Invocation::Usage("option_value_missing --data-root"),
                None => return Invocation::Usage("unknown_option"),
            },
        }
        index += 1;
    }
    match data_root {
        Some(root) => Invocation::Convert(root),
        None => Invocation::Usage("data_root_required"),
    }
}

fn main() -> ExitCode {
    let arguments: Vec<String> = std::env::args().skip(1).collect();
    match parse(&arguments) {
        Invocation::Help => {
            println!("{USAGE}");
            ExitCode::SUCCESS
        }
        Invocation::Version => {
            println!("{PACKAGE_ID} {PACKAGE_VERSION} entry {ENTRY}");
            ExitCode::SUCCESS
        }
        Invocation::Manifest => match licoup_appearance::declaration::manifest_json() {
            Ok(document) => {
                print!("{document}");
                ExitCode::SUCCESS
            }
            Err(_) => {
                eprintln!("manifest_unavailable");
                ExitCode::from(4)
            }
        },
        Invocation::Usage(problem) => {
            eprintln!("{problem}");
            eprintln!("{USAGE}");
            ExitCode::from(2)
        }
        Invocation::Convert(data_root) => {
            let report = convert(&data_root);
            match report.to_json() {
                Ok(document) => print!("{document}"),
                Err(_) => {
                    eprintln!("report_unavailable");
                    return ExitCode::from(4);
                }
            }
            ExitCode::from(u8::try_from(report.exit_code()).unwrap_or(4))
        }
    }
}
