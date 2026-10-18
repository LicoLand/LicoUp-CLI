use super::super::*;
use crate::domain::agent_hub::argv::{self, ArgvKind};
use crate::domain::agent_hub::contract::{
    ADAPTATION_DEEP, ADAPTATION_PARTIAL, ADAPTATION_PENDING, PARTIAL_ADAPTATION_ID,
    PENDING_ADAPTATION_ID,
};
use crate::domain::agent_hub::recipes::{manifest, parse_agent_toml, parse_manifest};
use crate::domain::agent_hub::selector;

#[test]
fn install_recipes_load_with_unique_ids_and_adaptation_tags() {
    let registry = registry().unwrap();
    let ids = registry
        .agents
        .iter()
        .map(|agent| agent.id.as_str())
        .collect::<Vec<_>>();
    let unique = ids
        .iter()
        .copied()
        .collect::<std::collections::BTreeSet<_>>();
    assert_eq!(unique.len(), ids.len());
    assert!(ids.contains(&"codex"));
    assert!(ids.contains(&"antigravity"));
    assert!(ids.contains(&"deepseek-harness"));
    for agent in &registry.agents {
        if agent.id == PARTIAL_ADAPTATION_ID {
            assert_eq!(agent.adaptation, ADAPTATION_PARTIAL);
        } else if agent.id == PENDING_ADAPTATION_ID {
            assert_eq!(agent.adaptation, ADAPTATION_PENDING);
        } else {
            assert_eq!(agent.adaptation, ADAPTATION_DEEP);
        }
        assert!(agent.summary.contains(' '));
        assert!(!agent.summary.to_lowercase().contains("rank #"));
        assert!(agent.homepage.starts_with("https://"));
        let kinds = agent
            .channels
            .iter()
            .map(|channel| channel.kind.as_str())
            .collect::<Vec<_>>();
        assert!(kinds.contains(&"homebrew"));
        assert!(kinds.contains(&"npm"));
        assert!(kinds.contains(&"winget") || agent.id == "hermes");
        assert!(kinds.contains(&"official-artifact") || agent.id == "pi");
    }
    let cursor = registry
        .agents
        .iter()
        .find(|agent| agent.id == "cursor")
        .unwrap();
    let cursor_kinds = cursor
        .channels
        .iter()
        .map(|channel| channel.kind.as_str())
        .collect::<Vec<_>>();
    assert_eq!(
        cursor_kinds,
        vec!["homebrew", "npm", "winget", "official-artifact"]
    );
    assert_eq!(cursor.binary_names, vec!["cursor-agent"]);
    let deepseek = registry
        .agents
        .iter()
        .find(|agent| agent.id == "deepseek-harness")
        .unwrap();
    assert_eq!(deepseek.binary_names, vec!["dsh"]);
    let openclaw = registry
        .agents
        .iter()
        .find(|agent| agent.id == "openclaw")
        .unwrap();
    assert!(openclaw.connection_modes.contains(&"local".to_string()));
    assert!(
        openclaw
            .connection_modes
            .contains(&"virtual-machine".to_string())
    );
    let hermes = registry
        .agents
        .iter()
        .find(|agent| agent.id == "hermes")
        .unwrap();
    assert!(
        hermes
            .connection_modes
            .contains(&"virtual-machine".to_string())
    );
}

#[test]
fn warehouse_is_one_manifest_and_one_toml_per_agent() {
    let loaded = manifest().unwrap();
    assert_eq!(
        loaded.schema_version,
        crate::domain::agent_hub::SCHEMA_VERSION
    );
    assert_eq!(
        loaded
            .agents
            .iter()
            .map(|agent| agent.file.as_str())
            .collect::<Vec<_>>(),
        vec![
            "codex.toml",
            "cursor.toml",
            "opencode.toml",
            "claude-code.toml",
            "pi.toml",
            "openclaw.toml",
            "hermes.toml",
            "antigravity.toml",
            "deepseek-harness.toml",
        ]
    );
    parse_manifest(include_str!(
        "../../../../resources/agent-hub/manifest.toml"
    ))
    .unwrap();
    for agent in &loaded.agents {
        let raw = match agent.id.as_str() {
            "codex" => include_str!("../../../../resources/agent-hub/codex.toml"),
            "cursor" => include_str!("../../../../resources/agent-hub/cursor.toml"),
            "opencode" => include_str!("../../../../resources/agent-hub/opencode.toml"),
            "claude-code" => include_str!("../../../../resources/agent-hub/claude-code.toml"),
            "pi" => include_str!("../../../../resources/agent-hub/pi.toml"),
            "openclaw" => include_str!("../../../../resources/agent-hub/openclaw.toml"),
            "hermes" => include_str!("../../../../resources/agent-hub/hermes.toml"),
            "antigravity" => include_str!("../../../../resources/agent-hub/antigravity.toml"),
            "deepseek-harness" => {
                include_str!("../../../../resources/agent-hub/deepseek-harness.toml")
            }
            other => panic!("unexpected agent {other}"),
        };
        let document = parse_agent_toml(raw).unwrap();
        assert_eq!(document.id, agent.id);
        assert!(!document.channels.is_empty());
    }
}

#[test]
fn recipes_are_argv_only_official_https_and_never_pipe_to_shell() {
    let sources = [
        include_str!("../../../../resources/agent-hub/codex.toml"),
        include_str!("../../../../resources/agent-hub/cursor.toml"),
        include_str!("../../../../resources/agent-hub/opencode.toml"),
        include_str!("../../../../resources/agent-hub/claude-code.toml"),
        include_str!("../../../../resources/agent-hub/pi.toml"),
        include_str!("../../../../resources/agent-hub/openclaw.toml"),
        include_str!("../../../../resources/agent-hub/hermes.toml"),
        include_str!("../../../../resources/agent-hub/antigravity.toml"),
        include_str!("../../../../resources/agent-hub/deepseek-harness.toml"),
    ];
    for raw in sources {
        assert!(!raw.contains("curl|"));
        assert!(!raw.contains("| sh"));
        assert!(!raw.contains("| bash"));
        assert!(!raw.contains("| iex"));
    }
    let registry = registry().unwrap();
    for agent in &registry.agents {
        for channel in &agent.channels {
            assert!(channel.official_source.starts_with("https://"));
            for argv in [
                &channel.install_argv,
                &channel.windows_install_argv,
                &channel.update_argv,
                &channel.uninstall_argv,
                &channel.verify_argv,
            ] {
                argv::validate(argv, ArgvKind::for_channel(&channel.kind)).unwrap();
                let joined = argv.join(" ");
                assert!(!joined.contains('|'));
                assert!(!joined.contains(" -c "));
            }
        }
    }
}

#[test]
fn each_desktop_os_selects_one_stable_channel_from_capability_snapshot() {
    let registry = registry().unwrap();
    let cases = [
        (
            "macos",
            "aarch64",
            &["homebrew", "npm"][..],
            "codex",
            "homebrew",
        ),
        ("macos", "aarch64", &["npm"][..], "codex", "npm"),
        (
            "windows",
            "x86_64",
            &["winget", "npm"][..],
            "codex",
            "winget",
        ),
        ("linux", "x86_64", &["npm"][..], "codex", "npm"),
        ("macos", "aarch64", &[][..], "codex", "official-artifact"),
        ("macos", "aarch64", &["homebrew"][..], "cursor", "homebrew"),
        ("linux", "aarch64", &[][..], "cursor", "official-artifact"),
        (
            "macos",
            "aarch64",
            &["homebrew", "npm"][..],
            "opencode",
            "homebrew",
        ),
        (
            "macos",
            "aarch64",
            &["homebrew", "npm"][..],
            "claude-code",
            "homebrew",
        ),
        (
            "windows",
            "x86_64",
            &["winget"][..],
            "claude-code",
            "winget",
        ),
        ("linux", "x86_64", &["npm"][..], "pi", "npm"),
        ("macos", "aarch64", &["homebrew", "npm"][..], "pi", "npm"),
        ("macos", "aarch64", &["npm"][..], "openclaw", "npm"),
        ("linux", "aarch64", &[][..], "hermes", "official-artifact"),
        (
            "macos",
            "aarch64",
            &["homebrew"][..],
            "antigravity",
            "homebrew",
        ),
        (
            "linux",
            "x86_64",
            &[][..],
            "antigravity",
            "official-artifact",
        ),
        (
            "macos",
            "aarch64",
            &["homebrew", "npm"][..],
            "deepseek-harness",
            "npm",
        ),
        ("linux", "x86_64", &["npm"][..], "deepseek-harness", "npm"),
    ];
    for (os, arch, managers, agent_id, expected) in cases {
        let agent = registry
            .agents
            .iter()
            .find(|item| item.id == agent_id)
            .unwrap();
        let selected = selector::select_channel(
            agent,
            &crate::domain::agent_hub::contract::PlatformInstallCapabilities {
                os: os.to_string(),
                architecture: arch.to_string(),
                managers: managers.iter().map(|item| (*item).to_string()).collect(),
                scan_generation: 1,
            },
        )
        .unwrap();
        assert_eq!(
            selected.channel.id, expected,
            "{agent_id} on {os} with {managers:?}"
        );
    }
}

#[test]
fn cursor_npm_and_winget_are_data_but_never_selected() {
    let registry = registry().unwrap();
    let cursor = registry
        .agents
        .iter()
        .find(|agent| agent.id == "cursor")
        .unwrap();
    let npm = cursor
        .channels
        .iter()
        .find(|channel| channel.kind == "npm")
        .unwrap();
    let winget = cursor
        .channels
        .iter()
        .find(|channel| channel.kind == "winget")
        .unwrap();
    assert!(!npm.selectable);
    assert!(!winget.selectable);
    let selected = selector::select_channel(
        cursor,
        &crate::domain::agent_hub::contract::PlatformInstallCapabilities {
            os: "windows".to_string(),
            architecture: "x86_64".to_string(),
            managers: vec!["npm".to_string(), "winget".to_string()],
            scan_generation: 1,
        },
    )
    .unwrap();
    assert_eq!(selected.channel.kind, "official-artifact");
}

#[test]
fn hermes_windows_is_an_unsupported_combination() {
    let registry = registry().unwrap();
    let hermes = registry
        .agents
        .iter()
        .find(|agent| agent.id == "hermes")
        .unwrap();
    let error = selector::select_channel(
        hermes,
        &crate::domain::agent_hub::contract::PlatformInstallCapabilities {
            os: "windows".to_string(),
            architecture: "x86_64".to_string(),
            managers: vec!["npm".to_string()],
            scan_generation: 1,
        },
    )
    .unwrap_err();
    assert!(error.to_string().contains("unsupported_platform"));
}

#[test]
fn contract_surface_keeps_plugin_management_out_of_hub() {
    let surface = contract_surface();
    assert_eq!(surface["pluginManagementBoundary"], "adapter-plugins-only");
    assert_eq!(surface["hostScope"], "desktop");
    assert!(surface.get("firstBatchIds").is_none());
}

fn capabilities(
    os: &str,
    architecture: &str,
    managers: &[&str],
) -> crate::domain::agent_hub::contract::PlatformInstallCapabilities {
    crate::domain::agent_hub::contract::PlatformInstallCapabilities {
        os: os.to_string(),
        architecture: architecture.to_string(),
        managers: managers.iter().map(|item| (*item).to_string()).collect(),
        scan_generation: 1,
    }
}

fn synthetic_channel(
    id: &str,
    kind: &str,
    manager: &str,
) -> crate::domain::agent_hub::contract::InstallChannel {
    crate::domain::agent_hub::contract::InstallChannel {
        id: id.to_string(),
        kind: kind.to_string(),
        oses: vec![
            "macos".to_string(),
            "linux".to_string(),
            "windows".to_string(),
        ],
        architectures: Vec::new(),
        priority: 10,
        official_recommended: true,
        licoup_verified: true,
        requires_manager: manager.to_string(),
        elevation: "none".to_string(),
        scope: "user".to_string(),
        selectable: true,
        unsupported_reason: None,
        package_coordinate: id.to_string(),
        package_form: None,
        official_source: "https://example.invalid/agent".to_string(),
        version_policy: "latest-stable".to_string(),
        artifact: None,
        install: None,
        install_argv: Vec::new(),
        windows_install_argv: Vec::new(),
        update_argv: Vec::new(),
        uninstall_argv: Vec::new(),
        verify_argv: Vec::new(),
    }
}

fn synthetic_recipe(
    id: &str,
    channels: Vec<crate::domain::agent_hub::contract::InstallChannel>,
) -> crate::domain::agent_hub::contract::AgentRecipe {
    crate::domain::agent_hub::contract::AgentRecipe {
        id: id.to_string(),
        label: id.to_string(),
        adaptation: crate::domain::agent_hub::contract::ADAPTATION_DEEP.to_string(),
        binary_names: vec![id.to_string()],
        protocol: "synthetic".to_string(),
        license: "MIT".to_string(),
        summary: "synthetic recipe".to_string(),
        homepage: "https://example.invalid/agent".to_string(),
        requires_login: false,
        connection_modes: vec!["local".to_string()],
        official_docs: "https://example.invalid/docs".to_string(),
        channels,
        unsupported: Vec::new(),
    }
}

#[test]
fn the_hub_presents_a_vendor_binary_before_a_toolchain_channel() {
    let registry = registry().unwrap();
    let codex = registry
        .agents
        .iter()
        .find(|agent| agent.id == "codex")
        .unwrap();
    let offered = selector::available_channels(
        codex,
        &capabilities("macos", "aarch64", &["homebrew", "npm"]),
    );
    let kinds = offered
        .iter()
        .map(|channel| channel.kind.as_str())
        .collect::<Vec<_>>();
    assert_eq!(
        kinds.first().copied(),
        Some("official-artifact"),
        "{kinds:?}"
    );
    let classes = offered
        .iter()
        .map(|channel| crate::domain::agent_hub::contract::channel_class(channel))
        .collect::<Vec<_>>();
    let mut sorted = classes.clone();
    sorted.sort_by_key(|class| match *class {
        "binary" => 0,
        "package-manager" => 1,
        _ => 2,
    });
    assert_eq!(classes, sorted, "binary channels come first: {classes:?}");
}

#[test]
fn a_host_without_node_offers_no_npm_channel_and_no_failing_recipe() {
    let registry = registry().unwrap();
    let snapshot = capabilities("macos", "aarch64", &["homebrew"]);
    for agent in &registry.agents {
        let offered = selector::available_channels(agent, &snapshot);
        assert!(
            offered.iter().all(|channel| channel.kind != "npm"),
            "{} still offers npm without node",
            agent.id
        );
        if offered.is_empty() {
            // A recipe with nothing installable on this host is marked, not
            // presented as an offer that would fail when confirmed.
            assert!(
                selector::toolchain_only(agent),
                "{} has no channel but is not marked",
                agent.id
            );
            assert!(!selector::first_launch_eligible(agent, &snapshot));
            continue;
        }
        let selected = selector::select_channel(agent, &snapshot).unwrap();
        assert!(
            !crate::domain::agent_hub::contract::channel_requires_developer_toolchain(
                selected.channel
            ),
            "{} selected a toolchain channel while none was offered",
            agent.id
        );
    }
}

#[test]
fn a_toolchain_only_recipe_is_marked_and_never_recommended() {
    let npm_only = synthetic_recipe(
        "synthetic-npm",
        vec![synthetic_channel("npm", "npm", "npm")],
    );
    let without_node = capabilities("linux", "x86_64", &[]);
    assert!(selector::available_channels(&npm_only, &without_node).is_empty());
    assert!(selector::toolchain_only(&npm_only));
    assert!(!selector::first_launch_eligible(&npm_only, &without_node));
    assert!(
        selector::select_channel(&npm_only, &without_node)
            .unwrap_err()
            .to_string()
            .contains("channel_unavailable")
    );

    // Installing the runtime does not turn a runtime-only recipe into an
    // ordinary recommendation: the recipe itself still needs a toolchain.
    let with_node = capabilities("linux", "x86_64", &["npm"]);
    assert!(selector::toolchain_only(&npm_only));
    assert!(!selector::first_launch_eligible(&npm_only, &with_node));
    assert!(selector::select_channel(&npm_only, &with_node).is_ok());
}

#[test]
fn a_vendor_binary_installs_without_a_developer_toolchain() {
    let binary_only = synthetic_recipe(
        "synthetic-binary",
        vec![synthetic_channel(
            "official-artifact",
            "official-artifact",
            "none",
        )],
    );
    let bare_host = capabilities("linux", "x86_64", &[]);
    let selected = selector::select_channel(&binary_only, &bare_host).unwrap();
    assert_eq!(selected.channel.kind, "official-artifact");
    assert!(
        !crate::domain::agent_hub::contract::channel_requires_developer_toolchain(selected.channel)
    );
    assert!(!selector::toolchain_only(&binary_only));
    assert!(selector::first_launch_eligible(&binary_only, &bare_host));
}

#[test]
fn the_contract_surface_declares_the_channel_classes() {
    let surface = contract_surface();
    assert_eq!(
        surface["channelClasses"],
        serde_json::json!(["binary", "package-manager", "vendor-script"])
    );
}

/// One artifact declaration names exactly one published digest, on the
/// artifact's own origin. Acquisition fails closed without that declaration, so
/// a malformed one must never load.
#[test]
fn a_published_digest_declaration_is_validated() {
    use super::support::fixture_artifact_channel;
    use crate::domain::agent_hub::contract::ArtifactIntegrity;
    use crate::domain::agent_hub::recipes::validate_agent;

    let recipe_with = |integrity: Option<ArtifactIntegrity>| {
        synthetic_recipe(
            "synthetic-artifact",
            vec![fixture_artifact_channel(
                "https://vendor.invalid",
                integrity,
            )],
        )
    };
    let published = |digest: Option<String>, template: Option<&str>| ArtifactIntegrity {
        algorithm: "sha256".to_string(),
        digest,
        digest_url_template: template.map(str::to_string),
    };

    validate_agent(&recipe_with(Some(published(Some("a".repeat(64)), None)))).unwrap();
    validate_agent(&recipe_with(Some(published(
        None,
        Some("https://vendor.invalid/agent.tar.gz.sha256"),
    ))))
    .unwrap();

    for (label, integrity) in [
        ("no published digest at all", published(None, None)),
        (
            "two digest sources",
            published(
                Some("a".repeat(64)),
                Some("https://vendor.invalid/a.sha256"),
            ),
        ),
        (
            "a digest that is not sha256",
            published(Some("a".repeat(63)), None),
        ),
        (
            "a digest document on another origin",
            published(None, Some("https://downloads.other.invalid/a.sha256")),
        ),
    ] {
        assert!(
            validate_agent(&recipe_with(Some(integrity))).is_err(),
            "an artifact declaration with {label} must not load"
        );
    }

    let unsupported = recipe_with(Some(ArtifactIntegrity {
        algorithm: "md5".to_string(),
        digest: Some("b".repeat(32)),
        digest_url_template: None,
    }));
    assert!(validate_agent(&unsupported).is_err());
}

/// A redirect declaration is the one thing that reaches past the artifact's own
/// origin, so an inexact or over-broad one must never load: acquisition can only
/// pin what it can compare exactly, and a host the origin rule already admits
/// would be a widened origin wearing a redirect's name.
#[test]
fn a_redirect_host_declaration_is_validated() {
    use super::support::fixture_artifact_channel;
    use crate::domain::agent_hub::recipes::validate_agent;

    let recipe_with = |hosts: Vec<&str>| {
        let mut channel = fixture_artifact_channel("https://vendor.invalid", None);
        channel.artifact.as_mut().unwrap().redirect_hosts =
            hosts.into_iter().map(str::to_string).collect();
        synthetic_recipe("synthetic-artifact", vec![channel])
    };

    validate_agent(&recipe_with(Vec::new())).unwrap();
    validate_agent(&recipe_with(vec!["release-assets.githubusercontent.com"])).unwrap();

    for (label, hosts) in [
        ("an empty host", vec![""]),
        ("a padded host", vec![" assets.vendor.invalid"]),
        ("a scheme", vec!["https://assets.vendor.invalid"]),
        ("a path", vec!["assets.vendor.invalid/agent.tar.gz"]),
        ("a port", vec!["assets.vendor.invalid:443"]),
        ("uppercase", vec!["Assets.Vendor.Invalid"]),
        ("a bare label", vec!["assets"]),
        ("a leading dot", vec![".vendor.invalid"]),
        ("a trailing dot", vec!["assets.vendor.invalid."]),
        ("an empty label", vec!["assets..vendor.invalid"]),
        ("a hyphen-edged label", vec!["-assets.vendor.invalid"]),
        ("the declared origin itself", vec!["vendor.invalid"]),
        (
            "a subdomain of the declared origin",
            vec!["assets.vendor.invalid"],
        ),
        (
            "a duplicate",
            vec!["assets.vendor.invalid", "assets.vendor.invalid"],
        ),
    ] {
        assert!(
            validate_agent(&recipe_with(hosts)).is_err(),
            "a redirect declaration with {label} must not load"
        );
    }
}

/// The bundled artifact declarations carry the vendor publications that were
/// verified against the vendors' own endpoints, and nothing else.
///
/// A channel stays refused while its vendor publishes no digest for the artifact
/// the recipe stages: acquisition fails closed, so an undeclared digest is a
/// refusal, never an unverified install.
#[test]
fn artifact_declarations_carry_only_verified_vendor_publication() {
    use super::support::bare_host_capabilities;
    use crate::domain::agent_hub::acquisition;

    let registry = registry().unwrap();
    let artifact = |agent_id: &str| {
        let agent = registry
            .agents
            .iter()
            .find(|agent| agent.id == agent_id)
            .unwrap();
        let channel = agent
            .channels
            .iter()
            .find(|channel| channel.kind == "official-artifact")
            .unwrap();
        channel.artifact.as_ref().unwrap()
    };

    let codex = artifact("codex");
    assert_eq!(codex.origin_host, "github.com");
    assert_eq!(
        codex.url_template,
        "https://github.com/openai/codex/releases/latest/download/codex-package-{vendorArch}-{vendorOs}.tar.gz"
    );
    assert_eq!(
        codex.redirect_hosts,
        vec!["release-assets.githubusercontent.com"]
    );
    let integrity = codex.integrity.as_ref().expect("codex publishes a digest");
    assert_eq!(integrity.algorithm, "sha256");
    assert!(integrity.digest.is_none());
    assert_eq!(
        integrity.digest_url_template.as_deref(),
        Some("https://github.com/openai/codex/releases/latest/download/codex-package_SHA256SUMS")
    );
    let macos = bare_host_capabilities("macos", "aarch64");
    assert_eq!(
        acquisition::artifact_url(codex, &macos, "latest").unwrap(),
        "https://github.com/openai/codex/releases/latest/download/codex-package-aarch64-apple-darwin.tar.gz"
    );
    assert_eq!(
        acquisition::digest_document_url(codex, &macos, "latest")
            .unwrap()
            .as_deref(),
        Some("https://github.com/openai/codex/releases/latest/download/codex-package_SHA256SUMS")
    );

    // opencode names the vendor's own asset extensions, and stays refused: the
    // release publishes no digest for any of them.
    let opencode = artifact("opencode");
    assert_eq!(
        opencode.redirect_hosts,
        vec!["release-assets.githubusercontent.com"]
    );
    assert!(opencode.integrity.is_none());
    assert_eq!(
        acquisition::artifact_url(opencode, &macos, "latest").unwrap(),
        "https://github.com/anomalyco/opencode/releases/latest/download/opencode-darwin-arm64.zip"
    );
    assert_eq!(
        acquisition::artifact_url(
            opencode,
            &bare_host_capabilities("linux", "x86_64"),
            "latest"
        )
        .unwrap(),
        "https://github.com/anomalyco/opencode/releases/latest/download/opencode-linux-x64.tar.gz"
    );

    // No digest was observed for these vendors' staged artifact, so no channel
    // declares one and each stays refused with `artifact_integrity_undeclared`.
    for agent_id in ["cursor", "claude-code", "antigravity", "hermes", "openclaw"] {
        assert!(
            artifact(agent_id).integrity.is_none(),
            "{agent_id} must stay refused until its vendor publishes a digest for the staged artifact"
        );
    }
}

/// A declared install destination is where the Hub writes and what it removes,
/// so an inexact one must never load: a destination without a root the discovery
/// owner knows would install an Agent somewhere no scan looks.
#[test]
fn an_install_destination_declaration_is_validated() {
    use super::support::fixture_artifact_channel;
    use crate::domain::agent_hub::contract::InstallPlacement;
    use crate::domain::agent_hub::recipes::validate_agent;

    let placement =
        |dir: &[(&str, &str)], binary: &[(&str, &str)], argv: &[&str]| InstallPlacement {
            binary: binary
                .iter()
                .map(|(os, name)| (os.to_string(), name.to_string()))
                .collect(),
            dir: dir
                .iter()
                .map(|(os, template)| (os.to_string(), template.to_string()))
                .collect(),
            argv: argv.iter().map(|arg| arg.to_string()).collect(),
        };
    let recipe_with = |install: InstallPlacement, verify: &str| {
        let mut channel = fixture_artifact_channel("https://vendor.invalid", None);
        channel.install = Some(install);
        channel.verify_argv = vec![verify.to_string(), "--version".to_string()];
        synthetic_recipe("synthetic-artifact", vec![channel])
    };
    let valid = || {
        placement(
            &[("macos", "{home}/.local/bin")],
            &[("macos", "synthetic-agent")],
            &["install", "-m", "0755", "{staging}/agent", "{install}"],
        )
    };

    validate_agent(&recipe_with(valid(), "{install}")).unwrap();
    // A vendor installer that places the result needs no placement step.
    validate_agent(&recipe_with(
        placement(
            &[("macos", "{home}/.local/bin")],
            &[("macos", "synthetic-agent")],
            &[],
        ),
        "{install}",
    ))
    .unwrap();

    for (label, install, verify) in [
        (
            "a directory without its binary name",
            placement(
                &[("macos", "{home}/.local/bin")],
                &[],
                &["install", "{staging}/a", "{install}"],
            ),
            "{install}",
        ),
        (
            "a binary without its directory",
            placement(
                &[],
                &[("macos", "synthetic-agent")],
                &["install", "{staging}/a", "{install}"],
            ),
            "{install}",
        ),
        (
            "a destination on another OS than the channel lists",
            placement(
                &[("windows", "{home}/.local/bin")],
                &[("windows", "a.exe")],
                &["install", "{staging}/a", "{install}"],
            ),
            "{install}",
        ),
        (
            "a template with no host root",
            placement(
                &[("macos", "/usr/local/bin")],
                &[("macos", "synthetic-agent")],
                &["install", "{staging}/a", "{install}"],
            ),
            "{install}",
        ),
        (
            "a template with no root token",
            placement(
                &[("macos", "~/.local/bin")],
                &[("macos", "synthetic-agent")],
                &["install", "{staging}/a", "{install}"],
            ),
            "{install}",
        ),
        (
            "a template that climbs out of its root",
            placement(
                &[("macos", "{home}/../etc")],
                &[("macos", "synthetic-agent")],
                &["install", "{staging}/a", "{install}"],
            ),
            "{install}",
        ),
        (
            "a binary that is a path",
            placement(
                &[("macos", "{home}/.local/bin")],
                &[("macos", "bin/agent")],
                &["install", "{staging}/a", "{install}"],
            ),
            "{install}",
        ),
        (
            "a placement step with no declared target",
            placement(
                &[("macos", "{home}/.local/bin")],
                &[("macos", "synthetic-agent")],
                &["install", "-m", "0755", "{staging}/agent"],
            ),
            "{install}",
        ),
        (
            "a verification that names a PATH binary instead of the destination",
            placement(
                &[("macos", "{home}/.local/bin")],
                &[("macos", "synthetic-agent")],
                &["install", "-m", "0755", "{staging}/agent", "{install}"],
            ),
            "synthetic-agent",
        ),
    ] {
        assert!(
            validate_agent(&recipe_with(install, verify)).is_err(),
            "an install declaration with {label} must not load"
        );
    }
}

/// Every destination the bundled recipes declare is one the discovery owner
/// already admits, so what the Hub installs is what the next Agent scan finds.
#[test]
fn declared_install_destinations_are_locations_discovery_admits() {
    use licoup_agent_targets::domain::targets::scan_paths;

    let registry = registry().unwrap();
    let roots = scan_paths::HostRoots::from_environment();
    let mut declared = 0;
    for agent in &registry.agents {
        for channel in &agent.channels {
            let Some(install) = channel.install.as_ref() else {
                continue;
            };
            assert_eq!(
                channel.verify_argv.first().map(String::as_str),
                Some("{install}"),
                "{}/{} verifies its declared destination",
                agent.id,
                channel.id
            );
            for (os, template) in &install.dir {
                let binary = install
                    .binary
                    .get(os)
                    .unwrap_or_else(|| panic!("{}/{} names {os}", agent.id, channel.id));
                let directory = scan_paths::expand_path_template(template, &roots)
                    .unwrap_or_else(|| panic!("{}/{} template {template}", agent.id, channel.id));
                let path = directory.join(binary);
                assert!(
                    scan_paths::install_destination_admitted(&path, os, &roots),
                    "{}/{} must install where discovery looks: {}",
                    agent.id,
                    channel.id,
                    path.display()
                );
                declared += 1;
            }
        }
    }
    assert_eq!(declared, 10, "five channels declare two OSes each");
}
