use anyhow::{Result, anyhow, ensure};
use serde::Deserialize;
use std::collections::BTreeSet;
use std::path::{Component, Path, PathBuf};

pub(super) const MANIFEST_FILE: &str = "licoup-collaboration-plugin.json";
pub(super) const MANIFEST_SCHEMA: &str = "licoup.optional-collaboration-plugin.v2";
pub(super) const PLUGIN_KIND: &str = "licomesh-collaboration";
pub(super) const LOCAL_DEPLOYMENT_CAPABILITY: &str = "licomesh.local-deployment.compose";
pub(super) const MCP_INSTALL_CAPABILITY: &str = "licomesh.mcp.install";
pub(super) const SERVER_RUNNER_CONTRACT: &str = "licoup.local-server-runner.v2";
pub(super) const SERVER_HEALTH_CONTRACT: &str = "licoup.local-server-health.v1";
pub(super) const SERVER_CAPABILITIES_CONTRACT: &str = "licoup.local-server-capabilities.v1";
const MAX_TEXT_BYTES: usize = 255;

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct PluginManifestDocument {
    schema_version: String,
    kind: String,
    plugin_id: String,
    display_name: String,
    version: String,
    capabilities: Vec<String>,
    workflows: WorkflowDescriptors,
    server_runners: Vec<ServerRunnerDocument>,
    signed_package_inventory_digest_sha256: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct WorkflowDescriptors {
    local_deployment: String,
    mcp_install: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct ServerRunnerDocument {
    source_url: String,
    source_commit_oid: String,
    platform: String,
    architecture: String,
    relative_path: String,
    digest_sha256: String,
    runner_contract_version: String,
    health_contract_version: String,
    capabilities_contract_version: String,
    signature_base64url: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct ValidatedServerRunner {
    pub(super) source_url: String,
    pub(super) source_commit_oid: String,
    pub(super) platform: String,
    pub(super) architecture: String,
    pub(super) relative_path: PathBuf,
    pub(super) digest_sha256: String,
    pub(super) runner_contract_version: String,
    pub(super) health_contract_version: String,
    pub(super) capabilities_contract_version: String,
    pub(super) signature_base64url: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct ValidatedManifest {
    pub plugin_id: String,
    pub display_name: String,
    pub version: String,
    pub capabilities: Vec<String>,
    pub local_deployment_descriptor: PathBuf,
    pub mcp_install_descriptor: PathBuf,
    pub server_runners: Vec<ValidatedServerRunner>,
    pub signed_package_inventory_digest_sha256: String,
}

pub(super) fn parse_manifest(bytes: &[u8]) -> Result<ValidatedManifest> {
    let document: PluginManifestDocument = serde_json::from_slice(bytes)
        .map_err(|_| anyhow!("collaboration_plugin_manifest_invalid"))?;
    ensure!(
        document.schema_version == MANIFEST_SCHEMA,
        "collaboration_plugin_manifest_schema_invalid"
    );
    ensure!(
        document.kind == PLUGIN_KIND,
        "collaboration_plugin_kind_invalid"
    );
    validate_slug(&document.plugin_id, "collaboration_plugin_id_invalid")?;
    validate_text(
        &document.display_name,
        "collaboration_plugin_display_name_invalid",
    )?;
    validate_text(&document.version, "collaboration_plugin_version_invalid")?;

    let capabilities = document.capabilities.into_iter().collect::<BTreeSet<_>>();
    ensure!(
        capabilities.len() == 2
            && capabilities.contains(LOCAL_DEPLOYMENT_CAPABILITY)
            && capabilities.contains(MCP_INSTALL_CAPABILITY),
        "collaboration_plugin_capabilities_invalid"
    );
    let local_deployment_descriptor = validate_relative_path(
        &document.workflows.local_deployment,
        "collaboration_plugin_local_deployment_descriptor_invalid",
    )?;
    let mcp_install_descriptor = validate_relative_path(
        &document.workflows.mcp_install,
        "collaboration_plugin_mcp_install_descriptor_invalid",
    )?;
    ensure!(
        local_deployment_descriptor != mcp_install_descriptor,
        "collaboration_plugin_workflow_descriptors_conflict"
    );
    let server_runners = validate_server_runners(document.server_runners)?;
    ensure!(
        is_sha256(&document.signed_package_inventory_digest_sha256),
        "collaboration_plugin_signed_inventory_digest_invalid"
    );

    Ok(ValidatedManifest {
        plugin_id: document.plugin_id,
        display_name: document.display_name,
        version: document.version,
        capabilities: capabilities.into_iter().collect(),
        local_deployment_descriptor,
        mcp_install_descriptor,
        server_runners,
        signed_package_inventory_digest_sha256: document.signed_package_inventory_digest_sha256,
    })
}

fn validate_server_runners(
    documents: Vec<ServerRunnerDocument>,
) -> Result<Vec<ValidatedServerRunner>> {
    ensure!(
        !documents.is_empty() && documents.len() <= 6,
        "collaboration_plugin_server_runner_count_invalid"
    );
    let mut targets = BTreeSet::new();
    let mut runners = documents
        .into_iter()
        .map(|document| {
            ensure!(
                matches!(document.platform.as_str(), "macos" | "windows" | "ubuntu")
                    && matches!(document.architecture.as_str(), "x86_64" | "aarch64")
                    && targets.insert((document.platform.clone(), document.architecture.clone())),
                "collaboration_plugin_server_runner_target_invalid"
            );
            let expected_path =
                expected_server_runner_path(&document.platform, &document.architecture);
            let relative_path = validate_relative_path(
                &document.relative_path,
                "collaboration_plugin_server_runner_path_invalid",
            )?;
            ensure!(
                relative_path == expected_path
                    && is_github_source_url(&document.source_url)
                    && is_commit_oid(&document.source_commit_oid)
                    && is_sha256(&document.digest_sha256)
                    && document.runner_contract_version == SERVER_RUNNER_CONTRACT
                    && document.health_contract_version == SERVER_HEALTH_CONTRACT
                    && document.capabilities_contract_version == SERVER_CAPABILITIES_CONTRACT,
                "collaboration_plugin_server_runner_contract_invalid"
            );
            Ok(ValidatedServerRunner {
                source_url: document.source_url,
                source_commit_oid: document.source_commit_oid,
                platform: document.platform,
                architecture: document.architecture,
                relative_path,
                digest_sha256: document.digest_sha256,
                runner_contract_version: document.runner_contract_version,
                health_contract_version: document.health_contract_version,
                capabilities_contract_version: document.capabilities_contract_version,
                signature_base64url: validate_signature_text(&document.signature_base64url)?,
            })
        })
        .collect::<Result<Vec<_>>>()?;
    runners.sort_by(|left, right| {
        (&left.platform, &left.architecture).cmp(&(&right.platform, &right.architecture))
    });
    Ok(runners)
}

pub(super) fn current_server_runner_target() -> Result<(&'static str, &'static str)> {
    let platform = match std::env::consts::OS {
        "macos" => "macos",
        "windows" => "windows",
        "linux" => "ubuntu",
        _ => {
            return Err(anyhow!(
                "collaboration_plugin_server_runner_platform_unsupported"
            ));
        }
    };
    let architecture = match std::env::consts::ARCH {
        "x86_64" => "x86_64",
        "aarch64" => "aarch64",
        _ => {
            return Err(anyhow!(
                "collaboration_plugin_server_runner_architecture_unsupported"
            ));
        }
    };
    Ok((platform, architecture))
}

pub(super) fn expected_server_runner_path(platform: &str, architecture: &str) -> PathBuf {
    let executable = if platform == "windows" {
        "licomesh-server-runner.exe"
    } else {
        "licomesh-server-runner"
    };
    PathBuf::from("runners")
        .join(platform)
        .join(architecture)
        .join(executable)
}

fn is_sha256(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
}

fn is_commit_oid(value: &str) -> bool {
    value.len() == 40
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
}

fn is_github_source_url(value: &str) -> bool {
    url::Url::parse(value).is_ok_and(|url| {
        url.scheme() == "https"
            && url.host_str() == Some("github.com")
            && url.port().is_none()
            && url.query().is_none()
            && url.fragment().is_none()
            && url.username().is_empty()
            && url.password().is_none()
            && url.path().ends_with(".git")
            && url
                .path_segments()
                .is_some_and(|segments| segments.filter(|segment| !segment.is_empty()).count() == 2)
    })
}

fn validate_signature_text(value: &str) -> Result<String> {
    use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
    ensure!(
        value == value.trim()
            && URL_SAFE_NO_PAD
                .decode(value)
                .is_ok_and(|bytes| bytes.len() == 64),
        "collaboration_plugin_server_runner_signature_invalid"
    );
    Ok(value.to_owned())
}

fn validate_slug(value: &str, code: &'static str) -> Result<()> {
    ensure!(
        !value.is_empty()
            && value.len() <= MAX_TEXT_BYTES
            && value
                .bytes()
                .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
            && value
                .bytes()
                .next()
                .is_some_and(|byte| byte.is_ascii_lowercase())
            && !value.ends_with('-')
            && !value.contains("--"),
        code
    );
    Ok(())
}

fn validate_text(value: &str, code: &'static str) -> Result<()> {
    ensure!(
        value == value.trim()
            && !value.is_empty()
            && value.len() <= MAX_TEXT_BYTES
            && !value.chars().any(char::is_control),
        code
    );
    Ok(())
}

pub(super) fn validate_relative_path(value: &str, code: &'static str) -> Result<PathBuf> {
    ensure!(
        value == value.trim() && !value.is_empty() && value.len() <= 1024 && !value.contains('\\'),
        code
    );
    let path = Path::new(value);
    ensure!(!path.is_absolute(), code);
    let mut component_count = 0usize;
    for component in path.components() {
        match component {
            Component::Normal(value) => {
                let value = value.to_str().ok_or_else(|| anyhow!(code))?;
                ensure!(
                    !value.is_empty()
                        && value != "."
                        && value != ".."
                        && value.bytes().all(|byte| byte.is_ascii_alphanumeric()
                            || matches!(byte, b'.' | b'_' | b'-')),
                    code
                );
                component_count += 1;
            }
            _ => return Err(anyhow!(code)),
        }
    }
    ensure!(component_count > 0, code);
    Ok(path.to_path_buf())
}

pub(super) fn normalized_relative_protocol_path(path: &Path) -> Result<String> {
    validate_relative_path(
        path.to_str()
            .ok_or_else(|| anyhow!("collaboration_plugin_package_path_encoding_invalid"))?,
        "collaboration_plugin_package_path_invalid",
    )?;
    let parts = path
        .components()
        .map(|component| match component {
            Component::Normal(value) => value
                .to_str()
                .map(str::to_owned)
                .ok_or_else(|| anyhow!("collaboration_plugin_package_path_encoding_invalid")),
            _ => Err(anyhow!("collaboration_plugin_package_path_invalid")),
        })
        .collect::<Result<Vec<_>>>()?;
    ensure!(
        !parts.is_empty(),
        "collaboration_plugin_package_path_invalid"
    );
    Ok(parts.join("/"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
    use serde_json::json;

    fn valid_manifest() -> Vec<u8> {
        let (platform, architecture) = current_server_runner_target().unwrap();
        serde_json::to_vec(&json!({
            "schemaVersion": MANIFEST_SCHEMA,
            "kind": PLUGIN_KIND,
            "pluginId": "licomesh-collaboration",
            "displayName": "LicoMesh Collaboration",
            "version": "1.0.0",
            "capabilities": [LOCAL_DEPLOYMENT_CAPABILITY, MCP_INSTALL_CAPABILITY],
            "workflows": {
                "localDeployment": "workflows/local-deployment.json",
                "mcpInstall": "workflows/mcp-install.json"
            },
            "signedPackageInventoryDigestSha256": "b".repeat(64),
            "serverRunners": [{
                "sourceUrl": "https://github.com/example/collaboration-plugin.git",
                "sourceCommitOid": "c".repeat(40),
                "platform": platform,
                "architecture": architecture,
                "relativePath": expected_server_runner_path(platform, architecture),
                "digestSha256": "a".repeat(64),
                "runnerContractVersion": SERVER_RUNNER_CONTRACT,
                "healthContractVersion": SERVER_HEALTH_CONTRACT,
                "capabilitiesContractVersion": SERVER_CAPABILITIES_CONTRACT,
                "signatureBase64url": URL_SAFE_NO_PAD.encode([0u8; 64])
            }]
        }))
        .unwrap()
    }

    #[test]
    fn declarative_manifest_accepts_only_the_two_collaboration_capabilities() {
        let manifest = parse_manifest(&valid_manifest()).unwrap();
        assert_eq!(manifest.plugin_id, "licomesh-collaboration");
        assert_eq!(manifest.capabilities.len(), 2);
    }

    #[test]
    fn executable_hooks_and_unknown_fields_are_rejected() {
        let mut value: serde_json::Value = serde_json::from_slice(&valid_manifest()).unwrap();
        value["executable"] = json!("plugin.sh");
        assert!(parse_manifest(&serde_json::to_vec(&value).unwrap()).is_err());
    }

    #[test]
    fn workflow_descriptors_cannot_escape_the_package() {
        let mut value: serde_json::Value = serde_json::from_slice(&valid_manifest()).unwrap();
        value["workflows"]["mcpInstall"] = json!("../outside.json");
        assert!(parse_manifest(&serde_json::to_vec(&value).unwrap()).is_err());
    }
}
