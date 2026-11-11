//! Agent Hub contract: recipe registry, capabilities, ownership, and lifecycle.

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

pub const SCHEMA_VERSION: &str = "v0.0.2:client-agent-hub-manifest-1";
pub const HOST_SCOPE: &str = "desktop";
pub const PLUGIN_MANAGEMENT_BOUNDARY: &str = "adapter-plugins-only";
pub const DEEP_ADAPTATION_IDS: [&str; 7] = [
    "codex",
    "cursor",
    "opencode",
    "claude-code",
    "pi",
    "openclaw",
    "hermes",
];
pub const PARTIAL_ADAPTATION_ID: &str = "antigravity";
pub const PENDING_ADAPTATION_ID: &str = "deepseek-harness";

pub const ADAPTATION_DEEP: &str = "deep";
pub const ADAPTATION_PARTIAL: &str = "partial";
pub const ADAPTATION_PENDING: &str = "pending-evaluation";

pub const OWNERSHIP_NONE: &str = "none";
pub const OWNERSHIP_EXTERNAL: &str = "external";
pub const OWNERSHIP_OWNED: &str = "owned";

pub const LIFECYCLE_DISCOVERED: &str = "discovered";
pub const LIFECYCLE_PLANNED: &str = "planned";
pub const LIFECYCLE_CONFIRMED: &str = "confirmed";
pub const LIFECYCLE_APPLYING: &str = "applying";
pub const LIFECYCLE_VERIFYING: &str = "verifying";
pub const LIFECYCLE_RESCANNING: &str = "rescanning";
pub const LIFECYCLE_AVAILABLE: &str = "available";
pub const LIFECYCLE_NEEDS_LOGIN: &str = "needs-login";
pub const LIFECYCLE_FAILED: &str = "failed";

pub const CHANNEL_HOMEBREW: &str = "homebrew";
pub const CHANNEL_NPM: &str = "npm";
pub const CHANNEL_WINGET: &str = "winget";
pub const CHANNEL_OFFICIAL_ARTIFACT: &str = "official-artifact";

/// Install channel classes, in the order the Hub presents them.
///
/// A binary channel ships the vendor's own program and needs nothing else on
/// the machine; a package-manager channel needs an operating-system package
/// manager; a vendor-script channel needs a language runtime, which is a
/// developer toolchain on a user's machine.
pub const CHANNEL_CLASS_BINARY: &str = "binary";
pub const CHANNEL_CLASS_PACKAGE_MANAGER: &str = "package-manager";
pub const CHANNEL_CLASS_VENDOR_SCRIPT: &str = "vendor-script";

/// Channel kinds that ship the vendor's own program.
pub const BINARY_CHANNEL_KINDS: [&str; 2] = [CHANNEL_OFFICIAL_ARTIFACT, "binary"];

/// Channel kinds installed through a language runtime rather than through an
/// operating system package manager. A recipe that only offers these cannot be
/// installed on a machine without a developer toolchain.
pub const VENDOR_SCRIPT_CHANNEL_KINDS: [&str; 11] = [
    CHANNEL_NPM,
    "pnpm",
    "yarn",
    "bun",
    "deno",
    "pip",
    "pip3",
    "cargo",
    "gem",
    "go",
    "vendor-script",
];

/// Managers that only exist alongside a developer toolchain.
pub const DEVELOPER_TOOLCHAIN_MANAGERS: [&str; 16] = [
    CHANNEL_NPM,
    "pnpm",
    "yarn",
    "bun",
    "deno",
    "pip",
    "pip3",
    "python",
    "python3",
    "cargo",
    "rustup",
    "gem",
    "go",
    "dotnet",
    "maven",
    "gradle",
];

/// The class one install channel belongs to.
pub fn channel_class(channel: &InstallChannel) -> &'static str {
    let kind = channel.kind.as_str();
    if BINARY_CHANNEL_KINDS.contains(&kind) {
        CHANNEL_CLASS_BINARY
    } else if VENDOR_SCRIPT_CHANNEL_KINDS.contains(&kind)
        || DEVELOPER_TOOLCHAIN_MANAGERS.contains(&channel.requires_manager.as_str())
    {
        CHANNEL_CLASS_VENDOR_SCRIPT
    } else {
        CHANNEL_CLASS_PACKAGE_MANAGER
    }
}

/// The rank the Hub presents channels in: binary first, then an operating
/// system package manager, then a developer toolchain.
pub fn channel_class_rank(channel: &InstallChannel) -> i32 {
    match channel_class(channel) {
        CHANNEL_CLASS_BINARY => 0,
        CHANNEL_CLASS_PACKAGE_MANAGER => 1,
        _ => 2,
    }
}

/// Whether installing through one channel needs a developer toolchain.
pub fn channel_requires_developer_toolchain(channel: &InstallChannel) -> bool {
    channel_class(channel) == CHANNEL_CLASS_VENDOR_SCRIPT
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct AgentHubManifest {
    pub schema_version: String,
    pub host_scope: String,
    pub plugin_management_boundary: String,
    pub adaptation_tags: Vec<String>,
    pub channel_kinds: Vec<String>,
    pub agents: Vec<ManifestAgent>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct ManifestAgent {
    pub id: String,
    pub label: String,
    pub adaptation: String,
    pub protocol: String,
    pub license: String,
    pub summary: String,
    pub homepage: String,
    #[serde(default)]
    pub requires_login: bool,
    #[serde(default)]
    pub connection_modes: Vec<String>,
    pub file: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct AgentTomlDocument {
    pub id: String,
    pub binary_names: Vec<String>,
    pub official_docs: String,
    pub channels: Vec<InstallChannel>,
    #[serde(default)]
    pub unsupported: Vec<UnsupportedCombination>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RecipeRegistryDocument {
    pub schema_version: String,
    pub host_scope: String,
    pub plugin_management_boundary: String,
    pub adaptation_tags: Vec<String>,
    pub channel_kinds: Vec<String>,
    pub agents: Vec<AgentRecipe>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentRecipe {
    pub id: String,
    pub label: String,
    pub adaptation: String,
    pub binary_names: Vec<String>,
    pub protocol: String,
    pub license: String,
    pub summary: String,
    pub homepage: String,
    #[serde(default)]
    pub requires_login: bool,
    #[serde(default)]
    pub connection_modes: Vec<String>,
    pub official_docs: String,
    pub channels: Vec<InstallChannel>,
    #[serde(default)]
    pub unsupported: Vec<UnsupportedCombination>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct InstallChannel {
    pub id: String,
    pub kind: String,
    pub oses: Vec<String>,
    #[serde(default)]
    pub architectures: Vec<String>,
    pub priority: i32,
    #[serde(default)]
    pub official_recommended: bool,
    #[serde(default)]
    pub licoup_verified: bool,
    pub requires_manager: String,
    #[serde(default = "none_elevation")]
    pub elevation: String,
    #[serde(default = "user_scope")]
    pub scope: String,
    #[serde(default = "default_selectable")]
    pub selectable: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unsupported_reason: Option<String>,
    pub package_coordinate: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub package_form: Option<String>,
    pub official_source: String,
    pub version_policy: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub artifact: Option<ArtifactSpec>,
    /// Where this channel installs its executable, when the recipe names it.
    ///
    /// A declared destination is what `{install}` resolves to, so uninstall
    /// names a path the recipe declared rather than one the caller had to
    /// supply. Its directory is a per-OS template written in the discovery
    /// owner's own root vocabulary, so the Hub installs only where the Agent
    /// scanner already looks.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub install: Option<InstallPlacement>,
    #[serde(default)]
    pub install_argv: Vec<String>,
    #[serde(default)]
    pub windows_install_argv: Vec<String>,
    #[serde(default)]
    pub update_argv: Vec<String>,
    #[serde(default)]
    pub uninstall_argv: Vec<String>,
    #[serde(default)]
    pub verify_argv: Vec<String>,
}

/// The declared install destination of one channel.
///
/// Two placements exist and the declaration says which one a channel uses. A
/// channel whose [`argv`](Self::argv) is empty hands placement to the vendor's
/// own installer, which the operation runs and which lands the executable at
/// the declared destination. A channel that declares an
/// [`argv`](Self::argv) places the staged result itself, in one visible step
/// whose only write target is `{install}`.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct InstallPlacement {
    /// The installed executable's file name, per OS.
    #[serde(default)]
    pub binary: std::collections::BTreeMap<String, String>,
    /// The destination directory, per OS, in the discovery root vocabulary
    /// (`{home}`, `{local_appdata}`, ...). An OS the channel lists but does not
    /// declare here cannot resolve `{install}` and is refused.
    #[serde(default)]
    pub dir: std::collections::BTreeMap<String, String>,
    /// The argv step that places the staged result at the destination.
    ///
    /// Empty when the vendor's own installer performs placement. When present
    /// it runs after the operation's argv, it must name `{install}` as its
    /// target, and it may only read from the Hub's private staging root.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub argv: Vec<String>,
}

fn none_elevation() -> String {
    "none".to_string()
}

fn user_scope() -> String {
    "user".to_string()
}

fn default_selectable() -> bool {
    true
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct ArtifactSpec {
    pub origin_host: String,
    pub url_template: String,
    #[serde(default)]
    pub vendor_os: std::collections::BTreeMap<String, String>,
    #[serde(default)]
    pub vendor_arch: std::collections::BTreeMap<String, String>,
    #[serde(default)]
    pub installer: std::collections::BTreeMap<String, String>,
    /// The hosts this recipe explicitly permits as redirect targets.
    ///
    /// [`origin_host`](Self::origin_host) alone pins the first request: the URL
    /// the recipe builds must resolve there. Vendors that hand their own
    /// downloads to a content network redirect that request to a host they name
    /// in `Location`, and a recipe may name exactly those hosts here. The
    /// declaration widens redirect hops only — it can never satisfy the first
    /// request — it is validated at registry load, and it is per recipe, so no
    /// host becomes reachable for a channel that did not declare it.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub redirect_hosts: Vec<String>,
    /// The digest the vendor publishes for this artifact. Its absence is a
    /// refusal, not a silent unverified install: acquisition stages nothing
    /// until the recipe names the published digest or its document.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub integrity: Option<ArtifactIntegrity>,
}

/// Where the published digest of one vendor artifact comes from.
///
/// Exactly one of [`ArtifactIntegrity::digest`] (a digest the recipe pins, only
/// usable for an immutable URL) and [`ArtifactIntegrity::digest_url_template`]
/// (a digest document the vendor publishes beside the artifact) is declared.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct ArtifactIntegrity {
    pub algorithm: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub digest: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub digest_url_template: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct UnsupportedCombination {
    pub oses: Vec<String>,
    pub reason: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct PlatformInstallCapabilities {
    pub os: String,
    pub architecture: String,
    pub managers: Vec<String>,
    pub scan_generation: u64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct InstallOwnership {
    pub agent_id: String,
    pub channel_id: String,
    pub channel_kind: String,
    pub package_coordinate: String,
    pub installed_version: String,
    pub ownership: String,
    pub lifecycle: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DiscoveryFact {
    pub agent_id: String,
    pub present: bool,
    pub location: String,
    pub scan_source: String,
    #[serde(default)]
    pub installed_version: String,
    #[serde(default)]
    pub latest_version: String,
    /// Install forms detected on this host (`desktop`, `cli`, `ide`,
    /// `plugin`) — the card's type badges. Empty when nothing was found.
    #[serde(default)]
    pub install_kinds: Vec<String>,
    /// Exact target-discovery binding used only by the bounded native version
    /// probe. It is never serialized into an Agent Hub card or receipt.
    #[serde(skip)]
    pub(crate) executable_binding: Option<std::path::PathBuf>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HubEvent {
    pub phase: String,
    pub code: String,
}

pub fn contract_surface() -> Value {
    json!({
        "schemaVersion": SCHEMA_VERSION,
        "hostScope": HOST_SCOPE,
        "pluginManagementBoundary": PLUGIN_MANAGEMENT_BOUNDARY,
        "adaptation": {
            "deep": DEEP_ADAPTATION_IDS,
            "partial": [PARTIAL_ADAPTATION_ID],
            "pendingEvaluation": ADAPTATION_PENDING
        },
        "channelKinds": [CHANNEL_HOMEBREW, CHANNEL_NPM, CHANNEL_WINGET, CHANNEL_OFFICIAL_ARTIFACT],
        "channelClasses": [
            CHANNEL_CLASS_BINARY,
            CHANNEL_CLASS_PACKAGE_MANAGER,
            CHANNEL_CLASS_VENDOR_SCRIPT
        ],
        "ownership": [OWNERSHIP_NONE, OWNERSHIP_EXTERNAL, OWNERSHIP_OWNED],
        "lifecycle": [
            LIFECYCLE_DISCOVERED,
            LIFECYCLE_PLANNED,
            LIFECYCLE_CONFIRMED,
            LIFECYCLE_APPLYING,
            LIFECYCLE_VERIFYING,
            LIFECYCLE_RESCANNING,
            LIFECYCLE_AVAILABLE,
            LIFECYCLE_NEEDS_LOGIN,
            LIFECYCLE_FAILED
        ],
        "operations": ["install", "update", "uninstall", "verify", "rescan"],
        "confirmation": "single-use-plan-token"
    })
}
