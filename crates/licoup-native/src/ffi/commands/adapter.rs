//! Adapter bridge lifecycle commands.
//!
//! The catalog is projected from the canonical packaged runtime registry.
//! Keep agent-specific transport and filesystem details inside each adapter.

use super::{AdmittedCommand, CliExecution};
use anyhow::Result;
use std::path::Path;

pub(super) fn handle_catalog(_command: AdmittedCommand) -> Result<CliExecution> {
    crate::platform::runtime_adapters::install();
    let bridge = licoup_agent_antigravity::driver::hook_bridge_status();
    let installed = bridge
        .get("installed")
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false);
    Ok(CliExecution::Json(
        crate::platform::runtime_adapters::adapter_management_catalog(installed),
    ))
}

pub(super) fn handle_antigravity_status(_command: AdmittedCommand) -> Result<CliExecution> {
    Ok(CliExecution::Json(
        licoup_agent_antigravity::driver::hook_bridge_status(),
    ))
}

pub(super) fn handle_antigravity_install(_command: AdmittedCommand) -> Result<CliExecution> {
    Ok(CliExecution::Json(adapter_lifecycle_result(
        "antigravity",
        "install",
        licoup_agent_antigravity::driver::install_hook_bridge(),
    )))
}

pub(super) fn handle_antigravity_uninstall(_command: AdmittedCommand) -> Result<CliExecution> {
    Ok(CliExecution::Json(adapter_lifecycle_result(
        "antigravity",
        "uninstall",
        licoup_agent_antigravity::driver::uninstall_hook_bridge_report(),
    )))
}

pub(super) fn handle_antigravity_authorize(command: AdmittedCommand) -> Result<CliExecution> {
    let binary_path = command.option_text("binary-path");
    Ok(CliExecution::Json(adapter_lifecycle_result(
        "antigravity",
        "authorize",
        licoup_agent_antigravity::driver::authorize(binary_path.as_deref()),
    )))
}

pub(super) fn handle_subagent_mcp_status(command: AdmittedCommand) -> Result<CliExecution> {
    let agent_id = command.option_text("agent-id").unwrap_or_default();
    let binary = command.option_text("binary-path").map(Path::new);
    let mcp_binary = command.option_text("mcp-binary-path").map(Path::new);
    let config_path = command.option_text("config-path").map(Path::new);
    let state =
        crate::platform::subagent_mcp_ensure::status(&agent_id, binary, mcp_binary, config_path);
    Ok(CliExecution::Json(serde_json::json!({
        "ok": true,
        "agentId": agent_id,
        "state": state.as_str(),
        "ready": state.ready(),
        "configPathExplicit": config_path.is_some(),
        "orchestrationOwner": if state.ready() { "main-agent-plugin" } else { "licoup" },
    })))
}

pub(super) fn handle_subagent_mcp_plan(command: AdmittedCommand) -> Result<CliExecution> {
    let agent_id = command.option_text("agent-id").unwrap_or_default();
    let binary = command.option_text("binary-path").map(Path::new);
    let mcp_binary = command.option_text("mcp-binary-path").map(Path::new);
    let config_path = command.option_text("config-path").map(Path::new);
    let explicit = config_path.is_some();
    Ok(CliExecution::Json(
        match crate::platform::subagent_mcp_ensure::plan(&agent_id, binary, mcp_binary, config_path)
        {
            Ok(plan) => serde_json::json!({
                "ok": true,
                "agentId": plan.agent_id,
                "digest": plan.digest,
                "pluginVersion": plan.plugin_version,
                "marketplaceSource": plan.source,
                "marketplaceRelease": plan.release,
                "requiresConfirmation": plan.requires_confirmation,
                "configPathExplicit": explicit,
                "fallbackOwner": "licoup",
            }),
            Err(error) => subagent_mcp_error(error),
        },
    ))
}

pub(super) fn handle_subagent_mcp_install(command: AdmittedCommand) -> Result<CliExecution> {
    let agent_id = command.option_text("agent-id").unwrap_or_default();
    let binary = command.option_text("binary-path").map(Path::new);
    let mcp_binary = command.option_text("mcp-binary-path").map(Path::new);
    let config_path = command.option_text("config-path").map(Path::new);
    let confirmation = command.option_text("confirmation").unwrap_or_default();
    let confirmed = command.option_flag("confirmed");
    Ok(CliExecution::Json(
        match crate::platform::subagent_mcp_ensure::install(
            &agent_id,
            binary,
            mcp_binary,
            &confirmation,
            confirmed,
            config_path,
        ) {
            Ok((installed, ready)) => serde_json::json!({
                "ok": true,
                "agentId": agent_id,
                "installed": installed,
                "pluginReadyForNewConversations": ready,
                "orchestrationOwner": "main-agent-plugin",
            }),
            Err(error) => subagent_mcp_error(error),
        },
    ))
}

fn subagent_mcp_error(
    error: crate::platform::subagent_mcp_ensure::SubagentMcpEnsureError,
) -> serde_json::Value {
    serde_json::json!({
        "ok": false,
        "error": {
            "code": error.code(),
            "stage": "adapter/subagent-mcp",
            "component": "managed-plugin",
            "retryable": matches!(
                error,
                crate::platform::subagent_mcp_ensure::SubagentMcpEnsureError::ProcessUnavailable
                    | crate::platform::subagent_mcp_ensure::SubagentMcpEnsureError::InstallFailed
            ),
        },
        "fallbackOwner": "licoup",
    })
}

fn adapter_lifecycle_result(
    adapter_id: &str,
    action: &str,
    result: std::result::Result<serde_json::Value, &'static str>,
) -> serde_json::Value {
    match result {
        Ok(value) => value,
        Err(_) => serde_json::json!({
            "ok": false,
            "adapterId": adapter_id,
            "error": {
                "code": match action {
                    "install" => "adapter_plugin_install_failed",
                    "uninstall" => "adapter_plugin_uninstall_failed",
                    _ => "adapter_plugin_action_failed",
                },
                "stage": "adapter/lifecycle",
                "component": "managed-bridge",
                "retryable": true,
                "recovery": "retry_or_review_local_adapter_state",
            },
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn catalog_projects_every_packaged_adapter_without_fake_install_actions() {
        let CliExecution::Json(catalog) =
            crate::ffi::commands::execute_cli(vec!["adapter".into(), "catalog".into()]).unwrap()
        else {
            assert!(false, "adapter catalog must be JSON");
            return;
        };
        assert_eq!(catalog["ok"], true);
        assert_eq!(catalog["schemaVersion"], "lico.adapter-plugin-catalog.v1");
        let adapters = catalog["adapters"].as_array().unwrap();
        assert_eq!(
            adapters.len(),
            crate::platform::runtime_adapters::PACKAGED_RUNTIME_ADAPTER_IDS.len()
        );
        for adapter in adapters {
            let managed = adapter["managementKind"] == "managed-bridge";
            assert_eq!(
                adapter["lifecycleActions"]
                    .as_array()
                    .is_some_and(|actions| !actions.is_empty()),
                managed
            );
            if !managed {
                assert_eq!(adapter["installationState"], "not-required");
            }
        }
    }

    #[test]
    fn managed_bridge_failures_are_typed_and_do_not_expose_platform_details() {
        for (action, code) in [
            ("install", "adapter_plugin_install_failed"),
            ("uninstall", "adapter_plugin_uninstall_failed"),
        ] {
            let result = adapter_lifecycle_result(
                "antigravity",
                action,
                Err("private_platform_failure_detail"),
            );
            assert_eq!(result["ok"], false);
            assert_eq!(result["adapterId"], "antigravity");
            assert_eq!(result["error"]["code"], code);
            assert_eq!(result["error"]["stage"], "adapter/lifecycle");
            assert_eq!(result["error"]["component"], "managed-bridge");
            assert_eq!(result["error"]["retryable"], true);
            assert!(
                !result
                    .to_string()
                    .contains("private_platform_failure_detail")
            );
        }
    }

    #[test]
    fn the_retired_codex_plugin_route_is_refused_rather_than_answered() {
        // The LicoUp Codex Plugin route is retired: that plugin belongs to
        // Codex's own marketplace and is owned by the adapter package, so this
        // client plans, approves, installs and probes none of it. The former CLI
        // path is not admitted, and the absent local binary it named reaches no
        // report rather than a second owner's answer.
        let missing_binary =
            std::env::temp_dir().join(format!("lico-missing-codex-{}", uuid::Uuid::new_v4()));
        let error = crate::ffi::commands::admit_cli_command(vec![
            "adapter".into(),
            "codex".into(),
            "plugin".into(),
            "status".into(),
            "--binary-path".into(),
            missing_binary.to_string_lossy().into_owned(),
        ])
        .expect_err("the retired route is refused");
        let refusal = error
            .downcast_ref::<crate::ffi::commands::CliCommandError>()
            .expect("an admission refusal is a typed CLI error");
        assert_eq!(refusal.code(), "cli_operation_unsupported");
        assert_eq!(refusal.recovery(), "use_cli_help");
        assert!(!format!("{error:?}").contains("missing-codex"));
    }
}
