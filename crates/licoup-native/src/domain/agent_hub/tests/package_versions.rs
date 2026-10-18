use crate::domain::agent_hub::contract::InstallChannel;
use crate::domain::agent_hub::package_versions::{ChannelVersions, lookup_local};
use std::fs;
use std::time::{SystemTime, UNIX_EPOCH};

fn temp_root(name: &str) -> std::path::PathBuf {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default();
    let dir = std::env::temp_dir().join(format!(
        "lico-agent-hub-pkg-{}-{}-{}",
        name,
        now.as_secs(),
        now.subsec_nanos()
    ));
    fs::create_dir_all(&dir).unwrap();
    dir
}

fn cask_channel() -> InstallChannel {
    InstallChannel {
        id: "homebrew".to_string(),
        kind: "homebrew".to_string(),
        oses: vec!["macos".to_string()],
        architectures: vec!["aarch64".to_string()],
        priority: 10,
        official_recommended: true,
        licoup_verified: true,
        requires_manager: "homebrew".to_string(),
        elevation: "none".to_string(),
        scope: "user".to_string(),
        selectable: true,
        unsupported_reason: None,
        package_coordinate: "codex".to_string(),
        package_form: Some("cask".to_string()),
        official_source: "https://example.invalid".to_string(),
        version_policy: "latest-stable".to_string(),
        artifact: None,
        install: None,
        install_argv: vec![],
        windows_install_argv: vec![],
        update_argv: vec![],
        uninstall_argv: vec![],
        verify_argv: vec![],
    }
}

#[test]
fn homebrew_cask_reads_installed_and_formula_latest_from_local_roots() {
    let root = temp_root("cask");
    fs::create_dir_all(root.join("Caskroom").join("codex").join("0.42.1")).unwrap();
    let recipe = root
        .join("Library")
        .join("Taps")
        .join("homebrew")
        .join("homebrew-cask")
        .join("Casks")
        .join("c");
    fs::create_dir_all(&recipe).unwrap();
    fs::write(
        recipe.join("codex.rb"),
        "cask \"codex\" do\n  version \"0.43.0\"\nend\n",
    )
    .unwrap();
    let versions = lookup_local(&cask_channel(), &[root]);
    assert_eq!(
        versions,
        ChannelVersions {
            installed: "0.42.1".to_string(),
            latest: "0.43.0".to_string(),
        }
    );
}

#[test]
fn npm_reads_installed_package_json_and_leaves_latest_unknown() {
    let root = temp_root("npm");
    let pkg = root
        .join("lib")
        .join("node_modules")
        .join("@openai")
        .join("codex");
    fs::create_dir_all(&pkg).unwrap();
    fs::write(
        pkg.join("package.json"),
        r#"{"name":"@openai/codex","version":"1.2.3"}"#,
    )
    .unwrap();
    let mut channel = cask_channel();
    channel.kind = "npm".to_string();
    channel.package_coordinate = "@openai/codex".to_string();
    channel.package_form = None;
    let versions = lookup_local(&channel, &[root]);
    assert_eq!(versions.installed, "1.2.3");
    assert_eq!(versions.latest, "");
}
