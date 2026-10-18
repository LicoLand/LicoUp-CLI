use super::super::manifest::validate_relative_path;
use super::inspection::package_file;
use super::{InspectedPackage, PackageFile, WorkflowChoice};
use anyhow::{Result, anyhow, ensure};
use serde::Deserialize;
use serde_json::Value;
use std::collections::BTreeSet;

pub(super) const LOCAL_DEPLOYMENT_SCHEMA: &str = "licoup.collaboration.local-deployment.v1";
pub(super) const MCP_INSTALL_SCHEMA: &str = "licoup.collaboration.mcp-install.v2";
const MCP_OUTBOUND_POLICY: &str = "direct-user-exact-scope-one-shot";

#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct LocalDeploymentDescriptor {
    schema_version: String,
    manual_only: bool,
    features: Vec<DeploymentChoice>,
}

#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct McpInstallDescriptor {
    schema_version: String,
    manual_only: bool,
    requires_per_file_approval: bool,
    outbound_policy: String,
    plugins: Vec<McpChoice>,
}

#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct DeploymentChoice {
    id: String,
    label: String,
    #[serde(default)]
    description: String,
    package_path: String,
    #[serde(default)]
    default_enabled: bool,
}

#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct McpChoice {
    id: String,
    label: String,
    #[serde(default)]
    description: String,
    package_path: String,
    endpoint: String,
}

pub(in crate::domain::collaboration_plugin) fn local_deployment_choices(
    package: &InspectedPackage,
) -> Result<Vec<WorkflowChoice>> {
    let descriptor: LocalDeploymentDescriptor = serde_json::from_slice(package_file(
        &package.files,
        &package.manifest.local_deployment_descriptor,
    )?)
    .map_err(|_| anyhow!("collaboration_plugin_deployment_catalog_invalid"))?;
    ensure!(
        descriptor.schema_version == LOCAL_DEPLOYMENT_SCHEMA
            && descriptor.manual_only
            && descriptor
                .features
                .iter()
                .all(|choice| !choice.default_enabled),
        "collaboration_plugin_deployment_policy_invalid"
    );
    descriptor
        .features
        .into_iter()
        .map(|choice| {
            Ok(WorkflowChoice {
                id: choice.id,
                package_path: validate_relative_path(
                    &choice.package_path,
                    "collaboration_plugin_workflow_package_path_invalid",
                )?,
                endpoint: None,
            })
        })
        .collect()
}

pub(in crate::domain::collaboration_plugin) fn mcp_install_choices(
    package: &InspectedPackage,
) -> Result<Vec<WorkflowChoice>> {
    let descriptor: McpInstallDescriptor = serde_json::from_slice(package_file(
        &package.files,
        &package.manifest.mcp_install_descriptor,
    )?)
    .map_err(|_| anyhow!("collaboration_plugin_mcp_catalog_invalid"))?;
    ensure!(
        descriptor.schema_version == MCP_INSTALL_SCHEMA
            && descriptor.manual_only
            && descriptor.requires_per_file_approval
            && descriptor.outbound_policy == MCP_OUTBOUND_POLICY,
        "collaboration_plugin_mcp_file_approval_policy_invalid"
    );
    descriptor
        .plugins
        .into_iter()
        .map(|choice| {
            Ok(WorkflowChoice {
                id: choice.id,
                package_path: validate_relative_path(
                    &choice.package_path,
                    "collaboration_plugin_workflow_package_path_invalid",
                )?,
                endpoint: Some(validate_mcp_endpoint(&choice.endpoint)?),
            })
        })
        .collect()
}

pub(super) fn validate_descriptor(
    bytes: &[u8],
    expected_schema: &str,
    files: &[PackageFile],
) -> Result<()> {
    let value: Value = serde_json::from_slice(bytes)
        .map_err(|_| anyhow!("collaboration_plugin_workflow_descriptor_invalid"))?;
    let object = value
        .as_object()
        .ok_or_else(|| anyhow!("collaboration_plugin_workflow_descriptor_invalid"))?;
    reject_executable_directives(&value)?;
    ensure!(
        object.get("schemaVersion").and_then(Value::as_str) == Some(expected_schema),
        "collaboration_plugin_workflow_descriptor_schema_invalid"
    );
    ensure!(
        object.get("manualOnly").and_then(Value::as_bool) == Some(true),
        "collaboration_plugin_workflow_must_be_manual"
    );
    match expected_schema {
        LOCAL_DEPLOYMENT_SCHEMA => {
            let descriptor: LocalDeploymentDescriptor = serde_json::from_value(value)
                .map_err(|_| anyhow!("collaboration_plugin_deployment_catalog_invalid"))?;
            ensure!(
                descriptor.schema_version == LOCAL_DEPLOYMENT_SCHEMA && descriptor.manual_only,
                "collaboration_plugin_deployment_policy_invalid"
            );
            let choices = descriptor
                .features
                .iter()
                .map(|choice| {
                    (
                        choice.id.as_str(),
                        choice.label.as_str(),
                        choice.description.as_str(),
                        choice.package_path.as_str(),
                    )
                })
                .collect::<Vec<_>>();
            validate_choices(&choices, files)?;
            ensure!(
                descriptor
                    .features
                    .iter()
                    .all(|choice| !choice.default_enabled),
                "collaboration_plugin_default_feature_selection_forbidden"
            );
        }
        MCP_INSTALL_SCHEMA => {
            let descriptor: McpInstallDescriptor = serde_json::from_value(value)
                .map_err(|_| anyhow!("collaboration_plugin_mcp_catalog_invalid"))?;
            ensure!(
                descriptor.schema_version == MCP_INSTALL_SCHEMA
                    && descriptor.manual_only
                    && descriptor.requires_per_file_approval
                    && descriptor.outbound_policy == MCP_OUTBOUND_POLICY,
                "collaboration_plugin_mcp_file_approval_policy_invalid"
            );
            let choices = descriptor
                .plugins
                .iter()
                .map(|choice| {
                    (
                        choice.id.as_str(),
                        choice.label.as_str(),
                        choice.description.as_str(),
                        choice.package_path.as_str(),
                    )
                })
                .collect::<Vec<_>>();
            validate_choices(&choices, files)?;
            for choice in &descriptor.plugins {
                validate_mcp_endpoint(&choice.endpoint)?;
            }
        }
        _ => {
            return Err(anyhow!(
                "collaboration_plugin_workflow_descriptor_schema_invalid"
            ));
        }
    }
    Ok(())
}

fn validate_mcp_endpoint(value: &str) -> Result<String> {
    use std::net::IpAddr;

    let endpoint =
        url::Url::parse(value).map_err(|_| anyhow!("collaboration_plugin_mcp_endpoint_invalid"))?;
    ensure!(
        value == value.trim()
            && value.len() <= 2048
            && !endpoint.cannot_be_a_base()
            && endpoint.username().is_empty()
            && endpoint.password().is_none()
            && endpoint.fragment().is_none(),
        "collaboration_plugin_mcp_endpoint_invalid"
    );
    let host = endpoint
        .host_str()
        .ok_or_else(|| anyhow!("collaboration_plugin_mcp_endpoint_invalid"))?;
    let loopback = host.eq_ignore_ascii_case("localhost")
        || host
            .parse::<IpAddr>()
            .is_ok_and(|address| address.is_loopback());
    ensure!(
        endpoint.scheme() == "https" || endpoint.scheme() == "http" && loopback,
        "collaboration_plugin_mcp_endpoint_requires_https"
    );
    Ok(endpoint.to_string())
}

fn validate_choices(choices: &[(&str, &str, &str, &str)], files: &[PackageFile]) -> Result<()> {
    ensure!(
        !choices.is_empty() && choices.len() <= 256,
        "collaboration_plugin_workflow_choice_count_invalid"
    );
    let mut ids = BTreeSet::new();
    let mut paths = BTreeSet::new();
    for (id, label, description, package_path) in choices {
        ensure!(
            is_slug(id) && ids.insert(*id),
            "collaboration_plugin_workflow_choice_id_invalid"
        );
        ensure!(
            is_bounded_text(label, 255) && description.len() <= 2048,
            "collaboration_plugin_workflow_choice_text_invalid"
        );
        let package_path = validate_relative_path(
            package_path,
            "collaboration_plugin_workflow_package_path_invalid",
        )?;
        ensure!(
            paths.insert(package_path.clone())
                && files.iter().any(|file| {
                    file.relative_path == package_path
                        || file.relative_path.starts_with(&package_path)
                }),
            "collaboration_plugin_workflow_package_payload_missing"
        );
    }
    Ok(())
}

fn is_slug(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && value
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
        && value
            .bytes()
            .next()
            .is_some_and(|byte| byte.is_ascii_lowercase())
        && !value.ends_with('-')
        && !value.contains("--")
}

fn is_bounded_text(value: &str, max_bytes: usize) -> bool {
    value == value.trim()
        && !value.is_empty()
        && value.len() <= max_bytes
        && !value.chars().any(char::is_control)
}

fn reject_executable_directives(value: &Value) -> Result<()> {
    match value {
        Value::Object(object) => {
            for (key, child) in object {
                ensure!(
                    !matches!(
                        key.as_str(),
                        "argv"
                            | "command"
                            | "executable"
                            | "hook"
                            | "hooks"
                            | "process"
                            | "script"
                            | "shell"
                    ),
                    "collaboration_plugin_executable_directive_rejected"
                );
                reject_executable_directives(child)?;
            }
        }
        Value::Array(values) => {
            for child in values {
                reject_executable_directives(child)?;
            }
        }
        _ => {}
    }
    Ok(())
}
