use crate::domain::agent_hub::recipes::{agent_recipe, registry};
use crate::domain::agent_hub::version_check::{installed_version, parse_output};

#[test]
fn each_agent_has_a_dedicated_parser() {
    assert_eq!(parse_output("codex", "codex-cli 0.147.0\n", ""), "0.147.0");
    assert_eq!(
        parse_output("cursor", "cursor-agent 2026.8.9\n", ""),
        "2026.8.9"
    );
    assert_eq!(
        parse_output("cursor", "cursor-agent 2026.08.25-3e8eec8\n", ""),
        "2026.08.25-3e8eec8"
    );
    assert_eq!(parse_output("opencode", "OpenCode 1.2.3\n", ""), "1.2.3");
    assert_eq!(
        parse_output("claude-code", "1.0.40 (Claude Code)\n", ""),
        "1.0.40"
    );
    assert_eq!(parse_output("pi", "pi 0.5.1\n", ""), "0.5.1");
    assert_eq!(parse_output("openclaw", "openclaw 0.3.0\n", ""), "0.3.0");
    assert_eq!(parse_output("hermes", "hermes 0.1.2\n", ""), "0.1.2");
    assert_eq!(parse_output("antigravity", "agy 0.9.0\n", ""), "0.9.0");
    assert_eq!(parse_output("deepseek-harness", "dsh 0.1.0\n", ""), "0.1.0");
}

#[test]
fn unknown_tokens_and_empty_probes_stay_blank() {
    assert_eq!(parse_output("codex", "", ""), "");
    assert_eq!(parse_output("codex", "unknown\n", ""), "");
    assert_eq!(parse_output("codex", "未知\n", ""), "");
    assert_eq!(parse_output("cursor", "latest\n", ""), "");
    assert_eq!(parse_output("not-an-agent", "1.0.0\n", ""), "");
}

#[test]
fn stderr_is_used_when_stdout_is_empty() {
    assert_eq!(parse_output("claude-code", "", "claude 1.2.3\n"), "1.2.3");
}

#[test]
fn unsafe_and_malformed_native_versions_stay_blank() {
    assert_eq!(
        parse_output("cursor", "cursor-agent not-a-version\n", ""),
        ""
    );
    assert_eq!(
        parse_output("cursor", "cursor-agent release-next\n", ""),
        ""
    );
    for malformed in [
        "cursor-agent 2026.08.32-3e8eec8\n",
        "cursor-agent 2026.08.25-3e8eec\n",
        "cursor-agent 2026.08.25-3e8eec8/path\n",
        "cursor-agent 2026.08.25-3e8eec8 extra\n",
    ] {
        assert_eq!(parse_output("cursor", malformed, ""), "");
    }
    assert_eq!(
        parse_output("antigravity", "agy 3.7.0 extra/path\n", ""),
        ""
    );
}

#[cfg(unix)]
#[test]
fn installed_version_executes_only_the_exact_bound_target_binary() {
    use std::fs;
    use std::os::unix::fs::PermissionsExt;

    let root = std::env::temp_dir().join(format!("licoup-bound-version-{}", uuid::Uuid::new_v4()));
    fs::create_dir_all(&root).unwrap();
    let executable = root.join("cursor-agent");
    fs::write(
        &executable,
        "#!/bin/sh\nprintf 'cursor-agent 2026.8.9\\n'\n",
    )
    .unwrap();
    fs::set_permissions(&executable, fs::Permissions::from_mode(0o700)).unwrap();

    let registry = registry().unwrap();
    let agent = agent_recipe(registry, "cursor").unwrap();
    let channel = agent
        .channels
        .iter()
        .find(|channel| channel.id == "homebrew")
        .unwrap();
    let version = installed_version(
        agent,
        Some(channel),
        true,
        true,
        &serde_json::json!({}),
        Some(&executable),
    );
    assert_eq!(version, "2026.8.9");

    let wrong_binding = root.join("unrelated-agent");
    fs::rename(&executable, &wrong_binding).unwrap();
    assert_eq!(
        installed_version(
            agent,
            Some(channel),
            true,
            true,
            &serde_json::json!({}),
            Some(&wrong_binding),
        ),
        ""
    );
    fs::remove_dir_all(root).unwrap();
}

/// A channel that verifies `{install}` verifies the executable at its declared
/// destination. A channel that names a destination it never declared stays
/// blank instead of probing a path the recipe never stated.
#[cfg(unix)]
#[test]
fn an_install_verification_needs_a_declared_destination() {
    use std::fs;
    use std::os::unix::fs::PermissionsExt;

    let root =
        std::env::temp_dir().join(format!("licoup-install-version-{}", uuid::Uuid::new_v4()));
    fs::create_dir_all(&root).unwrap();
    let executable = root.join("codex");
    fs::write(&executable, "#!/bin/sh\nprintf 'codex-cli 0.147.0\\n'\n").unwrap();
    fs::set_permissions(&executable, fs::Permissions::from_mode(0o700)).unwrap();

    let registry = registry().unwrap();
    let agent = agent_recipe(registry, "codex").unwrap();
    let channel = agent
        .channels
        .iter()
        .find(|channel| channel.id == "official-artifact")
        .unwrap();
    assert_eq!(
        channel.verify_argv.first().map(String::as_str),
        Some("{install}")
    );
    assert!(channel.install.is_some());
    assert_eq!(
        installed_version(
            agent,
            Some(channel),
            true,
            true,
            &serde_json::json!({}),
            Some(&executable),
        ),
        "0.147.0"
    );

    let mut undeclared = channel.clone();
    undeclared.install = None;
    assert_eq!(
        installed_version(
            agent,
            Some(&undeclared),
            true,
            true,
            &serde_json::json!({}),
            Some(&executable),
        ),
        ""
    );
    fs::remove_dir_all(root).unwrap();
}
