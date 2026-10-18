// opencode-serve commands: opencode-serve ensure|start|stop|restart|status

use super::{AdmittedCommand, CliExecution, admitted_params};
use anyhow::Result;

pub(super) fn handle_opencode_serve(command: AdmittedCommand) -> Result<CliExecution> {
    let action = match command.path() {
        ["opencode-serve", action] => *action,
        _ => {
            return Err(super::handler_error("command_failed", "use_cli_help").into());
        }
    };
    let params = admitted_params(
        &[
            ("port", command.option_text("port")),
            ("executable", command.option_text("executable")),
            ("attachUrl", command.option_text("attach-url")),
        ],
        &[],
        &[],
    );
    let result = match action {
        "ensure" => crate::platform::opencode_serve::ensure(&params)?,
        "start" => crate::platform::opencode_serve::start(&params)?,
        "stop" => crate::platform::opencode_serve::stop(&params)?,
        "restart" => crate::platform::opencode_serve::restart(&params)?,
        "status" => crate::platform::opencode_serve::status(&params)?,
        _ => {
            return Err(super::handler_error("command_failed", "use_cli_help").into());
        }
    };
    Ok(CliExecution::Json(result))
}
