use anyhow::{Result, anyhow, ensure};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::path::{Path, PathBuf};

use super::commit::{CommitKind, CommitUnit};
use super::model::{WorkflowKind, WorkflowPlanRecord};
use crate::domain::collaboration_plugin::authority::AuthorityRegistration;
use crate::domain::collaboration_plugin::lifecycle::InstalledWorkflowPlugin;
use crate::platform::client_state::ClientStateStore;

const COLLECTION: &str = "mcp-install-transactions";
const SCHEMA: &str = "licoup.mcp-install-transaction.v1";

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub(super) enum Phase {
    Prepared,
    FilesCommitted,
    AuthorityCommitted,
    RollingBack,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
enum UnitKind {
    Directory,
    File,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct PendingUnit {
    staging: String,
    destination: String,
    kind: UnitKind,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct PendingMcpInstall {
    schema_version: String,
    phase: Phase,
    plan: WorkflowPlanRecord,
    authority_registrations: Vec<AuthorityRegistration>,
    units: Vec<PendingUnit>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum Recovery {
    None,
    RolledBack,
    Committed,
}

impl PendingMcpInstall {
    fn validate(&self, store: &ClientStateStore) -> Result<()> {
        ensure!(
            self.schema_version == SCHEMA
                && self.plan.workflow_kind == WorkflowKind::McpInstall
                && matches!(
                    self.phase,
                    Phase::Prepared
                        | Phase::FilesCommitted
                        | Phase::AuthorityCommitted
                        | Phase::RollingBack
                ),
            "collaboration_mcp_transaction_invalid"
        );
        self.plan.validate()?;
        let expected = crate::domain::collaboration_plugin::registration::authority_bindings(
            store,
            &self.plan.agent_destinations,
            &self.plan.agent_registrations,
        )?;
        ensure!(
            self.authority_registrations == expected
                && self.units.len() == self.plan.agent_destinations.len().saturating_mul(2),
            "collaboration_mcp_transaction_binding_invalid"
        );
        for (index, destination) in self.plan.agent_destinations.iter().enumerate() {
            let registration = self
                .plan
                .agent_registrations
                .iter()
                .find(|registration| registration.agent_id == destination.agent_id)
                .ok_or_else(|| anyhow!("collaboration_mcp_registration_missing"))?;
            validate_unit(
                &self.units[index * 2],
                Path::new(&destination.install_destination),
                UnitKind::Directory,
            )?;
            validate_unit(
                &self.units[index * 2 + 1],
                Path::new(&registration.destination),
                UnitKind::File,
            )?;
        }
        Ok(())
    }
}

pub(super) fn begin(
    store: &ClientStateStore,
    plan: &WorkflowPlanRecord,
    authority_registrations: &[AuthorityRegistration],
    units: &[CommitUnit],
) -> Result<()> {
    ensure!(
        read(store)?.is_none(),
        "collaboration_mcp_transaction_pending"
    );
    let pending = PendingMcpInstall {
        schema_version: SCHEMA.to_owned(),
        phase: Phase::Prepared,
        plan: plan.clone(),
        authority_registrations: authority_registrations.to_vec(),
        units: units
            .iter()
            .map(|unit| {
                Ok(PendingUnit {
                    staging: path_text(&unit.staging)?,
                    destination: path_text(&unit.destination)?,
                    kind: match unit.kind {
                        CommitKind::Directory => UnitKind::Directory,
                        CommitKind::File => UnitKind::File,
                    },
                })
            })
            .collect::<Result<Vec<_>>>()?,
    };
    pending.validate(store)?;
    for (index, unit) in pending.units.iter().enumerate() {
        ensure!(
            path_entry_exists(Path::new(&unit.staging))?
                && !path_entry_exists(Path::new(&unit.destination))?,
            "collaboration_mcp_transaction_stage_state_invalid"
        );
        verify_unit(&pending, index, Path::new(&unit.staging))?;
    }
    write(store, Some(&pending))
}

pub(super) fn advance(store: &ClientStateStore, phase: Phase) -> Result<()> {
    let mut pending =
        read(store)?.ok_or_else(|| anyhow!("collaboration_mcp_transaction_missing"))?;
    ensure!(
        valid_transition(pending.phase, phase),
        "collaboration_mcp_transaction_phase_invalid"
    );
    pending.phase = phase;
    write(store, Some(&pending))
}

pub(super) fn clear(store: &ClientStateStore) -> Result<()> {
    write(store, None)
}

pub(super) fn pending_for_plan(store: &ClientStateStore, plan_id: &str) -> Result<bool> {
    Ok(read(store)?.is_some_and(|pending| pending.plan.plan_id == plan_id))
}

pub(super) fn recover(
    store: &ClientStateStore,
    plan: &WorkflowPlanRecord,
    installed: &InstalledWorkflowPlugin,
) -> Result<Recovery> {
    let Some(pending) = read(store)? else {
        return Ok(Recovery::None);
    };
    ensure!(
        pending.plan == *plan,
        "collaboration_mcp_transaction_plan_mismatch"
    );
    let (_, authority) = crate::domain::collaboration_plugin::lifecycle::verified_authority(
        store,
        "Recover the exact MCP installation authority transaction",
    )?;
    if authority
        .authority
        .ensure_registrations(&pending.authority_registrations)
        .is_ok()
    {
        ensure!(
            pending.units.iter().all(|unit| {
                path_entry_exists(Path::new(&unit.staging)).ok() == Some(false)
                    && path_entry_exists(Path::new(&unit.destination)).ok() == Some(true)
            }),
            "collaboration_mcp_transaction_committed_state_invalid"
        );
        crate::domain::collaboration_plugin::registration::verify_installed_registration_targets(
            installed,
            &pending.plan.agent_destinations,
            &pending.plan.agent_registrations,
        )?;
        clear(store)?;
        return Ok(Recovery::Committed);
    }
    ensure!(
        pending.authority_registrations.iter().all(|registration| {
            !authority
                .authority
                .contains_registration(&registration.registration_id)
        }) && pending.phase != Phase::AuthorityCommitted,
        "collaboration_authority_registration_binding_mismatch"
    );
    if pending.phase != Phase::RollingBack {
        advance(store, Phase::RollingBack)?;
    }
    rollback_files(&pending)?;
    clear(store)?;
    Ok(Recovery::RolledBack)
}

fn rollback_files(pending: &PendingMcpInstall) -> Result<()> {
    for (index, unit) in pending.units.iter().enumerate() {
        let staging = Path::new(&unit.staging);
        let destination = Path::new(&unit.destination);
        let staging_exists = path_entry_exists(staging)?;
        let destination_exists = path_entry_exists(destination)?;
        ensure!(
            !(staging_exists && destination_exists),
            "collaboration_mcp_transaction_rollback_state_invalid"
        );
        if destination_exists {
            verify_unit(pending, index, destination)?;
            match unit.kind {
                UnitKind::Directory => {
                    super::commit::commit_directory_no_replace(destination, staging)
                }
                UnitKind::File => super::commit::commit_file_no_replace(destination, staging),
            }
            .map_err(|_| anyhow!("collaboration_mcp_transaction_rollback_failed"))?;
        }
    }
    for (index, unit) in pending.units.iter().enumerate() {
        let staging = Path::new(&unit.staging);
        if !path_entry_exists(staging)? {
            continue;
        }
        verify_unit(pending, index, staging)?;
        let removed = match unit.kind {
            UnitKind::Directory => std::fs::remove_dir_all(staging),
            UnitKind::File => std::fs::remove_file(staging),
        };
        removed.map_err(|_| anyhow!("collaboration_mcp_transaction_cleanup_pending"))?;
    }
    ensure!(
        pending.units.iter().all(|unit| {
            path_entry_exists(Path::new(&unit.staging)).ok() == Some(false)
                && path_entry_exists(Path::new(&unit.destination)).ok() == Some(false)
        }),
        "collaboration_mcp_transaction_cleanup_pending"
    );
    Ok(())
}

fn verify_unit(pending: &PendingMcpInstall, index: usize, path: &Path) -> Result<()> {
    let destination = &pending.plan.agent_destinations[index / 2];
    let registration = pending
        .plan
        .agent_registrations
        .iter()
        .find(|registration| registration.agent_id == destination.agent_id)
        .ok_or_else(|| anyhow!("collaboration_mcp_registration_missing"))?;
    match pending.units[index].kind {
        UnitKind::Directory => {
            crate::domain::collaboration_plugin::registration::verify_payload_target(
                registration,
                path,
            )
        }
        UnitKind::File => {
            crate::domain::collaboration_plugin::registration::verify_registration_target(
                registration,
                path,
            )
        }
    }
}

fn validate_unit(unit: &PendingUnit, destination: &Path, kind: UnitKind) -> Result<()> {
    let staging = Path::new(&unit.staging);
    let stage_name = staging
        .file_name()
        .and_then(|value| value.to_str())
        .and_then(|value| value.strip_prefix(".licoup-stage-"))
        .ok_or_else(|| anyhow!("collaboration_mcp_transaction_stage_invalid"))?;
    ensure!(
        unit.kind == kind
            && Path::new(&unit.destination) == destination
            && destination.is_absolute()
            && staging.is_absolute()
            && staging.parent() == destination.parent()
            && uuid::Uuid::parse_str(stage_name).is_ok_and(|value| value.to_string() == stage_name),
        "collaboration_mcp_transaction_unit_invalid"
    );
    Ok(())
}

fn read(store: &ClientStateStore) -> Result<Option<PendingMcpInstall>> {
    let collection = store.read_collection(COLLECTION)?;
    let items = collection
        .get("items")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    ensure!(
        items.len() <= 1,
        "collaboration_mcp_transaction_state_invalid"
    );
    let Some(value) = items.into_iter().next() else {
        return Ok(None);
    };
    let pending: PendingMcpInstall = serde_json::from_value(value)
        .map_err(|_| anyhow!("collaboration_mcp_transaction_state_invalid"))?;
    pending.validate(store)?;
    Ok(Some(pending))
}

fn write(store: &ClientStateStore, pending: Option<&PendingMcpInstall>) -> Result<()> {
    if let Some(pending) = pending {
        pending.validate(store)?;
    }
    store
        .write_collection(
            COLLECTION,
            json!({"items": pending.into_iter().collect::<Vec<_>>()}),
        )
        .map(|_| ())
}

fn valid_transition(previous: Phase, next: Phase) -> bool {
    matches!(
        (previous, next),
        (Phase::Prepared, Phase::FilesCommitted)
            | (Phase::Prepared, Phase::RollingBack)
            | (Phase::FilesCommitted, Phase::AuthorityCommitted)
            | (Phase::FilesCommitted, Phase::RollingBack)
    )
}

fn path_text(path: &PathBuf) -> Result<String> {
    path.to_str()
        .map(str::to_owned)
        .ok_or_else(|| anyhow!("collaboration_mcp_transaction_path_invalid"))
}

fn path_entry_exists(path: &Path) -> Result<bool> {
    match std::fs::symlink_metadata(path) {
        Ok(_) => Ok(true),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(_) => Err(anyhow!(
            "collaboration_mcp_transaction_path_state_unavailable"
        )),
    }
}
