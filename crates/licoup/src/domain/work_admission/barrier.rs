//! The persisted close-admission barrier record.
//!
//! The record reuses the `native.update-handoff` machine
//! (`resources/state-machines/update-handoff.json`, `pending` → `claimed`). It
//! is written in the terminal `claimed` state, because holding admission closed
//! *is* the claim; a `pending` record on disk is a preparation and does not
//! close anything. Release retires the record rather than inventing a state the
//! machine does not define, and the next switch starts from a fresh `pending`.

use anyhow::{Context, Result, anyhow, ensure};
use licoup_foundation::platform::file_security::{
    atomic_write_private_text_bounded, read_existing_private_text_bounded,
    remove_private_state_marker,
};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

use super::MaintenanceOperation;
use crate::state_machines::update_handoff;

/// Record schema written by this owner.
pub(super) const BARRIER_SCHEMA: &str = "v0.0.1:maintenance-admission-1";
const BARRIER_RECORD: &str = "client-state/maintenance-admission.json";
const MAX_BARRIER_BYTES: usize = 16 * 1024;

/// The close-admission barrier a maintenance switch holds.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AdmissionBarrier {
    /// The switch this barrier was taken for.
    pub operation: MaintenanceOperation,
    /// The `native.update-handoff` state the barrier is held in. Today that is
    /// always the machine's terminal `claimed`.
    pub state: String,
    /// When the barrier was taken, in Unix milliseconds.
    pub claimed_at_unix_ms: i64,
}

#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct BarrierRecord {
    schema_version: String,
    state: String,
    operation: String,
    claimed_at_unix_ms: i64,
}

/// The barrier record path inside one data root.
pub(super) fn record_path(data_root: &Path) -> PathBuf {
    data_root.join(BARRIER_RECORD)
}

/// The barrier held for this data root, if any.
///
/// A missing record or a record in the machine's initial state means admission
/// is open. A record that cannot be read as this schema is an error: the caller
/// must not treat an unreadable barrier as permission to change installed
/// state.
pub(super) fn read(data_root: &Path) -> Result<Option<AdmissionBarrier>> {
    let Some(raw) = read_existing_private_text_bounded(&record_path(data_root), MAX_BARRIER_BYTES)
        .context("maintenance_admission_record_invalid")?
    else {
        return Ok(None);
    };
    let record: BarrierRecord =
        serde_json::from_str(&raw).context("maintenance_admission_record_invalid")?;
    ensure!(
        record.schema_version == BARRIER_SCHEMA && record.claimed_at_unix_ms >= 0,
        "maintenance_admission_record_invalid"
    );
    let state = update_handoff::State::from_name(&record.state)
        .ok_or_else(|| anyhow!("maintenance_admission_record_invalid"))?;
    let operation = MaintenanceOperation::from_name(&record.operation)
        .ok_or_else(|| anyhow!("maintenance_admission_record_invalid"))?;
    if state != update_handoff::State::Claimed {
        return Ok(None);
    }
    Ok(Some(AdmissionBarrier {
        operation,
        state: state.as_str().to_owned(),
        claimed_at_unix_ms: record.claimed_at_unix_ms,
    }))
}

/// Write the barrier durably: one atomic, fsynced private write.
pub(super) fn write(data_root: &Path, barrier: &AdmissionBarrier) -> Result<()> {
    ensure!(
        update_handoff::State::from_name(&barrier.state) == Some(update_handoff::State::Claimed),
        "maintenance_admission_transition_invalid"
    );
    let record = BarrierRecord {
        schema_version: BARRIER_SCHEMA.to_owned(),
        state: barrier.state.clone(),
        operation: barrier.operation.as_str().to_owned(),
        claimed_at_unix_ms: barrier.claimed_at_unix_ms,
    };
    let mut serialized =
        serde_json::to_string(&record).context("maintenance_admission_record_invalid")?;
    serialized.push('\n');
    ensure!(
        serialized.len() <= MAX_BARRIER_BYTES,
        "maintenance_admission_record_invalid"
    );
    atomic_write_private_text_bounded(&record_path(data_root), &serialized, MAX_BARRIER_BYTES)
        .context("maintenance_admission_write_failed")
}

/// Retire the barrier record, reopening admission for the next switch.
pub(super) fn retire(data_root: &Path) -> Result<()> {
    let removed = remove_private_state_marker(&record_path(data_root))
        .context("maintenance_admission_release_failed")?;
    if !removed {
        return Ok(());
    }
    Ok(())
}
