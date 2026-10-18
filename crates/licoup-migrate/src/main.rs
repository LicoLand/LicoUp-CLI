//! Standalone migration tool entry point.
//!
//! Prints one JSON report per invocation and exits non-zero on a typed failure. The
//! exit status is the tool's own evidence: a caller that scripts it can tell a refusal
//! from a completed read without parsing the payload.

use licoup_migrate::cli::{Invocation, Usage, Verb, parse};
use licoup_migrate::error::{
    ARCHIVE_REQUIRED, DATA_ROOT_REQUIRED, PACKAGE_STORE_UNAVAILABLE, TARGET_ROOT_REQUIRED,
    ToolError, WORK_ROOT_REQUIRED,
};
use licoup_migrate::inventory::{InventoryRequest, inventory};
use licoup_migrate::package_conversion::{
    PackageConversionRequest, admission_report, convert as package_convert,
};
use licoup_migrate::rehearse::{RehearsalRequest, rehearse};
use licoup_migrate::resume::{ResumeOptions, resume};
use licoup_migrate::{archive, convert, converter, inspect, plan};
use licoup_native::domain::local_recovery;
use std::path::Path;
use std::process::ExitCode;

fn main() -> ExitCode {
    let arguments: Vec<String> = std::env::args().skip(1).collect();
    let invocation = match parse(&arguments) {
        Ok(invocation) => invocation,
        Err(Usage::Help) => {
            println!("{}", licoup_migrate::cli::HELP);
            return ExitCode::SUCCESS;
        }
        Err(Usage::Version) => {
            println!("{}", env!("CARGO_PKG_VERSION"));
            return ExitCode::SUCCESS;
        }
        Err(usage) => {
            println!(
                "{}",
                serde_json::json!({ "status": "refused", "error": usage.to_string() })
            );
            return ExitCode::from(2);
        }
    };

    match run(&invocation) {
        Ok(outcome) => {
            println!("{}", outcome.report);
            // The exit status is part of the report: a run that left a domain owed is not
            // a completed conversion, so a caller cannot mistake it for one.
            ExitCode::from(if outcome.finished { 0 } else { 1 })
        }
        Err(error) => {
            // A rehearsal refusal names the shape it refused, because "not the last published
            // format" is only actionable with the format the source actually carries. The
            // read writes nothing, so a refusal still leaves the source untouched.
            let refusal = match invocation.verb {
                // A package conversion that was refused still answers the question the
                // operator asked first: what does this host say about maintenance now?
                // The decision is read from its own owner, never restated here.
                Verb::PackageConvert | Verb::PackageResume => match invocation.data_root.as_deref()
                {
                    Some(root) => serde_json::json!({
                        "status": "refused",
                        "error": error.code(),
                        "admission": admission_report(root),
                    }),
                    None => serde_json::json!({ "status": "refused", "error": error.code() }),
                },
                Verb::Rehearse => match invocation
                    .data_root
                    .as_deref()
                    .map(licoup_migrate::rehearse::source_shape)
                {
                    Some(Ok(shape)) => serde_json::json!({
                        "status": "refused",
                        "error": error.code(),
                        "observedShape": shape,
                    }),
                    _ => serde_json::json!({ "status": "refused", "error": error.code() }),
                },
                _ => serde_json::json!({ "status": "refused", "error": error.code() }),
            };
            println!("{refusal}");
            ExitCode::from(1)
        }
    }
}

/// One rendered report, with the question a caller asks about it.
struct Outcome {
    report: String,
    /// Whether the verb reached the result it was asked for.
    finished: bool,
}

impl Outcome {
    /// A read that answered, or a run that finished everything it owed.
    fn done(report: String) -> Self {
        Self {
            report,
            finished: true,
        }
    }

    /// A run that ran but did not finish the work it was asked about.
    fn unfinished(report: String) -> Self {
        Self {
            report,
            finished: false,
        }
    }
}

fn run(invocation: &Invocation) -> Result<Outcome, ToolError> {
    // Use the client's process-lifetime coordination, not merely the migration
    // owner's short admission lock. Hold it across every nested rehearsal or
    // recovery operation, without reacquiring it in the library helpers. The
    // operator statement still covers older/nonparticipating writers.
    let _selected_home = match invocation.verb {
        Verb::Inspect | Verb::Plan | Verb::Converters => None,
        Verb::Convert
        | Verb::Resume
        | Verb::Export
        | Verb::Import
        | Verb::Rehearse
        | Verb::PackageConvert
        | Verb::PackageResume => Some(
            local_recovery::acquire_exclusive_selected_home()
                .map_err(|_| ToolError::new("data_home_coordination_unavailable"))?
                .ok_or_else(|| ToolError::new(local_recovery::WRITERS_RUNNING))?,
        ),
    };
    match invocation.verb {
        Verb::Inspect => {
            let report = inspect::inspect(data_root(invocation)?)?;
            // A read that preserved another domain's refusal is still an unfinished
            // read: the report names the refused domains and the exit status follows.
            let finished = report.refused.is_empty() && report.newer_than_tool.is_empty();
            let rendered = render(&report)?;
            if finished {
                Ok(Outcome::done(rendered))
            } else {
                Ok(Outcome::unfinished(rendered))
            }
        }
        Verb::Plan => {
            let report = plan::plan(data_root(invocation)?, invocation.target.as_deref())?;
            plan::ensure_readable(&report)?;
            // A blocked authority is never planned from an invented version, and the
            // caller must not read the plan as complete work.
            let finished = report.blocked.is_empty();
            let rendered = render(&report)?;
            if finished {
                Ok(Outcome::done(rendered))
            } else {
                Ok(Outcome::unfinished(rendered))
            }
        }
        Verb::Convert => {
            let root = data_root(invocation)?;
            // The parser already required the statement; the flag is passed down so the
            // conversion itself refuses a run that never made it.
            let planned = plan::plan(root, invocation.target.as_deref())?;
            let owed: Vec<String> = planned
                .domains
                .iter()
                .map(|domain| domain.domain_id.clone())
                .collect();
            let report = convert::convert(root, &owed, invocation.writers_stopped)?;
            let rendered = render(&report)?;
            if report.is_complete() {
                Ok(Outcome::done(rendered))
            } else {
                Ok(Outcome::unfinished(rendered))
            }
        }
        Verb::Resume => {
            let report = resume(
                data_root(invocation)?,
                ResumeOptions {
                    writers_stopped: invocation.writers_stopped,
                    interrupter: None,
                },
                None,
            )?;
            let finished = report.is_complete();
            let rendered = render(&report)?;
            if finished {
                Ok(Outcome::done(rendered))
            } else {
                Ok(Outcome::unfinished(rendered))
            }
        }
        // The archive verbs carry the caller's options to the client's own archive owner
        // and render that owner's verdict; neither one archives anything here.
        Verb::Export => Ok(Outcome::done(render(&archive::export(
            data_root(invocation)?,
            archive_path(invocation)?,
            invocation.writers_stopped,
        )?)?)),
        Verb::Import => Ok(Outcome::done(render(&archive::import(
            archive_path(invocation)?,
            target_root(invocation)?,
        )?)?)),
        // The inventory is a read of one package store: it reports what is
        // installed, which installed package declares the required pair, and — when
        // the caller names a data root — what this host's own decision is.
        Verb::Converters => {
            let required = converter::required_conversion()?;
            let mut report = inventory(
                &InventoryRequest {
                    package_store: package_store(invocation)?,
                    index: invocation.index.as_deref(),
                    index_public_keys: invocation.index_public_keys.as_deref(),
                    requested_package: invocation.package.as_deref(),
                },
                &required,
            )?
            .0;
            if let Some(root) = invocation.data_root.as_deref() {
                report.admission = Some(admission_report(root));
            }
            let finished = !report.candidates.is_empty();
            let rendered = render(&report)?;
            if finished {
                Ok(Outcome::done(rendered))
            } else {
                Ok(Outcome::unfinished(rendered))
            }
        }
        // The rehearsal drives the same owners as a conversion and the two archive verbs,
        // one stage at a time. Its report names every stage whether or not it ran, so a run
        // that stopped part way is rendered rather than hidden; the exit status follows the
        // report so a caller that scripts it cannot read a partial rehearsal as a recovery.
        // One package-owned conversion: the package store's own admission for the
        // payload, the host's own maintenance decision for the root, and the
        // package's declared converter run by the tool over a staged copy.
        Verb::PackageConvert | Verb::PackageResume => {
            let report = package_convert(&PackageConversionRequest {
                data_root: data_root(invocation)?,
                work_root: work_root(invocation)?,
                package_store: package_store(invocation)?,
                package: invocation.package.as_deref(),
                payload: invocation.payload.as_deref(),
                index: invocation.index.as_deref(),
                index_public_keys: invocation.index_public_keys.as_deref(),
                writers_stopped: invocation.writers_stopped,
                resume: invocation.verb == Verb::PackageResume,
                stop: None,
            })?;
            let finished = report.is_complete();
            let rendered = render(&report)?;
            if finished {
                Ok(Outcome::done(rendered))
            } else {
                Ok(Outcome::unfinished(rendered))
            }
        }
        Verb::Rehearse => {
            let report = rehearse(&RehearsalRequest {
                data_root: data_root(invocation)?.to_path_buf(),
                work_root: work_root(invocation)?.to_path_buf(),
                writers_stopped: invocation.writers_stopped,
                keep_work_root: invocation.keep_work_root,
            })?;
            let finished = report.is_complete();
            let rendered = render(&report)?;
            if finished {
                Ok(Outcome::done(rendered))
            } else {
                Ok(Outcome::unfinished(rendered))
            }
        }
    }
}

/// The parser refuses a verb that omits an input it requires, so these guards carry the
/// same rule as a typed refusal for a future verb added without its parse rule. They
/// never render a path or a stored value.
fn data_root(invocation: &Invocation) -> Result<&Path, ToolError> {
    invocation.data_root.as_deref().ok_or(DATA_ROOT_REQUIRED)
}

fn archive_path(invocation: &Invocation) -> Result<&Path, ToolError> {
    invocation.archive.as_deref().ok_or(ARCHIVE_REQUIRED)
}

fn target_root(invocation: &Invocation) -> Result<&Path, ToolError> {
    invocation
        .target_root
        .as_deref()
        .ok_or(TARGET_ROOT_REQUIRED)
}

fn work_root(invocation: &Invocation) -> Result<&Path, ToolError> {
    invocation.work_root.as_deref().ok_or(WORK_ROOT_REQUIRED)
}

fn package_store(invocation: &Invocation) -> Result<&Path, ToolError> {
    invocation
        .package_store
        .as_deref()
        .ok_or(PACKAGE_STORE_UNAVAILABLE)
}

fn render<T: serde::Serialize>(value: &T) -> Result<String, ToolError> {
    Ok(serde_json::to_string(value).unwrap_or_else(|_| "{}".to_string()))
}
