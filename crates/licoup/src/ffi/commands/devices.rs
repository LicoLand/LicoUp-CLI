//! The `devices` command group (LicoArc pairing and paired-device messages).

use super::{AdmittedCommand, CliExecution};
use anyhow::{Result, anyhow};
use serde_json::Value;

fn stdin_json(command: &AdmittedCommand) -> Result<Value> {
    command
        .option_json("stdin-json")
        .cloned()
        .ok_or_else(|| anyhow!("devices_input_invalid"))
}

pub(super) fn handle_status(_command: AdmittedCommand) -> Result<CliExecution> {
    Ok(CliExecution::Json(crate::domain::devices::status()?))
}

pub(super) fn handle_enroll(command: AdmittedCommand) -> Result<CliExecution> {
    Ok(CliExecution::Json(crate::domain::devices::enroll(
        &stdin_json(&command)?,
    )?))
}

pub(super) fn handle_pair_offer(command: AdmittedCommand) -> Result<CliExecution> {
    Ok(CliExecution::Json(crate::domain::devices::pair_offer(
        command.option_text("label"),
    )?))
}

pub(super) fn handle_pair_claim(command: AdmittedCommand) -> Result<CliExecution> {
    Ok(CliExecution::Json(crate::domain::devices::pair_claim(
        &stdin_json(&command)?,
    )?))
}

pub(super) fn handle_pair_cancel(command: AdmittedCommand) -> Result<CliExecution> {
    let pairing_id = command
        .option_text("pairing-id")
        .ok_or_else(|| anyhow!("devices_pairing_id_required"))?;
    Ok(CliExecution::Json(crate::domain::devices::pair_cancel(
        pairing_id,
    )?))
}

pub(super) fn handle_unpair(command: AdmittedCommand) -> Result<CliExecution> {
    let peer_id = command
        .option_text("peer-id")
        .ok_or_else(|| anyhow!("devices_peer_id_required"))?;
    Ok(CliExecution::Json(crate::domain::devices::unpair(peer_id)?))
}

pub(super) fn handle_sync(_command: AdmittedCommand) -> Result<CliExecution> {
    Ok(CliExecution::Json(crate::domain::devices::sync()?))
}

pub(super) fn handle_message_send(command: AdmittedCommand) -> Result<CliExecution> {
    let peer_id = command
        .option_text("peer-id")
        .ok_or_else(|| anyhow!("devices_peer_id_required"))?;
    let agent = command
        .option_text("agent")
        .ok_or_else(|| anyhow!("agent_binding_required"))?;
    Ok(CliExecution::Json(crate::domain::devices::message_send(
        peer_id,
        agent,
        command.option_text("session-id"),
        &stdin_json(&command)?,
    )?))
}

pub(super) fn handle_message_results(command: AdmittedCommand) -> Result<CliExecution> {
    Ok(CliExecution::Json(crate::domain::devices::message_results(
        command.option_text("peer-id"),
        command.option_text("since"),
    )?))
}
