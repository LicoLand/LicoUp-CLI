use super::{AdmittedCommand, CliExecution};
use anyhow::{Result, anyhow, ensure};
use serde_json::{Value, json};

use licoup_vault::api_key::{
    GatewayCredentialLeaseDays, LlmApiKeyCredentialUpdate, LlmApiKeyProvider, NewLlmApiKey,
};

fn local_token_helper() -> Result<std::path::PathBuf> {
    let executable =
        std::env::current_exe().map_err(|_| anyhow!("llm_gateway_agent_config_path_invalid"))?;
    if !executable.is_absolute() {
        return Err(anyhow!("llm_gateway_agent_config_path_invalid"));
    }
    Ok(executable)
}

pub(super) fn handle_status(_command: AdmittedCommand) -> Result<CliExecution> {
    Ok(CliExecution::Json(json!({
        "ok": true,
        "schemaVersion": "licoup.llm-api-key-status.v1",
        "supported": crate::platform::llm_api_key_vault::PlatformLlmApiKeyVault::platform_supported(),
        "locked": true,
        "providers": ["kimi", "deepseek", "kilo"],
        "leaseDayOptions": [7, 30, 60, 90, 180, 365]
    })))
}

pub(super) fn handle_list(_command: AdmittedCommand) -> Result<CliExecution> {
    let root = licoup_foundation::platform::paths::portable_data_dir()?;
    let inventory =
        crate::platform::llm_api_key_vault::PlatformLlmApiKeyVault::at_state_root(&root)?.list()?;
    let mut result = serde_json::to_value(inventory)?;
    result["supported"] =
        json!(crate::platform::llm_api_key_vault::PlatformLlmApiKeyVault::platform_supported());
    result["migrationPending"] =
        json!(crate::domain::client_state_migration::gateway_credential_migration_pending(&root)?);
    Ok(CliExecution::Json(result))
}

pub(super) fn handle_migrate(_command: AdmittedCommand) -> Result<CliExecution> {
    let root = licoup_foundation::platform::paths::portable_data_dir()?;
    let inventory = crate::domain::client_state_migration::migrate_gateway_credentials(&root)?;
    let mut result = serde_json::to_value(inventory)?;
    result["ok"] = json!(true);
    result["migrationPending"] = json!(false);
    Ok(CliExecution::Json(result))
}

pub(super) fn handle_authorize(command: AdmittedCommand) -> Result<CliExecution> {
    let credential_id = command.option_text("credential-id");
    Ok(CliExecution::Json(
        crate::platform::llm_gateway_service::credentials_authorize(credential_id)?,
    ))
}

pub(super) fn handle_clear(command: AdmittedCommand) -> Result<CliExecution> {
    let credential_id = command.option_text("credential-id");
    Ok(CliExecution::Json(
        crate::platform::llm_gateway_service::credentials_clear(credential_id)?,
    ))
}

pub(super) fn handle_create(mut command: AdmittedCommand) -> Result<CliExecution> {
    let mut input = match command.take_option_json("stdin-json") {
        Some(Value::Object(input)) => input,
        _ => return Err(anyhow!("llm_api_key_private_input_invalid")),
    };
    let provider = provider(input.remove("provider").as_ref().and_then(Value::as_str))?;
    let label = input
        .remove("label")
        .and_then(|value| value.as_str().map(str::to_owned))
        .ok_or_else(|| anyhow!("llm_api_key_label_invalid"))?;
    let api_key = input
        .remove("apiKey")
        .and_then(|value| match value {
            Value::String(value) => Some(value),
            _ => None,
        })
        .ok_or_else(|| anyhow!("llm_api_key_secret_invalid"))?;
    let validity = lease_days(input.remove("leaseDays"))?.unwrap_or_default();
    let inventory = crate::platform::llm_api_key_vault::PlatformLlmApiKeyVault::production()?
        .create(NewLlmApiKey::new(provider, label, api_key, validity)?)?;
    Ok(CliExecution::Json(serde_json::to_value(inventory)?))
}

pub(super) fn handle_update(mut command: AdmittedCommand) -> Result<CliExecution> {
    let id = command.required_text("credential-id").to_owned();
    let mut input = match command.take_option_json("stdin-json") {
        Some(Value::Object(input)) => input,
        _ => return Err(anyhow!("llm_api_key_private_input_invalid")),
    };
    let label = input
        .remove("label")
        .and_then(|value| value.as_str().map(str::to_owned));
    let extension = lease_days(input.remove("extendDays"))?;
    let update = LlmApiKeyCredentialUpdate::new(label, extension)?;
    let inventory = crate::platform::llm_api_key_vault::PlatformLlmApiKeyVault::production()?
        .update(&id, update)?;
    Ok(CliExecution::Json(serde_json::to_value(inventory)?))
}

pub(super) fn handle_delete(command: AdmittedCommand) -> Result<CliExecution> {
    let id = command.required_text("credential-id");
    let inventory =
        crate::platform::llm_api_key_vault::PlatformLlmApiKeyVault::production()?.delete(id)?;
    Ok(CliExecution::Json(serde_json::to_value(inventory)?))
}

pub(super) fn handle_lease(command: AdmittedCommand) -> Result<CliExecution> {
    let days = command
        .required_text("days")
        .parse::<u16>()
        .map_err(|_| anyhow!("llm_api_key_lease_days_invalid"))?;
    let days = GatewayCredentialLeaseDays::try_from(days).map_err(|code| anyhow!(code))?;
    let inventory = crate::platform::llm_api_key_vault::PlatformLlmApiKeyVault::production()?
        .set_lease_days(days)?;
    Ok(CliExecution::Json(serde_json::to_value(inventory)?))
}

pub(super) fn handle_agent_plan(command: AdmittedCommand) -> Result<CliExecution> {
    let target =
        licoup_private_gateway::model::llm_gateway_agent_config::GatewayAgentTarget::parse(
            command.required_text("agent"),
        )?;
    let root = std::path::PathBuf::from(command.required_text("config-root"));
    let port = command
        .option_text("port")
        .unwrap_or("15722")
        .parse::<u16>()?;
    let helper = local_token_helper()?;
    let models = if target.embeds_model_catalog() {
        crate::platform::llm_gateway_service::service_model_catalog(port)?
    } else {
        Vec::new()
    };
    let plan = licoup_private_gateway::model::llm_gateway_agent_config::plan_agent_config(
        target, &root, port, &helper, &models,
    )?;
    Ok(CliExecution::Json(serde_json::to_value(plan)?))
}

pub(super) fn handle_agent_apply(mut command: AdmittedCommand) -> Result<CliExecution> {
    let target =
        licoup_private_gateway::model::llm_gateway_agent_config::GatewayAgentTarget::parse(
            command.required_text("agent"),
        )?;
    let root = std::path::PathBuf::from(command.required_text("config-root"));
    let port = command
        .option_text("port")
        .unwrap_or("15722")
        .parse::<u16>()?;
    let helper = local_token_helper()?;
    let models = confirmed_agent_models(command.take_option_json("stdin-json"), target)?;
    let plan = licoup_private_gateway::model::llm_gateway_agent_config::plan_agent_config(
        target, &root, port, &helper, &models,
    )?;
    let confirmation = command
        .option_text("confirmation")
        .ok_or_else(|| anyhow!("llm_gateway_agent_config_confirmation_required"))?;
    if !command.option_flag("confirmed") || confirmation != plan.confirmation_digest {
        return Err(anyhow!("llm_gateway_agent_config_confirmation_invalid"));
    }
    let parent = plan
        .destination
        .parent()
        .ok_or_else(|| anyhow!("llm_gateway_agent_config_path_invalid"))?;
    licoup_foundation::platform::file_security::ensure_private_dir(parent)?;
    let content = if plan.content.contains(
        licoup_private_gateway::model::llm_gateway_agent_config::LOCAL_CLIENT_TOKEN_PLACEHOLDER,
    ) {
        let token = licoup_private_gateway::control::client_auth::ensure_default_token()?;
        let token = token
            .expose_utf8()
            .map_err(|_| anyhow!("gateway_client_token_invalid"))?;
        plan.content.replace(
            licoup_private_gateway::model::llm_gateway_agent_config::LOCAL_CLIENT_TOKEN_PLACEHOLDER,
            token,
        )
    } else {
        plan.content.clone()
    };
    if content.contains(
        licoup_private_gateway::model::llm_gateway_agent_config::LOCAL_CLIENT_TOKEN_PLACEHOLDER,
    ) {
        return Err(anyhow!("gateway_client_token_invalid"));
    }
    licoup_foundation::platform::file_security::atomic_write_private_text(
        &plan.destination,
        &content,
    )?;
    Ok(CliExecution::Json(
        json!({"ok": true, "agentId": plan.agent_id,
        "configured": true, "destination": plan.destination, "containsUpstreamSecret": false}),
    ))
}

fn confirmed_agent_models(
    supplied: Option<Value>,
    target: licoup_private_gateway::model::llm_gateway_agent_config::GatewayAgentTarget,
) -> Result<Vec<licoup_private_gateway::model::llm_gateway_agent_config::GatewayAgentModel>> {
    if !target.embeds_model_catalog() {
        ensure!(
            supplied.is_none(),
            "llm_gateway_agent_config_model_catalog_invalid"
        );
        return Ok(Vec::new());
    }
    let Value::Object(mut input) =
        supplied.ok_or_else(|| anyhow!("llm_gateway_agent_config_model_catalog_required"))?
    else {
        return Err(anyhow!("llm_gateway_agent_config_model_catalog_invalid"));
    };
    let models = input
        .remove("models")
        .and_then(|value| value.as_array().cloned())
        .ok_or_else(|| anyhow!("llm_gateway_agent_config_model_catalog_invalid"))?;
    ensure!(
        input.is_empty() && models.len() <= 8192,
        "llm_gateway_agent_config_model_catalog_invalid"
    );
    models
        .into_iter()
        .map(|model| {
            let Value::Object(mut model) = model else {
                return Err(anyhow!("llm_gateway_agent_config_model_catalog_invalid"));
            };
            let id = model
                .remove("id")
                .and_then(|value| value.as_str().map(str::to_owned))
                .ok_or_else(|| anyhow!("llm_gateway_agent_config_model_catalog_invalid"))?;
            let name = model
                .remove("name")
                .and_then(|value| value.as_str().map(str::to_owned))
                .ok_or_else(|| anyhow!("llm_gateway_agent_config_model_catalog_invalid"))?;
            let valid_id = id.split_once(':').is_some_and(|(provider, upstream)| {
                licoup_private_gateway::model::llm_gateway::namespaced_model_id(provider, upstream)
                    .as_deref()
                    == Some(id.as_str())
            });
            ensure!(
                model.is_empty()
                    && valid_id
                    && !name.is_empty()
                    && name.len() <= 1024
                    && !name.chars().any(char::is_control),
                "llm_gateway_agent_config_model_catalog_invalid"
            );
            Ok(
                licoup_private_gateway::model::llm_gateway_agent_config::GatewayAgentModel {
                    id,
                    name,
                },
            )
        })
        .collect()
}

pub(super) fn handle_service_status(command: AdmittedCommand) -> Result<CliExecution> {
    // Alias onto the unified Gateway Runtime status (LLM + channels).
    let port = service_port(command.option_text("port").as_deref())?;
    Ok(CliExecution::Json(
        crate::platform::gateway_runtime::service_status(port)?,
    ))
}

pub(super) fn handle_service_usage(_command: AdmittedCommand) -> Result<CliExecution> {
    Ok(CliExecution::Json(
        crate::platform::llm_gateway_service::service_usage()?,
    ))
}

pub(super) fn handle_service_initialize(command: AdmittedCommand) -> Result<CliExecution> {
    let port = service_port(command.option_text("port").as_deref())?;
    Ok(CliExecution::Json(
        crate::platform::gateway_runtime::service_initialize(port)?,
    ))
}

pub(super) fn handle_service_start(command: AdmittedCommand) -> Result<CliExecution> {
    let port = service_port(command.option_text("port").as_deref())?;
    Ok(CliExecution::Json(
        crate::platform::gateway_runtime::service_start(port)?,
    ))
}

pub(super) fn handle_service_stop(command: AdmittedCommand) -> Result<CliExecution> {
    let port = service_port(command.option_text("port").as_deref())?;
    Ok(CliExecution::Json(
        crate::platform::gateway_runtime::service_stop(port)?,
    ))
}

pub(super) fn handle_service_autostart_status(_command: AdmittedCommand) -> Result<CliExecution> {
    Ok(CliExecution::Json(
        crate::platform::llm_gateway_autostart::autostart_status()?,
    ))
}

pub(super) fn handle_service_autostart_enable(command: AdmittedCommand) -> Result<CliExecution> {
    let port = service_port(command.option_text("port").as_deref())?;
    Ok(CliExecution::Json(
        crate::platform::llm_gateway_autostart::autostart_enable(port)?,
    ))
}

pub(super) fn handle_service_autostart_disable(_command: AdmittedCommand) -> Result<CliExecution> {
    Ok(CliExecution::Json(
        crate::platform::llm_gateway_autostart::autostart_disable()?,
    ))
}

fn service_port(port: Option<&str>) -> Result<u16> {
    match port {
        Some(value) => value
            .parse::<u16>()
            .ok()
            .filter(|port| *port != 0)
            .ok_or_else(|| anyhow!("llm_gateway_port_invalid")),
        None => Ok(crate::platform::llm_gateway_service::DEFAULT_PORT),
    }
}

fn provider(value: Option<&str>) -> Result<LlmApiKeyProvider> {
    match value {
        Some("kimi") => Ok(LlmApiKeyProvider::Kimi),
        Some("deepseek") => Ok(LlmApiKeyProvider::DeepSeek),
        Some("kilo") => Ok(LlmApiKeyProvider::Kilo),
        _ => Err(anyhow!("llm_api_key_provider_invalid")),
    }
}

fn lease_days(value: Option<Value>) -> Result<Option<GatewayCredentialLeaseDays>> {
    let Some(value) = value else {
        return Ok(None);
    };
    let days = value
        .as_u64()
        .and_then(|raw| u16::try_from(raw).ok())
        .ok_or_else(|| anyhow!("llm_api_key_lease_days_invalid"))?;
    GatewayCredentialLeaseDays::try_from(days)
        .map(Some)
        .map_err(|code| anyhow!(code))
}
