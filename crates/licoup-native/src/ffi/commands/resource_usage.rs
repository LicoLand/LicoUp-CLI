use super::{AdmittedCommand, CliExecution, admitted_params};
use anyhow::Result;

pub(super) fn handle_resource_usage_scan(command: AdmittedCommand) -> Result<CliExecution> {
    let params = admitted_params(
        &[("stateRoot", command.option_text("state-root"))],
        &[],
        &[],
    );
    Ok(CliExecution::Json(
        crate::domain::agent_resource_usage::scan(&params)?,
    ))
}
