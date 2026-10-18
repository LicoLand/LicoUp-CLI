//! The durable conversion journal.
//!
//! A conversion moves a data root through the client's own owner. The owner is
//! all-or-nothing per domain and is safe to run again, but a process can stop between two
//! of its moves, and a user who is told "conversion failed" needs to know what actually
//! happened to their data. This module is that record.
//!
//! The journal is written *before* each move is attempted and rewritten *after* the
//! owner reports the result, so at every instant the document on disk says either "this
//! step had not been attempted" or "this step was committed". There is no state in which
//! it claims work the owner never did, and no state in which a completed move is
//! invisible: the two writes bracket the commit, and the second one is what a resume
//! reads to decide whether a step still owes anything.
//!
//! It holds no schema authority. Every version and step id in it came from the client's
//! frontier projection, and the only thing this module decides is which marker to write
//! next. A resume therefore compares the journal against the client's *observed* state
//! rather than trusting the document: the journal says what was attempted, the store
//! says what happened, and a disagreement is a refusal, never a repair.

pub mod ledger;
pub mod markers;

pub use ledger::LedgerSnapshot;
pub use markers::{JOURNAL_FILE, LEDGER_FILE, MarkerRoot};

use crate::error::{COMMIT_UNSUPPORTED, JOURNAL_MISMATCHED, ToolResult, marker_invalid};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::Path;

/// The journal document's schema identity.
pub const JOURNAL_SCHEMA: &str = "v0.0.1:data-migration-journal-1";

/// The direction one run moves the root.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum RunDirection {
    Forward,
    Reverse,
    Mixed,
}

/// One domain step's durable state.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum StepStatus {
    /// Declared, not attempted.
    Pending,
    /// The owner was asked for this step; the process may have stopped inside it.
    Running,
    /// The owner reported the step applied, or reported it already current.
    Committed,
    /// The owner cannot complete this step without platform authority the tool lacks.
    PendingAuthorization,
}

impl StepStatus {
    /// Whether the step's outcome is settled.
    pub const fn is_settled(self) -> bool {
        matches!(self, Self::Committed)
    }

    /// The reason a step was left unsettled, as the journal stores it.
    pub const fn pending_reason(self) -> Option<&'static str> {
        match self {
            Self::PendingAuthorization => Some("migration_authorization_required"),
            Self::Pending | Self::Running | Self::Committed => None,
        }
    }
}

/// One domain's durable entry.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StepEntry {
    /// The version this step moves toward, as the client's frontier declares it.
    pub target_version: u32,
    /// The version the step was declared from.
    pub from_version: u32,
    /// The frontier step this move belongs to.
    pub step_id: String,
    pub status: StepStatus,
    /// Why an unsettled step is unsettled.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pending_reason: Option<String>,
    /// The version the owner reported at commit.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub committed_version: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub committed_at: Option<String>,
}

impl StepEntry {
    /// The version this entry is finished at.
    pub fn target_version(&self) -> u32 {
        self.committed_version.unwrap_or(self.target_version)
    }
}

/// One declared step to record before any move is attempted.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct StepRequest {
    pub domain_id: String,
    /// The version the domain stood at when the run was planned.
    pub from_version: u32,
    /// The version this run moves it to.
    pub target_version: u32,
    /// The frontier step this move belongs to.
    pub step_id: String,
}

/// Everything one run needs before it touches a store.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct JournalPlan {
    pub frontier_id: String,
    /// The product version the owner reported as running.
    pub target_version: String,
    pub steps: Vec<StepRequest>,
}

/// The durable journal document.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Journal {
    pub schema_version: String,
    pub status: String,
    pub started_at: String,
    pub updated_at: String,
    pub target_version: String,
    pub frontier_id: String,
    pub direction: RunDirection,
    /// The version the previous settled run reached, so a resume can tell that it is
    /// continuing the same run instead of a second one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub converted_version: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub completed_at: Option<String>,
    pub domains: BTreeMap<String, StepEntry>,
}

impl Journal {
    /// One domain's entry.
    pub fn step(&self, domain_id: &str) -> Option<&StepEntry> {
        self.domains.get(domain_id)
    }

    /// The next step that still owes work, in the order the frontier declared it.
    ///
    /// A resume takes exactly one step at a time; the caller runs this to completion, so
    /// the position of the step that stopped the run is visible after every attempt.
    pub fn next_step(&self) -> Option<(&str, &StepEntry)> {
        self.domains
            .iter()
            .find(|(_, entry)| {
                !entry.status.is_settled() && entry.status != StepStatus::PendingAuthorization
            })
            .map(|(domain_id, entry)| (domain_id.as_str(), entry))
    }

    /// Every domain that still owes work.
    pub fn outstanding_domains(&self) -> Vec<&str> {
        self.domains
            .iter()
            .filter(|(_, entry)| !entry.status.is_settled())
            .map(|(domain_id, _)| domain_id.as_str())
            .collect()
    }

    /// Every domain whose step the owner cannot finish here.
    pub fn pending_authorization_domains(&self) -> Vec<&str> {
        self.domains
            .iter()
            .filter(|(_, entry)| entry.status == StepStatus::PendingAuthorization)
            .map(|(domain_id, _)| domain_id.as_str())
            .collect()
    }

    /// How many domains have settled.
    pub fn committed_count(&self) -> usize {
        self.domains
            .values()
            .filter(|entry| entry.status.is_settled())
            .count()
    }

    /// Whether every declared step has settled.
    pub fn is_complete(&self) -> bool {
        !self.domains.is_empty() && self.outstanding_domains().is_empty()
    }

    /// Whether this journal belongs to the same run as `plan`.
    pub fn matches(&self, plan: &JournalPlan) -> bool {
        self.frontier_id == plan.frontier_id
    }
}

/// The journal this tool writes for one run.
pub fn open(data_root: &Path) -> ToolResult<Option<Journal>> {
    let path = MarkerRoot::at(data_root).journal_path();
    let Some(value) = markers::read_json::<serde_json::Value>(&path)? else {
        return Ok(None);
    };
    let document =
        serde_json::from_value::<Journal>(value).map_err(|_| marker_invalid("journal"))?;
    validate(&document)?;
    Ok(Some(document))
}

/// Refuse a journal this binary must not act on.
fn validate(document: &Journal) -> ToolResult<()> {
    if document.schema_version != JOURNAL_SCHEMA {
        return Err(JOURNAL_MISMATCHED);
    }
    if document.frontier_id.is_empty() {
        return Err(marker_invalid("journal"));
    }
    Ok(())
}

/// Refuse a journal whose run is not the run the caller is continuing.
pub fn ensure_matches(document: &Journal, plan: &JournalPlan) -> ToolResult<()> {
    if !document.matches(plan) {
        return Err(JOURNAL_MISMATCHED);
    }
    Ok(())
}

/// Whether this journal is the record of a completed run.
///
/// The document stays on disk after a successful conversion: it is the evidence that the
/// run finished, and it is what makes a repeated attempt answer "already current" instead
/// of starting a second conversion.
pub fn is_finished(document: &Journal) -> bool {
    document.status == "complete"
}

/// Write a fresh journal for one run, replacing any settled journal for the same root.
pub fn initialize(data_root: &Path, plan: &JournalPlan) -> ToolResult<Journal> {
    let now = timestamp();
    let direction = run_direction(plan);
    let mut domains = BTreeMap::new();
    for step in &plan.steps {
        domains.insert(
            step.domain_id.clone(),
            StepEntry {
                target_version: step.target_version,
                from_version: step.from_version,
                step_id: step.step_id.clone(),
                status: StepStatus::Pending,
                pending_reason: None,
                committed_version: None,
                committed_at: None,
            },
        );
    }
    let document = Journal {
        schema_version: JOURNAL_SCHEMA.to_string(),
        status: "inProgress".to_string(),
        started_at: now.clone(),
        updated_at: now,
        target_version: plan.target_version.clone(),
        frontier_id: plan.frontier_id.clone(),
        direction,
        converted_version: None,
        completed_at: None,
        domains,
    };
    markers::write_json(&MarkerRoot::at(data_root).journal_path(), &document)?;
    Ok(document)
}

/// Record that a step's move is about to be attempted.
pub fn mark_running(data_root: &Path, domain_id: &str, step_id: &str) -> ToolResult<Journal> {
    mutate(data_root, |document| {
        if let Some(entry) = document.domains.get_mut(domain_id) {
            entry.status = StepStatus::Running;
            entry.step_id = step_id.to_string();
            entry.pending_reason = None;
        }
        Ok(())
    })
}

/// Record that the owner reported a step applied, or already current.
pub fn mark_committed(
    data_root: &Path,
    domain_id: &str,
    committed_version: u32,
) -> ToolResult<Journal> {
    mutate(data_root, |document| {
        if let Some(entry) = document.domains.get_mut(domain_id) {
            entry.status = StepStatus::Committed;
            entry.committed_version = Some(committed_version);
            entry.committed_at = Some(timestamp());
            entry.pending_reason = None;
        }
        Ok(())
    })
}

/// Record that a step is waiting for authority the tool does not hold.
pub fn mark_pending_authorization(data_root: &Path, domain_id: &str) -> ToolResult<Journal> {
    mutate(data_root, |document| {
        if let Some(entry) = document.domains.get_mut(domain_id) {
            entry.status = StepStatus::PendingAuthorization;
            entry.pending_reason = StepStatus::PendingAuthorization
                .pending_reason()
                .map(str::to_string);
        }
        Ok(())
    })
}

/// Record that a step is owed again after a refusal.
pub fn mark_pending(data_root: &Path, domain_id: &str) -> ToolResult<Journal> {
    mutate(data_root, |document| {
        if let Some(entry) = document.domains.get_mut(domain_id) {
            entry.status = StepStatus::Pending;
            entry.pending_reason = None;
        }
        Ok(())
    })
}

/// Record the version a settled run reached, so a later resume continues the same run.
pub fn mark_converted_version(data_root: &Path, version: &str) -> ToolResult<Journal> {
    mutate(data_root, |document| {
        document.converted_version = Some(version.to_string());
        Ok(())
    })
}

/// Close the journal: every step settled, the run over.
pub fn finish(data_root: &Path) -> ToolResult<Journal> {
    mutate(data_root, |document| {
        if !document.is_complete() {
            return Err(COMMIT_UNSUPPORTED);
        }
        document.status = "complete".to_string();
        document.completed_at = Some(timestamp());
        Ok(())
    })
}

/// Discard a journal, for a run that must leave no record behind.
pub fn remove(data_root: &Path) -> ToolResult<bool> {
    markers::remove(&MarkerRoot::at(data_root).journal_path())
}

fn mutate(
    data_root: &Path,
    edit: impl FnOnce(&mut Journal) -> ToolResult<()>,
) -> ToolResult<Journal> {
    let path = MarkerRoot::at(data_root).journal_path();
    let Some(mut document) = open(data_root)? else {
        return Err(marker_invalid("journal"));
    };
    edit(&mut document)?;
    document.updated_at = timestamp();
    markers::write_json(&path, &document)?;
    Ok(document)
}

fn run_direction(plan: &JournalPlan) -> RunDirection {
    let mut forward = false;
    let mut reverse = false;
    for step in &plan.steps {
        match step.target_version.cmp(&step.from_version) {
            std::cmp::Ordering::Greater => forward = true,
            std::cmp::Ordering::Less => reverse = true,
            std::cmp::Ordering::Equal => {}
        }
    }
    match (forward, reverse) {
        (true, true) => RunDirection::Mixed,
        (true, false) => RunDirection::Forward,
        (false, true) => RunDirection::Reverse,
        (false, false) => RunDirection::Forward,
    }
}

/// One instant, rendered so that two entries written in the same run still order.
fn timestamp() -> String {
    match std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH) {
        Ok(elapsed) => format!("{}.{:09}", elapsed.as_secs(), elapsed.subsec_nanos()),
        Err(_) => "0.000000000".to_string(),
    }
}

/// Build one run's step list from the client's own projections.
///
/// Nothing here is derived: the declared steps are the frontier's, the version each step
/// starts from is the observed one, and the version it moves to is the frontier's target.
/// A domain the client already observes at its target contributes no step, because the
/// owner would report it unchanged and a journalled step for it would be this tool
/// claiming work that was never owed.
pub fn plan_from_projections(
    frontier: &licoup_native::domain::client_state_migration::FrontierProjection,
    observed: &[licoup_native::domain::client_state_migration::DomainStateProjection],
    target_version: &str,
) -> JournalPlan {
    // An absent authority is version zero, which is where the immutable first step
    // starts. A refused authority contributes no step: a run must not be planned from a
    // version the client's owner declined to report.
    let observed_versions: BTreeMap<&str, u32> = observed
        .iter()
        .filter_map(|state| match &state.authority {
            licoup_native::domain::client_state_migration::DomainAuthority::Known { version } => {
                Some((state.domain_id.as_str(), *version))
            }
            licoup_native::domain::client_state_migration::DomainAuthority::Absent => {
                Some((state.domain_id.as_str(), 0))
            }
            licoup_native::domain::client_state_migration::DomainAuthority::Refused { .. } => None,
        })
        .collect();

    let mut steps = Vec::new();
    for domain in &frontier.domains {
        let Some(observed_version) = observed_versions.get(domain.domain_id.as_str()).copied()
        else {
            continue;
        };
        if observed_version == domain.target_schema_version {
            continue;
        }
        let step_id = domain
            .steps
            .iter()
            .find(|step| step.from_schema_version >= observed_version)
            .map(|step| step.step_id.clone())
            .unwrap_or_else(|| {
                format!(
                    "{}.absent-to-{}",
                    domain.domain_id, domain.target_schema_version
                )
            });
        steps.push(StepRequest {
            domain_id: domain.domain_id.clone(),
            from_version: observed_version,
            target_version: domain.target_schema_version,
            step_id,
        });
    }
    steps.sort_by(|left, right| left.domain_id.cmp(&right.domain_id));

    JournalPlan {
        frontier_id: frontier.frontier_id.clone(),
        target_version: target_version.to_string(),
        steps,
    }
}

#[cfg(test)]
pub(crate) mod test_support {
    use std::path::{Path, PathBuf};

    /// The repository root, from the crate location the compiler recorded.
    ///
    /// Test cases keep their disposable roots under the repository's ignored `build/`
    /// directory rather than in the system temporary directory, so a run leaves nothing
    /// outside the checkout and two runs of the same case start from the same state.
    pub(crate) fn repository_root() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .and_then(Path::parent)
            .expect("the crate lives two levels below the repository root")
            .to_path_buf()
    }

    /// A disposable root under `build/tmp`, cleared before use.
    pub(crate) fn scratch(name: &str) -> PathBuf {
        let root = repository_root()
            .join("build/tmp/licoup-migrate-resume")
            .join(name);
        if root.exists() {
            std::fs::remove_dir_all(&root).expect("clear the disposable root");
        }
        std::fs::create_dir_all(&root).expect("create the disposable root");
        root
    }
}
