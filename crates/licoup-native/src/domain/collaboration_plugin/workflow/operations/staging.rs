use std::path::Path;

use anyhow::{Result, anyhow, ensure};
use serde_json::Value;

use crate::platform::client_state::ClientStateStore;

use super::super::super::package::SelectedPayloadFile;
use super::super::super::registration::{AgentDestination, PlannedAgentRegistration};
use super::super::commit::{
    CommitOptions, CommitUnit, cleanup_staged, commit_all, stage_payload,
    stage_private_registration,
};
use super::super::model::sha256_hex;
use super::super::store::{ClaimedPlan, abandon_claim, finish_claim};
#[cfg(test)]
use super::validation::bool_value;

pub(super) fn stage_mcp_units(
    payload: &[SelectedPayloadFile],
    registrations: &[PlannedAgentRegistration],
    destinations: &[AgentDestination],
) -> Result<Vec<CommitUnit>> {
    let mut units = Vec::with_capacity(destinations.len() * 2);
    let result = (|| -> Result<()> {
        for destination in destinations {
            units.push(stage_payload(
                payload,
                Path::new(&destination.install_destination),
            )?);
            let registration = registrations
                .iter()
                .find(|registration| registration.agent_id == destination.agent_id)
                .ok_or_else(|| anyhow!("collaboration_mcp_registration_missing"))?;
            ensure!(
                sha256_hex(registration.content.as_bytes()) == registration.digest_sha256,
                "collaboration_mcp_registration_digest_mismatch"
            );
            units.push(stage_private_registration(
                &registration.content,
                Path::new(&registration.destination),
            )?);
        }
        Ok(())
    })();
    if let Err(error) = result {
        cleanup_staged(&units);
        return Err(error);
    }
    Ok(units)
}

pub(super) fn commit_staged_units(units: &[CommitUnit], params: &Value) -> Result<bool> {
    match commit_all(units, commit_options(params)) {
        Ok(commit) => Ok(commit.cleanup_pending),
        Err(error) => {
            cleanup_staged(units);
            Err(error)
        }
    }
}

pub(super) fn discard_staged_units(units: &[CommitUnit]) {
    cleanup_staged(units);
}

pub(super) fn settle_apply_claim(
    store: &ClientStateStore,
    claim: &ClaimedPlan,
    outcome: Result<Value>,
    simulate_cleanup_failure: bool,
) -> Result<Value> {
    match outcome {
        Ok(mut response) => {
            let plan_cleanup_pending = finish_claim(store, claim, simulate_cleanup_failure);
            response["cleanupPending"] = Value::Bool(
                response["cleanupPending"].as_bool().unwrap_or(false) || plan_cleanup_pending,
            );
            Ok(response)
        }
        Err(error) => {
            abandon_claim(store, claim);
            Err(error)
        }
    }
}

pub(super) fn settle_mcp_apply_claim(
    store: &ClientStateStore,
    claim: &ClaimedPlan,
    outcome: Result<Value>,
    simulate_cleanup_failure: bool,
) -> Result<Value> {
    if outcome.is_err()
        && super::super::mcp_transaction::pending_for_plan(store, &claim.record.plan_id)?
    {
        super::super::store::restore_claim(store, claim)?;
        return outcome;
    }
    settle_apply_claim(store, claim, outcome, simulate_cleanup_failure)
}

#[cfg(test)]
fn commit_options(params: &Value) -> CommitOptions {
    CommitOptions {
        fail_after_commits: params
            .get("failAfterCommits")
            .and_then(|value| match value {
                Value::Number(value) => value.as_u64(),
                Value::String(value) => value.parse().ok(),
                _ => None,
            })
            .and_then(|value| usize::try_from(value).ok()),
        replace_destination_before_commit: params
            .get("replaceDestinationBeforeCommitIndex")
            .and_then(|value| match value {
                Value::Number(value) => value.as_u64(),
                Value::String(value) => value.parse().ok(),
                _ => None,
            })
            .and_then(|value| usize::try_from(value).ok()),
    }
}

#[cfg(not(test))]
fn commit_options(_params: &Value) -> CommitOptions {
    CommitOptions::default()
}

#[cfg(test)]
pub(super) fn simulate_plan_cleanup_failure(params: &Value) -> bool {
    bool_value(params.get("simulateCleanupFailure")).unwrap_or(false)
}

#[cfg(not(test))]
pub(super) fn simulate_plan_cleanup_failure(_params: &Value) -> bool {
    false
}
