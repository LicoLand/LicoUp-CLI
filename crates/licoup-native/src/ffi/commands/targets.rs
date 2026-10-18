use super::{AdmittedCommand, CliExecution, admitted_params};
use anyhow::{Result, ensure};
use serde_json::Value;

/// The Agent inventory port this command layer composes.
fn targets() -> crate::port::AgentTargetPort {
    crate::target_port::agent_target_port()
}

pub(super) fn handle_targets_catalog(_command: AdmittedCommand) -> Result<CliExecution> {
    Ok(CliExecution::Json(serde_json::json!({
        "ok": true,
        "targetIds": crate::domain::agent_catalog::ids(),
    })))
}

pub(super) fn handle_targets_scan(command: AdmittedCommand) -> Result<CliExecution> {
    let mut params = admitted_params(
        &[
            ("stateRoot", command.option_text("state-root")),
            (
                "includeAccessibleEnvironments",
                command.option_text("include-accessible-environments"),
            ),
            (
                "includeHistoryModelCatalog",
                command.option_text("include-history-model-catalog"),
            ),
            (
                "enableAgentCliModelLookup",
                command.option_text("enable-agent-cli-model-lookup"),
            ),
        ],
        &[],
        &[],
    );
    if let Some(private) = command.option_json("stdin-json") {
        let private = private
            .as_object()
            .ok_or_else(|| anyhow::anyhow!("target_selection_invalid"))?;
        ensure!(
            private
                .keys()
                .all(|key| matches!(key.as_str(), "targetIds" | "modelCatalogTargetIds")),
            "target_selection_invalid"
        );
        let object = params
            .as_object_mut()
            .ok_or_else(|| anyhow::anyhow!("target_selection_invalid"))?;
        for key in ["targetIds", "modelCatalogTargetIds"] {
            if let Some(value) = private.get(key) {
                object.insert(key.to_string(), value.clone());
            }
        }
    }
    Ok(CliExecution::Json(
        crate::domain::targets::scan_targets_with_params(&targets(), &params)?,
    ))
}

pub(super) fn handle_targets_add(command: AdmittedCommand) -> Result<CliExecution> {
    let mut params = admitted_params(
        &[
            ("target", command.option_text("target")),
            ("configPath", command.option_text("config-path")),
            ("binaryPath", command.option_text("binary-path")),
            ("historyRoot", command.option_text("history-root")),
            ("stateRoot", command.option_text("state-root")),
        ],
        &[],
        &[],
    );
    if let Some(private) = command.option_json("stdin-json") {
        let private = private
            .as_object()
            .ok_or_else(|| anyhow::anyhow!("target_private_input_invalid"))?;
        let public_target = params
            .get("target")
            .and_then(Value::as_str)
            .unwrap_or_default();
        if let Some(private_target) = private.get("target").and_then(Value::as_str) {
            ensure!(
                private_target == public_target,
                "target_private_input_mismatch"
            );
        }
        let object = params
            .as_object_mut()
            .ok_or_else(|| anyhow::anyhow!("target_private_input_invalid"))?;
        for key in ["label", "kind", "location", "runtimeConnection"] {
            if let Some(value) = private.get(key) {
                object.insert(key.to_string(), value.clone());
            }
        }
    }
    Ok(CliExecution::Json(crate::domain::targets::add_target(
        &targets(),
        &params,
    )?))
}

pub(super) fn handle_targets_inspect(command: AdmittedCommand) -> Result<CliExecution> {
    let target = command.required_text("target");
    let mut params = admitted_params(
        &[
            ("stateRoot", command.option_text("state-root")),
            (
                "includeAccessibleEnvironments",
                command.option_text("include-accessible-environments"),
            ),
            (
                "enableAgentCliModelLookup",
                command.option_text("enable-agent-cli-model-lookup"),
            ),
        ],
        &[],
        &[],
    );
    if let Some(object) = params.as_object_mut() {
        object.insert("target".to_string(), Value::String(target.to_string()));
    }
    Ok(CliExecution::Json(
        crate::domain::targets::inspect_target_with_params(&targets(), &params)?,
    ))
}

/// The selection matrix document for one Agent.
///
/// It composes the same catalogue facts `targets inspect` answers through the
/// same ports, and adds the two statements the catalogue cannot make on its
/// own: whether this host declares the Agent at all, and the effective
/// per-scope execution outcome the composed policy owner states. The four
/// dimensions stay separate in the document, so a client renders support,
/// availability, credentials and execution as four facts with four reasons
/// instead of one readiness label.
pub(super) fn handle_targets_model_selection(command: AdmittedCommand) -> Result<CliExecution> {
    let target = command.required_text("target");
    let mut params = admitted_params(
        &[
            ("stateRoot", command.option_text("state-root")),
            (
                "includeAccessibleEnvironments",
                command.option_text("include-accessible-environments"),
            ),
            (
                "enableAgentCliModelLookup",
                command.option_text("enable-agent-cli-model-lookup"),
            ),
        ],
        &[],
        &[],
    );
    if let Some(object) = params.as_object_mut() {
        object.insert("target".to_string(), Value::String(target.to_string()));
    }
    Ok(CliExecution::Json(
        crate::domain::targets::model_selection_matrix_document(target, &params)?,
    ))
}

#[cfg(test)]
mod tests {
    use super::super::{CliExecution, execute_cli};

    #[test]
    fn catalog_includes_discovery_only_targets_and_registered_lanes() {
        let CliExecution::Json(value) =
            execute_cli(vec!["targets".into(), "catalog".into()]).unwrap()
        else {
            panic!("target catalog must return JSON");
        };
        assert_eq!(value["ok"], true);
        let ids = value["targetIds"].as_array().unwrap();
        for id in ["code", "workbuddy", "kimi-code", "command-code"] {
            assert!(ids.iter().any(|value| value.as_str() == Some(id)));
        }
        assert_eq!(value.as_object().unwrap().len(), 2);
    }
}
