use super::{AdmittedCommand, CliExecution, admitted_params};
use anyhow::Result;

pub(super) fn handle_agent_usage_scan(command: AdmittedCommand) -> Result<CliExecution> {
    let params = admitted_params(
        &[
            ("agent", command.option_text("agent")),
            ("historyDays", command.option_text("history-days")),
            (
                "timezoneOffsetMinutes",
                command.option_text("timezone-offset-minutes"),
            ),
            ("stateRoot", command.option_text("state-root")),
        ],
        &[(
            "timezoneTransitionsJson",
            command.option_json("timezone-transitions-json"),
        )],
        &[("forceRefresh", command.option_flag("force-refresh"))],
    );
    Ok(CliExecution::Json(crate::domain::agent_usage::scan(
        &params,
    )?))
}

pub(super) fn handle_agent_usage_report(command: AdmittedCommand) -> Result<CliExecution> {
    let params = admitted_params(
        &[
            ("agent", command.option_text("agent")),
            ("limit", command.option_text("limit")),
            ("stateRoot", command.option_text("state-root")),
        ],
        &[],
        &[],
    );
    Ok(CliExecution::Json(crate::domain::agent_usage::report(
        &params,
    )?))
}
