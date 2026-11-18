//! Warehouse Manifest plus one TOML install recipe per agent.

use super::argv::{self, ArgvKind};
use super::contract::{
    ADAPTATION_DEEP, ADAPTATION_PARTIAL, ADAPTATION_PENDING, AgentHubManifest, AgentRecipe,
    AgentTomlDocument, HOST_SCOPE, InstallChannel, ManifestAgent, PARTIAL_ADAPTATION_ID,
    PENDING_ADAPTATION_ID, PLUGIN_MANAGEMENT_BOUNDARY, RecipeRegistryDocument, SCHEMA_VERSION,
};
use anyhow::{Result, anyhow, ensure};
use std::sync::OnceLock;
use url::Url;

const MANIFEST_TOML: &str = include_str!("../../../resources/agent-hub/manifest.toml");

static WAREHOUSE: OnceLock<Warehouse> = OnceLock::new();

pub struct Warehouse {
    pub manifest: AgentHubManifest,
    pub registry: RecipeRegistryDocument,
}

pub fn warehouse() -> Result<&'static Warehouse> {
    if let Some(loaded) = WAREHOUSE.get() {
        return Ok(loaded);
    }
    let loaded = load_warehouse(MANIFEST_TOML)?;
    Ok(WAREHOUSE.get_or_init(|| loaded))
}

pub fn manifest() -> Result<&'static AgentHubManifest> {
    Ok(&warehouse()?.manifest)
}

pub fn registry() -> Result<&'static RecipeRegistryDocument> {
    Ok(&warehouse()?.registry)
}

pub fn parse_manifest(raw: &str) -> Result<AgentHubManifest> {
    let document: AgentHubManifest = toml::from_str(raw)
        .map_err(|error| anyhow!("agent hub manifest is invalid TOML: {error}"))?;
    validate_manifest(&document)?;
    Ok(document)
}

pub fn parse_agent_toml(raw: &str) -> Result<AgentTomlDocument> {
    toml::from_str(raw).map_err(|error| anyhow!("agent recipe is invalid TOML: {error}"))
}

pub fn agent_recipe<'a>(
    document: &'a RecipeRegistryDocument,
    agent_id: &str,
) -> Result<&'a AgentRecipe> {
    document
        .agents
        .iter()
        .find(|agent| agent.id == agent_id)
        .ok_or_else(|| anyhow!("recipe_not_found"))
}

fn load_warehouse(manifest_raw: &str) -> Result<Warehouse> {
    let manifest = parse_manifest(manifest_raw)?;
    let mut agents = Vec::with_capacity(manifest.agents.len());
    for entry in &manifest.agents {
        let raw = agent_toml_source(entry)?;
        let document = parse_agent_toml(raw)?;
        let agent = merge_agent(entry, document)?;
        validate_agent(&agent)?;
        agents.push(agent);
    }
    let registry = RecipeRegistryDocument {
        schema_version: manifest.schema_version.clone(),
        host_scope: manifest.host_scope.clone(),
        plugin_management_boundary: manifest.plugin_management_boundary.clone(),
        adaptation_tags: manifest.adaptation_tags.clone(),
        channel_kinds: manifest.channel_kinds.clone(),
        agents,
    };
    Ok(Warehouse { manifest, registry })
}

fn merge_agent(entry: &ManifestAgent, document: AgentTomlDocument) -> Result<AgentRecipe> {
    ensure!(
        document.id == entry.id,
        "agent toml id must match manifest id {}",
        entry.id
    );
    Ok(AgentRecipe {
        id: entry.id.clone(),
        label: entry.label.clone(),
        adaptation: entry.adaptation.clone(),
        binary_names: document.binary_names,
        protocol: entry.protocol.clone(),
        license: entry.license.clone(),
        summary: entry.summary.clone(),
        homepage: entry.homepage.clone(),
        requires_login: entry.requires_login,
        connection_modes: entry.connection_modes.clone(),
        official_docs: document.official_docs,
        channels: document.channels,
        unsupported: document.unsupported,
    })
}

fn agent_toml_source(entry: &ManifestAgent) -> Result<&'static str> {
    let expected = format!("{}.toml", entry.id);
    ensure!(
        entry.file == expected,
        "manifest file for {} must be {}",
        entry.id,
        expected
    );
    match entry.id.as_str() {
        "codex" => Ok(include_str!("../../../resources/agent-hub/codex.toml")),
        "cursor" => Ok(include_str!("../../../resources/agent-hub/cursor.toml")),
        "opencode" => Ok(include_str!("../../../resources/agent-hub/opencode.toml")),
        "claude-code" => Ok(include_str!(
            "../../../resources/agent-hub/claude-code.toml"
        )),
        "pi" => Ok(include_str!("../../../resources/agent-hub/pi.toml")),
        "openclaw" => Ok(include_str!("../../../resources/agent-hub/openclaw.toml")),
        "hermes" => Ok(include_str!("../../../resources/agent-hub/hermes.toml")),
        "antigravity" => Ok(include_str!(
            "../../../resources/agent-hub/antigravity.toml"
        )),
        "deepseek-harness" => Ok(include_str!(
            "../../../resources/agent-hub/deepseek-harness.toml"
        )),
        _ => Err(anyhow!("recipe_not_found")),
    }
}

fn validate_manifest(document: &AgentHubManifest) -> Result<()> {
    ensure!(
        document.schema_version == SCHEMA_VERSION,
        "agent hub manifest schema version is unsupported"
    );
    ensure!(
        document.host_scope == HOST_SCOPE,
        "agent hub host scope must be desktop"
    );
    ensure!(
        document.plugin_management_boundary == PLUGIN_MANAGEMENT_BOUNDARY,
        "agent hub must not own adapter plugin lifecycle"
    );
    let mut seen = std::collections::BTreeSet::new();
    for agent in &document.agents {
        ensure!(
            seen.insert(agent.id.as_str()),
            "duplicate install recipe id {}",
            agent.id
        );
        let expected_adaptation = if agent.id == PARTIAL_ADAPTATION_ID {
            ADAPTATION_PARTIAL
        } else if agent.id == PENDING_ADAPTATION_ID {
            ADAPTATION_PENDING
        } else {
            ADAPTATION_DEEP
        };
        ensure!(
            agent.adaptation == expected_adaptation,
            "adaptation tag is incorrect for {}",
            agent.id
        );
        ensure!(
            !agent.summary.trim().is_empty(),
            "official summary is required for {}",
            agent.id
        );
        validate_https(&agent.homepage)?;
        if agent.id == "openclaw" || agent.id == "hermes" {
            ensure!(
                agent.connection_modes.iter().any(|mode| mode == "local")
                    && agent
                        .connection_modes
                        .iter()
                        .any(|mode| mode == "virtual-machine"),
                "{} must express local and virtual-machine connection on one card",
                agent.id
            );
        }
    }
    Ok(())
}

pub(crate) fn validate_agent(agent: &AgentRecipe) -> Result<()> {
    ensure!(
        agent.official_docs.starts_with("https://"),
        "official docs must be HTTPS"
    );
    let mut kinds = std::collections::BTreeSet::new();
    for channel in &agent.channels {
        ensure!(
            kinds.insert(channel.kind.as_str()),
            "channel kinds must be unique per agent"
        );
        validate_https(&channel.official_source)?;
        validate_install_placement(channel)?;
        if channel.selectable {
            ensure!(
                !channel.install_argv.is_empty() || !channel.windows_install_argv.is_empty(),
                "selectable channel {} requires argv",
                channel.id
            );
            argv::validate(&channel.install_argv, ArgvKind::for_channel(&channel.kind))?;
            if !channel.windows_install_argv.is_empty() {
                argv::validate(
                    &channel.windows_install_argv,
                    ArgvKind::for_channel(&channel.kind),
                )?;
            }
            argv::validate(&channel.update_argv, ArgvKind::for_channel(&channel.kind))?;
            argv::validate(&channel.uninstall_argv, ArgvKind::Lifecycle)?;
            argv::validate(&channel.verify_argv, ArgvKind::Lifecycle)?;
        } else {
            ensure!(
                channel.install_argv.is_empty(),
                "non-selectable channel must not carry install argv"
            );
        }
        if let Some(artifact) = &channel.artifact {
            ensure!(
                !artifact.origin_host.is_empty(),
                "official artifact origin host is required"
            );
            validate_https_template(&artifact.url_template)?;
            validate_redirect_hosts(artifact)?;
            let host = Url::parse(
                &artifact
                    .url_template
                    .replace("{version}", "latest")
                    .replace("{vendorOs}", "darwin")
                    .replace("{vendorArch}", "arm64")
                    .replace("{installer}", "install.sh"),
            )
            .ok()
            .and_then(|url| url.host_str().map(str::to_string));
            if let Some(host) = host {
                ensure!(
                    host == artifact.origin_host
                        || host.ends_with(&format!(".{}", artifact.origin_host)),
                    "artifact URL host must match originHost"
                );
            }
            if let Some(integrity) = &artifact.integrity {
                validate_artifact_integrity(integrity, artifact)?;
            }
        }
        if channel.oses.contains(&"windows".to_string())
            && channel.selectable
            && channel.install_argv.first().map(String::as_str) == Some("bash")
        {
            ensure!(
                !channel.windows_install_argv.is_empty(),
                "windows official-artifact channels cannot use bash argv"
            );
        }
    }
    Ok(())
}

/// A declared install destination is where the Hub writes and what it removes.
///
/// It is held to the discovery owner's own root vocabulary, so a destination
/// always resolves under a root that owner knows and discovery can therefore
/// find it; to one binary file name per OS; and to a placement step that names
/// `{install}` as the path it writes, so the copy has one declared target. A
/// channel that states where its executable lands also verifies that path, never
/// a name PATH happens to answer.
fn validate_install_placement(channel: &InstallChannel) -> Result<()> {
    let Some(install) = channel.install.as_ref() else {
        return Ok(());
    };
    ensure!(
        !install.dir.is_empty(),
        "a declared install destination needs a directory"
    );
    for (os, template) in &install.dir {
        ensure!(
            channel.oses.iter().any(|declared| declared == os),
            "an install destination must name an OS the channel lists"
        );
        ensure!(
            install_dir_template_is_rooted(template),
            "an install destination must resolve under a declared host root"
        );
        ensure!(
            install.binary.contains_key(os),
            "every declared install directory needs its binary name"
        );
    }
    for (os, binary) in &install.binary {
        ensure!(
            channel.oses.iter().any(|declared| declared == os),
            "an installed binary must name an OS the channel lists"
        );
        ensure!(
            install.dir.contains_key(os),
            "every installed binary needs its declared directory"
        );
        ensure!(
            install_file_name_is_safe(binary),
            "an installed binary must be one file name"
        );
    }
    argv::validate(&install.argv, ArgvKind::for_channel(&channel.kind))?;
    if !install.argv.is_empty() {
        ensure!(
            install.argv.iter().any(|arg| arg.contains("{install}")),
            "a placement step must name {{install}} as the path it writes"
        );
    }
    ensure!(
        channel.verify_argv.first().map(String::as_str)
            == Some(super::version_check::INSTALL_VERIFY_PROGRAM),
        "a channel that declares an install destination must verify {{install}}"
    );
    Ok(())
}

/// Whether an install directory template is one root token plus a plain
/// relative path, in the vocabulary the discovery owner expands.
fn install_dir_template_is_rooted(template: &str) -> bool {
    let Some(rest) = licoup_agent_targets::domain::targets::scan_paths::PATH_TEMPLATE_ROOTS
        .iter()
        .find_map(|root| template.strip_prefix(&format!("{{{root}}}")))
    else {
        return false;
    };
    let rest = rest.strip_prefix('/').unwrap_or(rest);
    !rest.is_empty()
        && !rest.contains('{')
        && !rest.contains('\\')
        && !rest.contains("..")
        && !rest.starts_with('/')
        && !rest.ends_with('/')
}

/// Whether a declared binary is one safe file name rather than a path.
fn install_file_name_is_safe(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 128
        && name != "."
        && name != ".."
        && name
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || "._+-".contains(character))
}

/// A declared redirect host is one exact, fully qualified hostname.
///
/// The declaration widens redirect hops beyond the artifact's own origin, so it
/// is held to what acquisition can pin exactly: a bare lowercase hostname, one
/// entry per host, and never a host that the origin rule already admits — a
/// redirect host that could serve the first request would be a widened origin
/// wearing a redirect's name.
fn validate_redirect_hosts(artifact: &super::contract::ArtifactSpec) -> Result<()> {
    let origin = artifact.origin_host.trim().to_ascii_lowercase();
    let mut seen = std::collections::BTreeSet::new();
    for host in &artifact.redirect_hosts {
        ensure!(
            host.as_str() == host.trim() && !host.is_empty(),
            "artifact redirect hosts must be non-empty and unpadded"
        );
        ensure!(
            host.len() <= 253
                && host.chars().all(|character| character.is_ascii_lowercase()
                    || character.is_ascii_digit()
                    || character == '.'
                    || character == '-'),
            "artifact redirect host must be a bare lowercase hostname"
        );
        ensure!(
            host.contains('.') && !host.starts_with('.') && !host.ends_with('.'),
            "artifact redirect host must be fully qualified"
        );
        ensure!(
            host.split('.')
                .all(|label| !label.is_empty() && !label.starts_with('-') && !label.ends_with('-')),
            "artifact redirect host must not carry an empty or hyphen-edged label"
        );
        ensure!(
            !origin.is_empty() && host != &origin && !host.ends_with(&format!(".{origin}")),
            "a redirect host must not be reachable as the declared origin"
        );
        ensure!(
            seen.insert(host.as_str()),
            "artifact redirect hosts must be unique"
        );
    }
    Ok(())
}

/// One artifact declaration names exactly one published digest, and a digest
/// document must live on the artifact's own origin: a digest fetched from a
/// different host would prove nothing about the vendor's bytes.
fn validate_artifact_integrity(
    integrity: &super::contract::ArtifactIntegrity,
    artifact: &super::contract::ArtifactSpec,
) -> Result<()> {
    ensure!(
        integrity
            .algorithm
            .eq_ignore_ascii_case(super::acquisition::INTEGRITY_ALGORITHM_SHA256),
        "artifact integrity algorithm must be sha256"
    );
    match (
        integrity.digest.as_deref(),
        integrity.digest_url_template.as_deref(),
    ) {
        (Some(digest), None) => {
            let digest = digest.trim().to_ascii_lowercase();
            ensure!(
                digest.len() == 64
                    && digest
                        .chars()
                        .all(|character| character.is_ascii_hexdigit()),
                "artifact integrity digest must be 64 hexadecimal characters"
            );
        }
        (None, Some(template)) => {
            validate_https_template(template)?;
            let host = Url::parse(
                &template
                    .replace("{version}", "latest")
                    .replace("{vendorOs}", "darwin")
                    .replace("{vendorArch}", "arm64")
                    .replace("{installer}", "install.sh"),
            )
            .ok()
            .and_then(|url| url.host_str().map(str::to_string));
            if let Some(host) = host {
                ensure!(
                    host == artifact.origin_host
                        || host.ends_with(&format!(".{}", artifact.origin_host)),
                    "artifact digest URL host must match originHost"
                );
            }
        }
        _ => {
            return Err(anyhow!(
                "artifact integrity must declare exactly one published digest"
            ));
        }
    }
    Ok(())
}

fn validate_https(value: &str) -> Result<()> {
    let url = Url::parse(value).map_err(|_| anyhow!("official source must be an HTTPS URL"))?;
    ensure!(url.scheme() == "https", "official source must be HTTPS");
    ensure!(url.host_str().is_some(), "official source host is required");
    Ok(())
}

fn validate_https_template(template: &str) -> Result<()> {
    let preview = template
        .replace("{version}", "latest")
        .replace("{vendorOs}", "darwin")
        .replace("{vendorArch}", "arm64")
        .replace("{installer}", "install.sh");
    validate_https(&preview)
}
