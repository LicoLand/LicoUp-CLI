//! Current state of one data root, as the client's own owners report it.
//!
//! Nothing here is derived by this tool: the domain list, the target versions and every
//! observed authority come from the client's migration owner. Reporting a locally invented
//! version would make the tool a second authority, which is exactly what the delivery
//! forbids.
//!
//! The owner reports one authority per domain and keeps its states distinct: a domain whose
//! authority cannot be read is reported as refused with its own stable code while every
//! sibling domain keeps its observation. This read preserves that shape instead of
//! flattening a refusal into version zero, so an unreadable or ahead root stays
//! distinguishable from an absent one.

use crate::error::{
    DATA_ROOT_MISSING, DATA_ROOT_NOT_DIRECTORY, FRONTIER_UNAVAILABLE, STATE_UNAVAILABLE, ToolResult,
};
use licoup_native::domain::client_state_migration::DomainAuthority;
use serde::Serialize;
use std::path::Path;

/// One domain, as the client reports it.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DomainReport {
    pub domain_id: String,
    /// The client's own authority for this domain: absent, known at a version, or
    /// refused with the owner's stable code.
    pub authority: DomainAuthority,
    /// The marker's version when a marker exists.
    pub marker_schema_version: Option<u32>,
    /// The version the next migration edge moves from, when the authority is readable.
    /// An absent authority means version zero, which is what the immutable first step
    /// moves from.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub effective_version: Option<u32>,
    /// The version the client's frontier declares as the current target.
    pub target_schema_version: u32,
    /// Whether the domain is already at its target.
    pub at_target: bool,
}

/// One domain the client's owner refused to read, with the owner's own code.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RefusedDomain {
    pub domain_id: String,
    pub code: String,
}

impl RefusedDomain {
    /// Every domain whose authority refused, in the client's own order.
    pub fn collect(
        states: &[licoup_native::domain::client_state_migration::DomainStateProjection],
    ) -> Vec<Self> {
        states
            .iter()
            .filter_map(|state| match &state.authority {
                DomainAuthority::Refused { code } => Some(Self {
                    domain_id: state.domain_id.clone(),
                    code: code.clone(),
                }),
                DomainAuthority::Absent | DomainAuthority::Known { .. } => None,
            })
            .collect()
    }
}

/// The whole observed state of one root.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct InspectReport {
    pub status: &'static str,
    pub frontier_id: String,
    pub domains: Vec<DomainReport>,
    /// Domains whose stored authority the client's owner refused to read; a non-empty
    /// list is unfinished work, never a healthy read.
    pub refused: Vec<RefusedDomain>,
    /// Domains whose stored state is ahead of this tool; a non-empty list is a refusal
    /// that must not be planned or converted.
    pub newer_than_tool: Vec<String>,
}

/// Read one root through the client's own migration owner.
pub fn inspect(data_root: &Path) -> ToolResult<InspectReport> {
    if !data_root.exists() {
        return Err(DATA_ROOT_MISSING);
    }
    if !data_root.is_dir() {
        return Err(DATA_ROOT_NOT_DIRECTORY);
    }
    let frontier = licoup_native::domain::client_state_migration::frontier_projection_struct()
        .map_err(|_| FRONTIER_UNAVAILABLE)?;
    let states = licoup_native::domain::client_state_migration::domain_state_projection(data_root)
        .map_err(|_| STATE_UNAVAILABLE)?;

    let targets: std::collections::BTreeMap<&str, u32> = frontier
        .domains
        .iter()
        .map(|domain| (domain.domain_id.as_str(), domain.target_schema_version))
        .collect();

    let mut domains = Vec::with_capacity(states.len());
    let mut newer_than_tool = Vec::new();
    for state in &states {
        let Some(target) = targets.get(state.domain_id.as_str()).copied() else {
            continue;
        };
        match &state.authority {
            DomainAuthority::Refused { code } => {
                if code == "state_newer_than_binary" {
                    newer_than_tool.push(state.domain_id.clone());
                }
            }
            DomainAuthority::Absent | DomainAuthority::Known { .. } => {}
        }
        let effective_version = match &state.authority {
            DomainAuthority::Known { version } => Some(*version),
            DomainAuthority::Absent => Some(0),
            DomainAuthority::Refused { .. } => None,
        };
        domains.push(DomainReport {
            domain_id: state.domain_id.clone(),
            authority: state.authority.clone(),
            marker_schema_version: state.marker_schema_version,
            effective_version,
            target_schema_version: target,
            at_target: effective_version == Some(target),
        });
    }
    domains.sort_by(|left, right| left.domain_id.cmp(&right.domain_id));
    let refused = RefusedDomain::collect(&states);

    Ok(InspectReport {
        status: "inspected",
        frontier_id: frontier.frontier_id,
        domains,
        refused,
        newer_than_tool,
    })
}

/// The domains that still owe work on this root.
pub fn outstanding(report: &InspectReport) -> Vec<&DomainReport> {
    report
        .domains
        .iter()
        .filter(|domain| !domain.at_target)
        .collect()
}
