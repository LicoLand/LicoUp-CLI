use super::{AdmittedCommand, CliExecution, admitted_params};
use anyhow::Result;
use serde_json::Value;

fn with_target(mut params: Value, target: &str) -> Value {
    if let Some(object) = params.as_object_mut() {
        object.insert("target".to_string(), Value::String(target.to_string()));
        object.insert(
            "positionals".to_string(),
            Value::Array(vec![Value::String(target.to_string())]),
        );
    }
    params
}

pub(super) fn handle_skill_list(command: AdmittedCommand) -> Result<CliExecution> {
    let params = admitted_params(
        &[
            ("agent", command.option_text("agent")),
            ("skillRoot", command.option_text("skill-root")),
        ],
        &[],
        &[],
    );
    Ok(CliExecution::Json(crate::domain::skill_hub::skill_list(
        &params,
    )?))
}

pub(super) fn handle_skill_get(command: AdmittedCommand) -> Result<CliExecution> {
    let skill_id = command.required_text("skill-id");
    let params = with_target(
        admitted_params(
            &[
                ("agent", command.option_text("agent")),
                ("skillRoot", command.option_text("skill-root")),
            ],
            &[],
            &[],
        ),
        skill_id,
    );
    Ok(CliExecution::Json(crate::domain::skill_hub::skill_get(
        &params,
    )?))
}

pub(super) fn handle_skill_delete_plan(command: AdmittedCommand) -> Result<CliExecution> {
    let params = admitted_params(
        &[
            ("skill", command.option_text("skill")),
            ("path", command.option_text("path")),
        ],
        &[],
        &[],
    );
    Ok(CliExecution::Json(
        crate::domain::skill_hub::skill_delete_plan(&params)?,
    ))
}

pub(super) fn handle_skill_delete_apply(command: AdmittedCommand) -> Result<CliExecution> {
    let params = admitted_params(
        &[
            ("skill", command.option_text("skill")),
            ("path", command.option_text("path")),
            ("confirmation", command.option_text("confirmation")),
        ],
        &[],
        &[],
    );
    Ok(CliExecution::Json(
        crate::domain::skill_hub::skill_delete_apply(&params)?,
    ))
}

pub(super) fn handle_skill_visibility(command: AdmittedCommand) -> Result<CliExecution> {
    let skill_id = command.required_text("skill-id");
    let params = with_target(
        admitted_params(
            &[
                ("agent", command.option_text("agent")),
                ("hidden", command.option_text("hidden")),
            ],
            &[],
            &[],
        ),
        skill_id,
    );
    Ok(CliExecution::Json(
        crate::domain::skill_hub::skill_visibility(&params)?,
    ))
}

pub(super) fn handle_skill_usage_report(command: AdmittedCommand) -> Result<CliExecution> {
    let params = admitted_params(
        &[
            ("agent", command.option_text("agent")),
            ("skill", command.option_text("skill")),
            ("days", command.option_text("days")),
            ("from", command.option_text("from")),
            ("to", command.option_text("to")),
        ],
        &[],
        &[],
    );
    Ok(CliExecution::Json(
        crate::domain::skill_hub::skill_usage_report(&params)?,
    ))
}

pub(super) fn handle_skill_usage_scan(command: AdmittedCommand) -> Result<CliExecution> {
    let params = admitted_params(
        &[
            ("agent", command.option_text("agent")),
            ("historyRoot", command.option_text("history-root")),
            ("homeDir", command.option_text("home-dir")),
        ],
        &[],
        &[("forceRefresh", command.option_flag("force-refresh"))],
    );
    Ok(CliExecution::Json(
        crate::domain::skill_hub::skill_usage_scan(&params)?,
    ))
}
