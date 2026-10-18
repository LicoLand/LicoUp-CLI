//! Run the client's own conversion and report exactly what it concluded.
//!
//! This module holds no conversion algorithm. It asks the client's migration owner to
//! convert one data root, and it reports the appearance domain's outcome in the
//! package's own vocabulary. Two rules shape it:
//!
//! - **Refuse before writing.** The client's read-only projection answers for the
//!   appearance domain first. A store shape this client refuses — a document written by
//!   a format no release published — is reported with the client's own stable code and
//!   the run stops there, before the admission is asked to open anything. The source
//!   document is left exactly as it was.
//! - **Never infer completion.** The status is derived from the client's own verdict. A
//!   domain the owner did not account for is reported as a refusal, not as a success,
//!   and a run that leaves the domain owed exits non-zero.

use anyhow::Result;
use licoup_native::domain::client_state_migration::{
    AdmissionResult, DomainAuthority, DomainStateProjection, admit, domain_state_projection,
};
use serde::Serialize;
use std::path::Path;

use crate::{DOMAIN_ID, PACKAGE_ID, REPORT_SCHEMA};

/// The report a run that could not be started carries.
///
/// The word is the standalone tool's own vocabulary for the same fact: the client's
/// owners could not report the observed state.
pub const STATE_UNAVAILABLE: &str = "migration_state_unavailable";

/// The client's own code for a step that did not complete.
pub const STEP_FAILED: &str = "migration_step_failed";

/// Where a refusal was decided.
///
/// The stage is part of the report because "refused before writing" and "the owner
/// refused mid-run" are different facts about a root, and a caller that cannot tell them
/// apart cannot tell a preserved source from a half-finished one.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum Stage {
    /// The client's read-only probe refused the appearance store; nothing was opened.
    Probe,
    /// The client's admission refused the root; the domain was not moved.
    Owner,
}

impl Stage {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Probe => "probe",
            Self::Owner => "owner",
        }
    }
}

/// What one conversion run concluded about the appearance domain.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum Status {
    /// The client's owner moved the appearance domain to its target.
    Converted,
    /// The client's owner reported the domain already at its target.
    ///
    /// This is also the outcome of a resumed interruption: a store that committed
    /// before the process stopped is authoritative, so the resume reconciles the
    /// completed step instead of applying it a second time.
    AlreadyCurrent,
    /// The run did not move the domain and says why.
    Refused,
    /// The client's owners could not report the state at all.
    Unavailable,
}

impl Status {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Converted => "converted",
            Self::AlreadyCurrent => "already-current",
            Self::Refused => "refused",
            Self::Unavailable => "unavailable",
        }
    }

    /// Whether the appearance domain is at its target after this run.
    pub const fn is_settled(self) -> bool {
        matches!(self, Self::Converted | Self::AlreadyCurrent)
    }
}

/// The one JSON report an invocation prints.
///
/// Every field is either a published identity from the client's catalogue or a stable
/// code. No local path, no stored value and no free-form message crosses this boundary.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Report {
    pub schema_version: &'static str,
    pub package_id: &'static str,
    pub domain_id: &'static str,
    pub source_format: String,
    pub target_format: String,
    pub status: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub code: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stage: Option<&'static str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub running_product_version: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub frontier_id: Option<String>,
}

impl Report {
    /// The process exit code for this report.
    ///
    /// A run that leaves the domain owed never exits zero, so a caller scripting the
    /// converter cannot read a refusal as a completed conversion.
    pub fn exit_code(&self) -> i32 {
        match self.status {
            "converted" | "already-current" => 0,
            "refused" => 3,
            _ => 4,
        }
    }

    /// The report as one JSON document.
    pub fn to_json(&self) -> Result<String> {
        let mut document = serde_json::to_string_pretty(self)?;
        document.push('\n');
        Ok(document)
    }
}

/// Ask the client's own migration owner to convert `data_root`.
///
/// The caller is the migration coordinator: it has already stopped the writers and holds
/// the admission guarantee. This function adds no second lock and no second journal — it
/// reads the client's projection, runs the client's admission and classifies the result.
pub fn convert(data_root: &Path) -> Report {
    let endpoints = match crate::declaration::endpoints() {
        Ok(endpoints) => endpoints,
        Err(_) => {
            return unavailable(
                String::new(),
                String::new(),
                "migration_frontier_incomplete",
            );
        }
    };
    let source = endpoints.source_format().to_owned();
    let target = endpoints.target_format().to_owned();

    // The client's own read-only projection is asked first, so an unsupported source is
    // refused with the client's stable code before any file is opened for writing.
    match appearance_authority(data_root) {
        Ok(Some(DomainAuthority::Refused { code })) => {
            return refused(source, target, code, Stage::Probe);
        }
        Ok(Some(_)) => {}
        Ok(None) => {
            return refused(
                source,
                target,
                "migration_frontier_incomplete".to_owned(),
                Stage::Probe,
            );
        }
        Err(_) => return unavailable(source, target, STATE_UNAVAILABLE),
    }

    match admit(data_root) {
        Ok(admission) => classify(source, target, &admission, data_root),
        Err(error) => refused(
            source,
            target,
            stable_code(&error.to_string()),
            Stage::Owner,
        ),
    }
}

/// The client's own authority for the appearance domain.
fn appearance_authority(data_root: &Path) -> Result<Option<DomainAuthority>> {
    let projected: Vec<DomainStateProjection> = domain_state_projection(data_root)?;
    Ok(projected
        .into_iter()
        .find(|domain| domain.domain_id == DOMAIN_ID)
        .map(|domain| domain.authority))
}

/// Turn the client's admission result into this package's report.
fn classify(
    source: String,
    target: String,
    admission: &AdmissionResult,
    data_root: &Path,
) -> Report {
    let settled = |ids: &[String]| ids.iter().any(|id| id == DOMAIN_ID);
    let report = |status: Status, code: Option<String>, stage: Option<Stage>| Report {
        schema_version: REPORT_SCHEMA,
        package_id: PACKAGE_ID,
        domain_id: DOMAIN_ID,
        source_format: source.clone(),
        target_format: target.clone(),
        status: status.as_str(),
        code,
        stage: stage.map(Stage::as_str),
        running_product_version: Some(admission.running_product_version.clone()),
        frontier_id: Some(admission.frontier_id.clone()),
    };

    if settled(&admission.applied_domain_ids) {
        return report(Status::Converted, None, None);
    }
    if settled(&admission.skipped_domain_ids) {
        return report(Status::AlreadyCurrent, None, None);
    }
    if settled(&admission.unavailable_feature_domain_ids) {
        // An optional store the client could not interpret is not replaced by a default
        // and is not a conversion. The projection carries the client's own code for it.
        let code = appearance_authority(data_root)
            .ok()
            .flatten()
            .and_then(|authority| match authority {
                DomainAuthority::Refused { code } => Some(code),
                _ => None,
            })
            .unwrap_or_else(|| STEP_FAILED.to_owned());
        return report(Status::Refused, Some(code), Some(Stage::Owner));
    }
    // The owner did not account for the domain it was asked about. It stays owed.
    report(
        Status::Refused,
        Some(STEP_FAILED.to_owned()),
        Some(Stage::Owner),
    )
}

fn refused(source: String, target: String, code: String, stage: Stage) -> Report {
    Report {
        schema_version: REPORT_SCHEMA,
        package_id: PACKAGE_ID,
        domain_id: DOMAIN_ID,
        source_format: source,
        target_format: target,
        status: Status::Refused.as_str(),
        code: Some(code),
        stage: Some(stage.as_str()),
        running_product_version: None,
        frontier_id: None,
    }
}

fn unavailable(source: String, target: String, code: &str) -> Report {
    Report {
        schema_version: REPORT_SCHEMA,
        package_id: PACKAGE_ID,
        domain_id: DOMAIN_ID,
        source_format: source,
        target_format: target,
        status: Status::Unavailable.as_str(),
        code: Some(code.to_owned()),
        stage: None,
        running_product_version: None,
        frontier_id: None,
    }
}

/// The client's stable codes, so a report never carries text that is not one of them.
const ADMISSION_CODES: &[&str] = &[
    "migration_lock_unavailable",
    "migration_ledger_invalid",
    "state_newer_than_binary",
    "migration_frontier_incomplete",
    "migration_step_failed",
    "migration_postcondition_failed",
    "update_handoff_mismatch",
    "unsupported_state_shape",
];

/// One stable code, or the client's catch-all when the text is not in the vocabulary.
///
/// The admission already reduces its failures to privacy-safe codes; this keeps that
/// property true here as well, so an unexpected error can never carry a path into a
/// report.
fn stable_code(text: &str) -> String {
    let trimmed = text.trim();
    ADMISSION_CODES
        .iter()
        .copied()
        .find(|code| *code == trimmed)
        .unwrap_or(STEP_FAILED)
        .to_owned()
}
