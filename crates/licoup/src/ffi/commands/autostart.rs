use super::{AdmittedCommand, CliExecution};
use anyhow::{Result, anyhow};

/// The Agent inventory port this command layer composes.
fn targets() -> crate::port::AgentTargetPort {
    crate::target_port::agent_target_port()
}

pub(super) fn handle_status(_command: AdmittedCommand) -> Result<CliExecution> {
    Ok(CliExecution::Json(
        crate::platform::client_autostart::status(&targets())?,
    ))
}

pub(super) fn handle_prepare_mcp(_command: AdmittedCommand) -> Result<CliExecution> {
    Ok(CliExecution::Json(
        crate::platform::client_autostart::prepare_mcp()?,
    ))
}

pub(super) fn handle_set(command: AdmittedCommand) -> Result<CliExecution> {
    let component = command
        .option_text("component")
        .ok_or_else(|| anyhow!("autostart_component_required"))?;
    let enabled = match command.option_text("enabled") {
        Some("true") | Some("1") | Some("yes") => true,
        Some("false") | Some("0") | Some("no") => false,
        Some(_) => return Err(anyhow!("autostart_enabled_invalid")),
        None => return Err(anyhow!("autostart_enabled_required")),
    };
    let silent = match command.option_text("silent") {
        Some("true") | Some("1") | Some("yes") => true,
        Some("false") | Some("0") | Some("no") | None => false,
        Some(_) => return Err(anyhow!("autostart_silent_invalid")),
    };
    let port = match command.option_text("port") {
        Some(value) => value
            .parse::<u16>()
            .ok()
            .filter(|port| *port != 0)
            .ok_or_else(|| anyhow!("llm_gateway_port_invalid"))?,
        None => crate::platform::llm_gateway_service::DEFAULT_PORT,
    };
    let result = match component {
        "desktop" => crate::platform::client_autostart::set_desktop(&targets(), enabled, silent)?,
        "mcp" => crate::platform::client_autostart::set_mcp(&targets(), enabled)?,
        "gateway" => crate::platform::client_autostart::set_gateway(&targets(), enabled, port)?,
        _ => return Err(anyhow!("autostart_component_invalid")),
    };
    Ok(CliExecution::Json(result))
}
