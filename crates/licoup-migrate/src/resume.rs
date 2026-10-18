//! Continue an interrupted conversion until it is unambiguously finished or blocked.
//!
//! A resume is not a second conversion. The journal names the steps one run declared; this
//! module drives those steps through the client's own owner one at a time and records what
//! the owner reported, so a process that stops between two steps is continued rather than
//! restarted. Three properties are what the resume is built around:
//!
//! 1. **One result.** A step is committed only when the owner reports it applied or already
//!    current *and* the client's observed state shows the version the journal asked for. A
//!    journal entry that claims a commit the store does not support is a refusal, never a
//!    repair.
//! 2. **No duplication.** The only record of a conversion is the client's own ledger and
//!    its domain markers, both written by the owner under its own lock. This module never
//!    writes either; it writes only its own snapshot, which the owner never reads. A
//!    resumed domain is therefore recorded once by construction, and the resume regression
//!    asserts that the client's ledger is byte-identical afterwards.
//! 3. **A blocked run stays visibly blocked.** A step the owner cannot complete — platform
//!    credential custody is the live case — leaves the run paused with that domain named,
//!    and a journal that still owes steps is never closed.
//!
//! Interruption points are exercised at runtime through [`Interrupter`]. The seam exists
//! because the interesting failure is a process that stops **between** two durable writes,
//! and no integration test can deliver a real `SIGKILL` at a chosen instruction from
//! outside the process. The permitted points are the boundaries the durability claim is
//! made at, and the regression in `tests/resume.rs` runs the whole resume once per point.

use crate::error::{
    COMMIT_UNSUPPORTED, DATA_ROOT_MISSING, DATA_ROOT_NOT_DIRECTORY, FRONTIER_UNAVAILABLE,
    STATE_UNAVAILABLE, STOPPED, ToolResult, WRITERS_RUNNING,
};
use crate::journal::ledger::{ClientLedger, LedgerSnapshot};
use crate::journal::{self, Journal, JournalPlan, markers};
use serde::Serialize;
use std::path::Path;

/// The name of the point before the client's owner is asked to move anything.
pub const BEFORE_OWNER: &str = "before-owner";
/// The name of the point after the client's owner produced its durable result and before
/// this tool recorded it: a step interrupted here has already committed.
pub const AFTER_OWNER: &str = "after-owner";
/// The name of the point after a step's outcome was recorded.
pub const AFTER_RECORD: &str = "after-record";

/// The point names a caller may interrupt the resume at.
pub const INTERRUPTION_POINTS: &[&str] = &[BEFORE_OWNER, AFTER_OWNER, AFTER_RECORD];

/// The schema identity of the client's own per-domain conversion marker.
const DOMAIN_MARKER_SCHEMA: &str = "v0.0.1:client-state-domain-marker-1";

/// One domain's marker, as the client's owner writes it.
#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct DomainMarker {
    schema_version: String,
    domain_id: String,
    authoritative_schema_version: u32,
}

/// A decision to stop the resume at a named boundary, as an interrupted process would.
pub type Interrupter<'a> = dyn Fn(&Journal, &str, &str) -> bool + 'a;

/// How the client's owner accounted for one step.
#[derive(Clone, Debug, Eq, PartialEq)]
enum OwnerVerdict {
    /// The owner performed the move.
    Applied,
    /// The owner reported the domain already at the version asked for.
    AlreadyCurrent,
    /// The owner cannot finish the move without platform authority.
    PendingAuthorization,
    /// The owner did not account for the domain at all.
    Unaccounted,
    /// The owner refused the root outright, carrying its own code.
    Refused(String),
}

/// One step's reported outcome.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ResumedStep {
    pub domain_id: String,
    pub step_id: String,
    /// `applied`, `alreadyCurrent`, `appliedBeforeInterruption`, or `pendingAuthorization`.
    pub outcome: &'static str,
    pub target_version: u32,
    pub observed_version: u32,
    pub committed_version: Option<u32>,
    /// True when this step's outcome was already durable when the resume started.
    pub settled_before_resume: bool,
    /// The owner's own refusal code when it declined the step, if any.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub refused: Option<String>,
}

/// What one resume attempt concluded.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ResumeReport {
    /// `completed`, `alreadyCurrent`, `paused`, `blocked`, or `noOp`.
    pub status: &'static str,
    pub target_version: String,
    pub frontier_id: String,
    pub steps: Vec<ResumedStep>,
    /// Domains that still owe a step after this attempt.
    pub still_owed: Vec<String>,
    /// Domains waiting for authority this tool does not hold.
    pub pending_authorization: Vec<String>,
    pub converted_domains: usize,
    pub declared_domains: usize,
    /// Every completed step id the client's ledger holds, counted per domain.
    pub ledger_records: usize,
}

impl ResumeReport {
    /// Whether the run is finished.
    pub fn is_complete(&self) -> bool {
        self.status == "completed" || self.status == "alreadyCurrent"
    }
}

/// How a caller asked for the resume.
#[derive(Clone, Copy, Default)]
pub struct ResumeOptions<'a> {
    /// The operator's statement that no writer is running against the data root.
    pub writers_stopped: bool,
    /// A runtime interruption point, used by the regression and unset in production.
    pub interrupter: Option<&'a Interrupter<'a>>,
}

/// Continue the interrupted run recorded for one data root.
///
/// In production the owner is always the client's own admission; a caller that supplies
/// its own `commit` narrows the owner, which is how the regression places an interruption
/// on one side of a single domain's commit.
pub fn resume(
    data_root: &Path,
    options: ResumeOptions<'_>,
    commit: Option<OwnerCommit<'_>>,
) -> ToolResult<ResumeReport> {
    match commit {
        Some(commit) => run(data_root, options, commit),
        None => run(data_root, options, &mut |root: &Path| {
            licoup_native::domain::client_state_migration::admit(root)
        }),
    }
}

/// The client's own admission, as the resume drives it.
pub type OwnerCommit<'a> = &'a mut dyn FnMut(
    &Path,
) -> anyhow::Result<
    licoup_native::domain::client_state_migration::AdmissionResult,
>;

/// The resume, with the owner's commit step named explicitly.
fn run(
    data_root: &Path,
    options: ResumeOptions<'_>,
    mut owner: impl FnMut(
        &Path,
    ) -> anyhow::Result<
        licoup_native::domain::client_state_migration::AdmissionResult,
    >,
) -> ToolResult<ResumeReport> {
    if !data_root.exists() {
        return Err(DATA_ROOT_MISSING);
    }
    if !data_root.is_dir() {
        return Err(DATA_ROOT_NOT_DIRECTORY);
    }
    let absolute = data_root.canonicalize().map_err(|_| DATA_ROOT_MISSING)?;

    // The operator's statement is checked before anything is opened or moved, so a resume
    // that lacks it cannot have touched the root at all. It is required even when no
    // journal turns out to exist: the tool reads the client's stores to answer that
    // question, and reading them while a writer runs is what the statement forbids.
    if !options.writers_stopped {
        return Err(WRITERS_RUNNING);
    }

    let Some(journal) = journal::open(&absolute)? else {
        // Nothing was interrupted here. A caller that wants a conversion asks for one; a
        // resume that invented work would be the one thing it must never do.
        return no_op_report(&absolute);
    };

    // A document that already settled is the evidence of the finished run, so asking again
    // answers from the owner's observed state instead of running a second conversion. It
    // also must not rewrite the record: a second run that changed the client's ledger
    // would be a second conversion wearing a resume's name.
    if journal::is_finished(&journal) {
        let recorded = LedgerSnapshot::read(&absolute)?;
        let report = already_current_report(&absolute, &journal, &mut owner)?;
        let after = LedgerSnapshot::read(&absolute)?;
        if report.is_complete() && !recorded.equals(&after) {
            return Err(COMMIT_UNSUPPORTED);
        }
        return Ok(report);
    }

    let interrupted = std::cell::Cell::new(false);
    let mut steps = Vec::new();

    // Deliberately a `loop` with a `let ... else` rather than `while let`: a `while let`
    // scrutinee lives for the whole loop body, so the journal document the test borrows
    // here would still be open while the body rewrites it.
    #[allow(clippy::while_let_loop)]
    loop {
        let Some((domain_id, entry)) = journal::open(&absolute)?
            .as_ref()
            .and_then(Journal::next_step)
            .map(|(domain_id, entry)| (domain_id.to_string(), entry.clone()))
        else {
            break;
        };

        if let Some(interrupter) = options.interrupter {
            let current = journal::open(&absolute)?.unwrap_or_else(|| journal.clone());
            if interrupter(&current, &domain_id, BEFORE_OWNER) {
                interrupted.set(true);
                break;
            }
        }

        journal::mark_running(&absolute, &domain_id, &entry.step_id)?;

        let verdict = owner_verdict(&mut owner, &absolute, &domain_id, entry.target_version)?;

        if let Some(interrupter) = options.interrupter {
            let current = journal::open(&absolute)?.unwrap_or_else(|| journal.clone());
            if interrupter(&current, &domain_id, AFTER_OWNER) {
                interrupted.set(true);
                break;
            }
        }

        let observed = observed_version(&absolute, &domain_id)?;
        // The client's frontier is the only authority on what a domain's target is. The
        // journal was written from it, so a journal step the frontier does not declare is a
        // document this tool must not act on: refusing it here is what keeps the tool from
        // recording a version the client's owner would never report.
        let declared_target = declared_target_version(&domain_id)?;
        let (outcome, recorded) = match verdict {
            OwnerVerdict::Applied | OwnerVerdict::AlreadyCurrent
                if Some(entry.target_version) == declared_target =>
            {
                (
                    if verdict == OwnerVerdict::Applied {
                        "applied"
                    } else {
                        "alreadyCurrent"
                    },
                    true,
                )
            }
            OwnerVerdict::Applied | OwnerVerdict::AlreadyCurrent => {
                journal::mark_pending(&absolute, &domain_id)?;
                ("unsupportedTarget", false)
            }
            OwnerVerdict::PendingAuthorization => {
                journal::mark_pending_authorization(&absolute, &domain_id)?;
                ("pendingAuthorization", false)
            }
            OwnerVerdict::Refused(_) => {
                journal::mark_pending(&absolute, &domain_id)?;
                ("refused", false)
            }
            OwnerVerdict::Unaccounted => {
                if Some(entry.target_version) == declared_target && observed >= entry.target_version
                {
                    // The owner moved the domain and reported nothing for it, which is the
                    // state a process leaves when it stops between the move and the report.
                    ("appliedBeforeInterruption", true)
                } else {
                    journal::mark_pending(&absolute, &domain_id)?;
                    ("stillOwed", false)
                }
            }
        };

        let committed_version = if recorded {
            // The owner is the authority on whether the step is done; what has to back its
            // report is the client's *own* durable record of it. A domain the owner calls
            // converted with no marker at the version the frontier declares is a
            // disagreement between two durable documents, and recording a version nothing
            // supports is how a conversion gets reported that never happened.
            if marker_version(&absolute, &domain_id)? < entry.target_version {
                return Err(COMMIT_UNSUPPORTED);
            }
            journal::mark_committed(&absolute, &domain_id, entry.target_version)?;
            Some(entry.target_version)
        } else {
            None
        };

        steps.push(ResumedStep {
            domain_id: domain_id.clone(),
            step_id: entry.step_id.clone(),
            outcome,
            target_version: entry.target_version,
            observed_version: observed,
            committed_version,
            settled_before_resume: false,
            refused: match &verdict {
                OwnerVerdict::Refused(code) => Some(code.clone()),
                _ => None,
            },
        });

        if let Some(interrupter) = options.interrupter {
            let current = journal::open(&absolute)?.unwrap_or_else(|| journal.clone());
            if interrupter(&current, &domain_id, AFTER_RECORD) {
                interrupted.set(true);
                break;
            }
        }

        if committed_version.is_none() && !matches!(verdict, OwnerVerdict::PendingAuthorization) {
            // The owner declined a step it has not paused; asking it again in the same
            // attempt would only repeat the refusal.
            break;
        }

        if journal::open(&absolute)?.is_some_and(|current| current.is_complete()) {
            journal::mark_converted_version(&absolute, &journal.target_version)?;
            journal::finish(&absolute)?;
            break;
        }
    }

    let final_journal = journal::open(&absolute)?.unwrap_or(journal);
    let ledger_after = LedgerSnapshot::read(&absolute)?;
    let client_ledger = ledger_after.parse()?;

    if final_journal.is_complete() {
        // The client's own ledger is the record of the conversion, and a resume must not
        // have rewritten it for a domain that was already complete.
        ensure_no_record_was_duplicated(&client_ledger)?;
    }

    Ok(build_report(
        &final_journal,
        steps,
        client_ledger.as_ref(),
        interrupted.get(),
    ))
}

/// The version the client's own marker records for one domain, or zero when it has none.
fn marker_version(data_root: &Path, domain_id: &str) -> ToolResult<u32> {
    let path = markers::MarkerRoot::at(data_root)
        .client_domain_state_directory()
        .join(format!("{domain_id}.json"));
    let Some(marker) = markers::read_json::<DomainMarker>(&path)? else {
        return Ok(0);
    };
    if marker.schema_version != DOMAIN_MARKER_SCHEMA || marker.domain_id != domain_id {
        return Err(COMMIT_UNSUPPORTED);
    }
    Ok(marker.authoritative_schema_version)
}

/// The version the client's frontier declares as one domain's target.
fn declared_target_version(domain_id: &str) -> ToolResult<Option<u32>> {
    let frontier = licoup_native::domain::client_state_migration::frontier_projection_struct()
        .map_err(|_| FRONTIER_UNAVAILABLE)?;
    Ok(frontier
        .domains
        .iter()
        .find(|domain| domain.domain_id == domain_id)
        .map(|domain| domain.target_schema_version))
}

/// Read the client's observed state for one domain, refusing a root the owner cannot read.
///
/// The authority keeps its three states distinct: an absent authority is version zero,
/// which is where the immutable first step starts, and a refused authority is an error
/// instead of an invented version.
fn observed_version(data_root: &Path, domain_id: &str) -> ToolResult<u32> {
    let states = licoup_native::domain::client_state_migration::domain_state_projection(data_root)
        .map_err(|_| STATE_UNAVAILABLE)?;
    match states
        .iter()
        .find(|state| state.domain_id == domain_id)
        .map(|state| &state.authority)
    {
        Some(licoup_native::domain::client_state_migration::DomainAuthority::Known { version }) => {
            Ok(*version)
        }
        Some(licoup_native::domain::client_state_migration::DomainAuthority::Absent) => Ok(0),
        Some(licoup_native::domain::client_state_migration::DomainAuthority::Refused {
            ..
        })
        | None => Err(STATE_UNAVAILABLE),
    }
}

/// The client's effective version for one projected state, or `None` when refused.
fn effective_version(
    state: &licoup_native::domain::client_state_migration::DomainStateProjection,
) -> Option<u32> {
    match &state.authority {
        licoup_native::domain::client_state_migration::DomainAuthority::Known { version } => {
            Some(*version)
        }
        licoup_native::domain::client_state_migration::DomainAuthority::Absent => Some(0),
        licoup_native::domain::client_state_migration::DomainAuthority::Refused { .. } => None,
    }
}

/// Ask the client's owner to move this root, then classify its report for one domain.
fn owner_verdict(
    commit: &mut impl FnMut(
        &Path,
    ) -> anyhow::Result<
        licoup_native::domain::client_state_migration::AdmissionResult,
    >,
    data_root: &Path,
    domain_id: &str,
    target_version: u32,
) -> ToolResult<OwnerVerdict> {
    let admission = match commit(data_root) {
        Ok(admission) => admission,
        Err(error) => {
            // The owner's refusals are already stable codes with no path or stored value in
            // them, which is exactly what this tool is allowed to report.
            return Ok(OwnerVerdict::Refused(error.to_string()));
        }
    };
    let applied = admission
        .applied_domain_ids
        .iter()
        .any(|id| id == domain_id);
    let unchanged = admission
        .skipped_domain_ids
        .iter()
        .any(|id| id == domain_id);
    let awaiting = admission
        .pending_authorization_domain_ids
        .iter()
        .any(|id| id == domain_id);

    let observed = observed_version(data_root, domain_id)?;
    if applied {
        return Ok(OwnerVerdict::Applied);
    }
    if unchanged || observed >= target_version {
        return Ok(OwnerVerdict::AlreadyCurrent);
    }
    if awaiting {
        return Ok(OwnerVerdict::PendingAuthorization);
    }
    let _ = STOPPED;
    Ok(OwnerVerdict::Unaccounted)
}

/// The observation that answers "did the resume add a record that was already there".
///
/// The client's ledger is the only document that records a conversion, so a step id that
/// appears twice in it is the concrete duplication a resume must never produce. A
/// completed run whose owner recorded nothing is not a completed run either.
fn ensure_no_record_was_duplicated(ledger: &Option<ClientLedger>) -> ToolResult<()> {
    let Some(ledger) = ledger else {
        // No client ledger after a complete run means the owner never recorded the
        // conversion, so the run is not supported as complete.
        return Err(COMMIT_UNSUPPORTED);
    };
    if !ledger.duplicated_steps().is_empty() {
        return Err(COMMIT_UNSUPPORTED);
    }
    Ok(())
}

fn build_report(
    journal: &Journal,
    steps: Vec<ResumedStep>,
    ledger: Option<&ClientLedger>,
    interrupted: bool,
) -> ResumeReport {
    let outstanding = journal.outstanding_domains();
    let pending_authorization = journal
        .pending_authorization_domains()
        .into_iter()
        .map(str::to_string)
        .collect::<Vec<_>>();
    let status = if journal.is_complete() && !interrupted {
        "completed"
    } else if !pending_authorization.is_empty() && outstanding.len() == pending_authorization.len()
    {
        "paused"
    } else if outstanding.is_empty() {
        "completed"
    } else {
        "blocked"
    };

    ResumeReport {
        status,
        target_version: journal.target_version.clone(),
        frontier_id: journal.frontier_id.clone(),
        steps,
        still_owed: outstanding.into_iter().map(str::to_string).collect(),
        pending_authorization,
        converted_domains: journal.committed_count(),
        declared_domains: journal.domains.len(),
        ledger_records: ledger.map_or(0, ClientLedger::recorded_steps),
    }
}

fn no_op_report(data_root: &Path) -> ToolResult<ResumeReport> {
    let frontier = licoup_native::domain::client_state_migration::frontier_projection_struct()
        .map_err(|_| FRONTIER_UNAVAILABLE)?;
    let states = licoup_native::domain::client_state_migration::domain_state_projection(data_root)
        .map_err(|_| STATE_UNAVAILABLE)?;
    let ledger = LedgerSnapshot::read(data_root)?.parse()?;
    Ok(ResumeReport {
        status: "noOp",
        target_version: states
            .iter()
            .map(|state| state.target_schema_version)
            .max()
            .map(|version| version.to_string())
            .unwrap_or_default(),
        frontier_id: frontier.frontier_id,
        steps: Vec::new(),
        still_owed: states
            .iter()
            .filter(|state| effective_version(state) != Some(state.target_schema_version))
            .map(|state| state.domain_id.clone())
            .collect(),
        pending_authorization: Vec::new(),
        converted_domains: 0,
        declared_domains: 0,
        ledger_records: ledger.map_or(0, |ledger| ledger.recorded_steps()),
    })
}

/// The report for a root whose recorded run already settled.
fn already_current_report(
    data_root: &Path,
    journal: &Journal,
    commit: &mut impl FnMut(
        &Path,
    ) -> anyhow::Result<
        licoup_native::domain::client_state_migration::AdmissionResult,
    >,
) -> ToolResult<ResumeReport> {
    let ledger = LedgerSnapshot::read(data_root)?.parse()?;
    let mut steps = Vec::new();
    for (domain_id, entry) in &journal.domains {
        // The client's own marker is the durable record of the commit, so it is what says
        // whether the finished run's result is still there. A marker that is missing or
        // behind is the one case where asking the owner again is legitimate: the owner
        // decides what the root actually needs, and it is the only writer that may put the
        // record back.
        let recorded = marker_version(data_root, domain_id)?;
        if recorded < entry.target_version() {
            let verdict = owner_verdict(commit, data_root, domain_id, entry.target_version())?;
            // The owner is the only writer that may restore the record, so what backs its
            // report is the marker it just wrote.
            let restored = marker_version(data_root, domain_id)?;
            if !matches!(
                verdict,
                OwnerVerdict::Applied | OwnerVerdict::AlreadyCurrent
            ) || restored < entry.target_version()
            {
                journal::mark_pending(data_root, domain_id)?;
                steps.push(ResumedStep {
                    domain_id: domain_id.clone(),
                    step_id: entry.step_id.clone(),
                    outcome: "stillOwed",
                    target_version: entry.target_version(),
                    observed_version: restored,
                    committed_version: None,
                    settled_before_resume: true,
                    refused: None,
                });
                continue;
            }
        }
        steps.push(ResumedStep {
            domain_id: domain_id.clone(),
            step_id: entry.step_id.clone(),
            outcome: "alreadyCurrent",
            target_version: entry.target_version(),
            observed_version: recorded,
            committed_version: entry.committed_version,
            settled_before_resume: true,
            refused: None,
        });
    }

    let settled = steps.iter().all(|step| step.committed_version.is_some());
    Ok(ResumeReport {
        status: if settled { "alreadyCurrent" } else { "blocked" },
        target_version: journal.target_version.clone(),
        frontier_id: journal.frontier_id.clone(),
        steps,
        still_owed: journal
            .outstanding_domains()
            .into_iter()
            .map(str::to_string)
            .collect(),
        pending_authorization: Vec::new(),
        converted_domains: journal.committed_count(),
        declared_domains: journal.domains.len(),
        ledger_records: ledger.map_or(0, |ledger| ledger.recorded_steps()),
    })
}

/// The steps one root still owes, from the client's own projections.
///
/// A conversion uses this to write the journal it will resume from; a resume does not,
/// because its steps are the ones the interrupted run already declared.
pub fn plan_for(data_root: &Path) -> ToolResult<JournalPlan> {
    let frontier = licoup_native::domain::client_state_migration::frontier_projection_struct()
        .map_err(|_| FRONTIER_UNAVAILABLE)?;
    let states = licoup_native::domain::client_state_migration::domain_state_projection(data_root)
        .map_err(|_| STATE_UNAVAILABLE)?;
    let running = licoup_native::domain::client_state_migration::running_product_version()
        .map_err(|_| FRONTIER_UNAVAILABLE)?
        .to_string();
    Ok(journal::plan_from_projections(&frontier, &states, &running))
}

/// The path of the run journal for one data root.
pub fn journal_path(data_root: &Path) -> std::path::PathBuf {
    markers::MarkerRoot::at(data_root).journal_path()
}
