use super::{AdmittedCommand, CliExecution, admitted_params};
use anyhow::Result;

fn empty_params() -> serde_json::Value {
    admitted_params(&[], &[], &[])
}

fn install_params(
    github_url: Option<&str>,
    plan_id: Option<&str>,
    expected_digest: Option<&str>,
    confirmed: Option<&str>,
) -> serde_json::Value {
    admitted_params(
        &[
            ("githubUrl", github_url),
            ("planId", plan_id),
            ("expectedDigestSha256", expected_digest),
            ("confirmed", confirmed),
        ],
        &[],
        &[],
    )
}

fn local_deployment_params(
    request_origin: Option<&str>,
    selected_feature_ids: Option<&str>,
    destination: Option<&str>,
    destination_confirmed: Option<&str>,
    port: Option<&str>,
    plan_id: Option<&str>,
    expected_plan: Option<&str>,
    expected_package: Option<&str>,
    confirmed: Option<&str>,
) -> serde_json::Value {
    admitted_params(
        &[
            ("requestOrigin", request_origin),
            ("selectedFeatureIds", selected_feature_ids),
            ("destination", destination),
            ("destinationConfirmed", destination_confirmed),
            ("port", port),
            ("planId", plan_id),
            ("expectedPlanDigestSha256", expected_plan),
            ("expectedPackageDigestSha256", expected_package),
            ("confirmed", confirmed),
        ],
        &[],
        &[],
    )
}

fn mcp_install_params(
    request_origin: Option<&str>,
    selected_plugin_ids: Option<&str>,
    plan_id: Option<&str>,
    expected_plan: Option<&str>,
    expected_package: Option<&str>,
    confirmed: Option<&str>,
    agent_destinations: Option<&serde_json::Value>,
) -> serde_json::Value {
    admitted_params(
        &[
            ("requestOrigin", request_origin),
            ("selectedPluginIds", selected_plugin_ids),
            ("planId", plan_id),
            ("expectedPlanDigestSha256", expected_plan),
            ("expectedPackageDigestSha256", expected_package),
            ("confirmed", confirmed),
        ],
        &[("agentDestinations", agent_destinations)],
        &[],
    )
}

pub(super) fn handle_status(_command: AdmittedCommand) -> Result<CliExecution> {
    Ok(CliExecution::Json(
        crate::domain::collaboration_plugin::status(&empty_params())?,
    ))
}

pub(super) fn handle_enable(_command: AdmittedCommand) -> Result<CliExecution> {
    Ok(CliExecution::Json(
        crate::domain::collaboration_plugin::enable(&empty_params())?,
    ))
}

pub(super) fn handle_install_plan(command: AdmittedCommand) -> Result<CliExecution> {
    let params = install_params(
        command.option_text("github-url"),
        command.option_text("plan-id"),
        command.option_text("expected-digest-sha256"),
        command.option_text("confirmed"),
    );
    Ok(CliExecution::Json(
        crate::domain::collaboration_plugin::install_plan(&params)?,
    ))
}

pub(super) fn handle_install_apply(command: AdmittedCommand) -> Result<CliExecution> {
    let params = install_params(
        command.option_text("github-url"),
        command.option_text("plan-id"),
        command.option_text("expected-digest-sha256"),
        command.option_text("confirmed"),
    );
    Ok(CliExecution::Json(
        crate::domain::collaboration_plugin::install_apply(&params)?,
    ))
}

pub(super) fn handle_install_cancel(command: AdmittedCommand) -> Result<CliExecution> {
    let params = install_params(
        command.option_text("github-url"),
        command.option_text("plan-id"),
        command.option_text("expected-digest-sha256"),
        command.option_text("confirmed"),
    );
    Ok(CliExecution::Json(
        crate::domain::collaboration_plugin::install_cancel(&params)?,
    ))
}

pub(super) fn handle_workflow_catalog(_command: AdmittedCommand) -> Result<CliExecution> {
    Ok(CliExecution::Json(
        crate::domain::collaboration_plugin::workflow_catalog(&empty_params())?,
    ))
}

pub(super) fn handle_local_deployment_plan(command: AdmittedCommand) -> Result<CliExecution> {
    let params = local_deployment_params(
        command.option_text("request-origin"),
        command.option_text("selected-feature-ids"),
        command.option_text("destination"),
        command.option_text("destination-confirmed"),
        command.option_text("port"),
        command.option_text("plan-id"),
        command.option_text("expected-plan-digest-sha256"),
        command.option_text("expected-package-digest-sha256"),
        command.option_text("confirmed"),
    );
    Ok(CliExecution::Json(
        crate::domain::collaboration_plugin::local_deployment_plan(&params)?,
    ))
}

pub(super) fn handle_local_deployment_apply(command: AdmittedCommand) -> Result<CliExecution> {
    let params = local_deployment_params(
        command.option_text("request-origin"),
        command.option_text("selected-feature-ids"),
        command.option_text("destination"),
        command.option_text("destination-confirmed"),
        command.option_text("port"),
        command.option_text("plan-id"),
        command.option_text("expected-plan-digest-sha256"),
        command.option_text("expected-package-digest-sha256"),
        command.option_text("confirmed"),
    );
    Ok(CliExecution::Json(
        crate::domain::collaboration_plugin::local_deployment_apply(&params)?,
    ))
}

pub(super) fn handle_mcp_install_plan(command: AdmittedCommand) -> Result<CliExecution> {
    let params = mcp_install_params(
        command.option_text("request-origin"),
        command.option_text("selected-plugin-ids"),
        command.option_text("plan-id"),
        command.option_text("expected-plan-digest-sha256"),
        command.option_text("expected-package-digest-sha256"),
        command.option_text("confirmed"),
        command.option_json("agent-destinations"),
    );
    Ok(CliExecution::Json(
        crate::domain::collaboration_plugin::mcp_install_plan(&params)?,
    ))
}

pub(super) fn handle_mcp_install_apply(command: AdmittedCommand) -> Result<CliExecution> {
    let params = mcp_install_params(
        command.option_text("request-origin"),
        command.option_text("selected-plugin-ids"),
        command.option_text("plan-id"),
        command.option_text("expected-plan-digest-sha256"),
        command.option_text("expected-package-digest-sha256"),
        command.option_text("confirmed"),
        command.option_json("agent-destinations"),
    );
    Ok(CliExecution::Json(
        crate::domain::collaboration_plugin::mcp_install_apply(&params)?,
    ))
}

pub(super) fn handle_workflow_cancel(command: AdmittedCommand) -> Result<CliExecution> {
    let params = admitted_params(
        &[
            ("requestOrigin", command.option_text("request-origin")),
            ("planId", command.option_text("plan-id")),
            (
                "expectedPlanDigestSha256",
                command.option_text("expected-plan-digest-sha256"),
            ),
            (
                "expectedPackageDigestSha256",
                command.option_text("expected-package-digest-sha256"),
            ),
            ("confirmed", command.option_text("confirmed")),
        ],
        &[],
        &[],
    );
    Ok(CliExecution::Json(
        crate::domain::collaboration_plugin::workflow_cancel(&params)?,
    ))
}

pub(super) fn handle_local_server_status(_command: AdmittedCommand) -> Result<CliExecution> {
    Ok(CliExecution::Json(
        crate::domain::collaboration_plugin::local_server_status(&empty_params())?,
    ))
}

pub(super) fn handle_local_server_start(command: AdmittedCommand) -> Result<CliExecution> {
    let params = admitted_params(
        &[
            ("requestOrigin", command.option_text("request-origin")),
            ("deploymentId", command.option_text("deployment-id")),
            ("confirmed", command.option_text("confirmed")),
        ],
        &[],
        &[],
    );
    Ok(CliExecution::Json(
        crate::domain::collaboration_plugin::local_server_start(&params)?,
    ))
}

pub(super) fn handle_local_server_stop(command: AdmittedCommand) -> Result<CliExecution> {
    let params = admitted_params(
        &[
            ("requestOrigin", command.option_text("request-origin")),
            ("deploymentId", command.option_text("deployment-id")),
            ("confirmed", command.option_text("confirmed")),
        ],
        &[],
        &[],
    );
    Ok(CliExecution::Json(
        crate::domain::collaboration_plugin::local_server_stop(&params)?,
    ))
}

pub(super) fn handle_local_server_uninstall(command: AdmittedCommand) -> Result<CliExecution> {
    let params = admitted_params(
        &[
            ("requestOrigin", command.option_text("request-origin")),
            ("deploymentId", command.option_text("deployment-id")),
            (
                "expectedAssemblyManifestDigestSha256",
                command.option_text("expected-assembly-manifest-digest-sha256"),
            ),
            ("confirmed", command.option_text("confirmed")),
        ],
        &[],
        &[],
    );
    Ok(CliExecution::Json(
        crate::domain::collaboration_plugin::local_server_uninstall(&params)?,
    ))
}

pub(super) fn handle_disable(_command: AdmittedCommand) -> Result<CliExecution> {
    Ok(CliExecution::Json(
        crate::domain::collaboration_plugin::disable(&empty_params())?,
    ))
}

pub(super) fn handle_cleanup(_command: AdmittedCommand) -> Result<CliExecution> {
    Ok(CliExecution::Json(
        crate::domain::collaboration_plugin::cleanup(&empty_params())?,
    ))
}

#[cfg(test)]
mod tests {
    #[test]
    fn collaboration_help_exposes_governed_routes() {
        let help = super::super::build_command_table().help_text().join("\n");
        for path in [
            "collaboration install cancel",
            "collaboration workflow local-deployment plan",
            "collaboration workflow local-deployment apply",
            "collaboration workflow mcp-install plan",
            "collaboration workflow mcp-install apply",
            "collaboration workflow cancel",
            "collaboration local-server status",
            "collaboration local-server start",
            "collaboration local-server stop",
            "collaboration local-server uninstall",
            "collaboration cleanup",
        ] {
            assert!(help.contains(path));
        }
    }
}
