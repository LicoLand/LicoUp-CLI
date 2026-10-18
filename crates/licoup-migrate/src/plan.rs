//! The conversion plan for one root.
//!
//! A plan names, per domain, the steps the client's frontier declares between the
//! observed version and the target. The steps are the client's own edge list; this tool
//! adds nothing to it. A step whose move only the client's owner can perform is marked
//! as such instead of being silently claimed here, and a domain whose authority the
//! client refuses is reported as blocked instead of being planned from an invented
//! version.

use crate::error::{FRONTIER_UNAVAILABLE, STATE_UNAVAILABLE, TARGET_UNSUPPORTED, ToolResult};
use crate::inspect::{InspectReport, RefusedDomain, inspect};
use serde::Serialize;
use std::collections::BTreeMap;
use std::path::Path;

/// One declared edge of the client's frontier.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PlannedStep {
    pub step_id: String,
    pub from_schema_version: u32,
    pub to_schema_version: u32,
    /// True when the move belongs to the client's own owner rather than to this tool.
    pub owner_only: bool,
}

/// The steps one domain owes.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DomainPlan {
    pub domain_id: String,
    pub from_version: u32,
    pub to_version: u32,
    pub steps: Vec<PlannedStep>,
}

/// The whole plan for one root.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PlanReport {
    pub status: &'static str,
    pub frontier_id: String,
    /// Domains already at their target.
    pub already_current: Vec<String>,
    pub domains: Vec<DomainPlan>,
    /// Domains whose authority the client's owner refused to read. A blocked domain is
    /// never planned from an invented version and always leaves the run unfinished.
    pub blocked: Vec<RefusedDomain>,
}

/// Domains whose conversion only the client's own owner may perform.
///
/// The list is not a second catalog: it names the domains whose published format is
/// produced by a writer that carries typed semantics this tool does not have.
const OWNER_ONLY_DOMAINS: &[&str] = &["canonical-conversation", "adaptive-flywheel"];

/// Build the plan for one root against the client's declared target.
pub fn plan(data_root: &Path, target: Option<&str>) -> ToolResult<PlanReport> {
    let report: InspectReport = inspect(data_root)?;

    if let Some(named) = target {
        let known = report.domains.iter().any(|domain| {
            domain.domain_id == named || domain.target_schema_version.to_string() == named
        });
        if !known && named != "latest" {
            return Err(TARGET_UNSUPPORTED);
        }
    }

    let frontier = licoup_native::domain::client_state_migration::frontier_projection_struct()
        .map_err(|_| FRONTIER_UNAVAILABLE)?;
    let edges: BTreeMap<
        &str,
        &Vec<licoup_native::domain::client_state_migration::FrontierStepProjection>,
    > = frontier
        .domains
        .iter()
        .map(|domain| (domain.domain_id.as_str(), &domain.steps))
        .collect();

    let mut already_current = Vec::new();
    let mut domains = Vec::new();
    for domain in &report.domains {
        if domain.at_target {
            already_current.push(domain.domain_id.clone());
            continue;
        }
        let Some(from_version) = domain.effective_version else {
            // A refused authority is blocked, never planned from version zero.
            continue;
        };
        let Some(steps) = edges.get(domain.domain_id.as_str()) else {
            continue;
        };
        let owner_only = OWNER_ONLY_DOMAINS.contains(&domain.domain_id.as_str());
        let planned = steps
            .iter()
            .filter(|step| {
                step.from_schema_version >= from_version
                    && step.to_schema_version <= domain.target_schema_version
            })
            .map(|step| PlannedStep {
                step_id: step.step_id.clone(),
                from_schema_version: step.from_schema_version,
                to_schema_version: step.to_schema_version,
                owner_only,
            })
            .collect::<Vec<_>>();
        if planned.is_empty() {
            continue;
        }
        domains.push(DomainPlan {
            domain_id: domain.domain_id.clone(),
            from_version,
            to_version: domain.target_schema_version,
            steps: planned,
        });
    }

    Ok(PlanReport {
        status: "planned",
        frontier_id: report.frontier_id,
        already_current,
        domains,
        blocked: report.refused,
    })
}

/// The domains in this plan whose move the client's owner must perform.
pub fn owner_only_domains(report: &PlanReport) -> Vec<&str> {
    report
        .domains
        .iter()
        .filter(|domain| domain.steps.iter().any(|step| step.owner_only))
        .map(|domain| domain.domain_id.as_str())
        .collect()
}

/// Reject a plan whose read failed, so a caller never reports success on a failed read.
pub fn ensure_readable(report: &PlanReport) -> ToolResult<()> {
    if report.frontier_id.is_empty() {
        return Err(STATE_UNAVAILABLE);
    }
    Ok(())
}
