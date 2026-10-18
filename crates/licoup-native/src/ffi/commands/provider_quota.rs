use super::{AdmittedCommand, CliExecution, admitted_params};
use anyhow::Result;

pub(super) fn handle_provider_quota_snapshot(command: AdmittedCommand) -> Result<CliExecution> {
    let params = admitted_params(
        &[
            ("agent", command.option_text("agent")),
            ("stateRoot", command.option_text("state-root")),
        ],
        &[],
        &[("forceRefresh", command.option_flag("force-refresh"))],
    );
    Ok(CliExecution::Json(crate::domain::provider_quota::snapshot(
        &params,
    )?))
}
