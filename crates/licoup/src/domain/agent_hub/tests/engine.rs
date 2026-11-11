use super::super::*;
use super::support::{
    FixtureReply, FixtureRoute, digest_document, fixture_artifact_channel, portable_params, serve,
    sha256_hex, synthetic_agent, synthetic_registry, temp_dir, test_store,
};
use crate::domain::agent_hub::acquisition::{RecordingArtifactFetcher, VendorArtifactFetcher};
use crate::domain::agent_hub::argv::RecordingArgvRunner;
use crate::domain::agent_hub::contract::ArtifactIntegrity;
use crate::domain::agent_hub::engine::{HubContext, apply_with, plan_with};
use serde_json::{Value, json};
use std::sync::Arc;

const ARCHIVE_NAME: &str = "agent-darwin-arm64.tar.gz";

fn macos_params(name: &str) -> Value {
    portable_params(name).1
}

/// Parameters for a macOS host with no package manager and no toolchain.
fn bare_params(state_root: &std::path::Path) -> Value {
    json!({
        "agentId": "synthetic",
        "stateRoot": state_root.to_string_lossy(),
        "platformCapabilities": {
            "os": "macos",
            "architecture": "aarch64",
            "managers": [],
            "scanGeneration": 5
        },
        "discoveryCandidates": []
    })
}

#[test]
fn plan_returns_selected_channel_and_apply_requires_confirmation_token() {
    let params = macos_params("plan-apply");
    let mut install = params.clone();
    install["agentId"] = json!("codex");
    let ctx = HubContext::with_runner(&install, Arc::new(RecordingArgvRunner::new())).unwrap();
    let planned = plan_with(&ctx, &install).unwrap();
    assert_eq!(planned["status"], "planned");
    assert_eq!(planned["selectedChannel"]["id"], "homebrew");
    assert_eq!(planned["selectedChannel"]["kind"], "homebrew");
    assert_eq!(
        planned["selectedChannel"]["argv"],
        json!(["brew", "install", "--cask", "codex"])
    );
    assert!(apply_with(&ctx, &install).is_err());

    let mut confirmed = install;
    confirmed["confirmation"] = planned["confirmation"].clone();
    let applied = apply_with(&ctx, &confirmed).unwrap();
    assert_eq!(applied["ok"], true);
    assert_eq!(applied["ownership"], "owned");
    assert_eq!(applied["status"], "needs-login");
    assert_eq!(applied["channelId"], "homebrew");
}

#[test]
fn apply_is_argv_only_and_records_fixed_package_manager_arguments() {
    let runner = RecordingArgvRunner::new();
    let mut params = macos_params("argv-only");
    params["agentId"] = json!("pi");
    let ctx = HubContext::with_runner(&params, Arc::new(runner.clone())).unwrap();
    let planned = plan_with(&ctx, &params).unwrap();
    assert_eq!(planned["selectedChannel"]["id"], "npm");
    let mut confirmed = params;
    confirmed["confirmation"] = planned["confirmation"].clone();
    apply_with(&ctx, &confirmed).unwrap();
    let recorded = runner.recorded();
    assert_eq!(recorded[0].0, "npm");
    assert_eq!(
        recorded[0].1,
        vec![
            "install".to_string(),
            "-g".to_string(),
            "@mariozechner/pi-coding-agent".to_string()
        ]
    );
    assert!(
        recorded
            .iter()
            .all(|(_, args)| !args.join(" ").contains('|'))
    );
}

#[test]
fn external_discovery_is_not_taken_over_and_owned_lifecycle_stays_on_channel() {
    let mut params = macos_params("external");
    params["agentId"] = json!("opencode");
    params["discoveryCandidates"] = json!([{
        "target": "opencode",
        "status": "detected",
        "present": true,
        "location": "local"
    }]);
    let runner = RecordingArgvRunner::new();
    let ctx = HubContext::with_runner(&params, Arc::new(runner.clone())).unwrap();
    let planned = plan_with(&ctx, &params).unwrap();
    assert_eq!(planned["status"], "external_protected");
    assert_eq!(planned["ownership"], "external");
    assert_eq!(runner.recorded().len(), 0);

    let mut owned_params = macos_params("owned");
    owned_params["agentId"] = json!("opencode");
    let owned_ctx =
        HubContext::with_runner(&owned_params, Arc::new(RecordingArgvRunner::new())).unwrap();
    let install_plan = plan_with(&owned_ctx, &owned_params).unwrap();
    let mut confirmed = owned_params.clone();
    confirmed["confirmation"] = install_plan["confirmation"].clone();
    apply_with(&owned_ctx, &confirmed).unwrap();

    confirmed["operation"] = json!("update");
    let update_plan = plan_with(&owned_ctx, &confirmed).unwrap();
    assert_eq!(update_plan["selectedChannel"]["id"], "homebrew");
    assert_eq!(
        update_plan["selectedChannel"]["argv"],
        json!(["brew", "upgrade", "anomalyco/tap/opencode"])
    );
}

#[test]
fn cancel_before_runner_does_not_record_ownership() {
    let mut params = macos_params("cancel");
    params["agentId"] = json!("claude-code");
    let store = test_store("cancel");
    params["stateRoot"] = json!(store.root().to_string_lossy());
    let runner = RecordingArgvRunner::new();
    let ctx = HubContext::with_runner(&params, Arc::new(runner.clone())).unwrap();
    let planned = plan_with(&ctx, &params).unwrap();
    let mut cancelled = params;
    cancelled["confirmation"] = planned["confirmation"].clone();
    cancelled["cancel"] = json!(true);
    let result = apply_with(&ctx, &cancelled).unwrap();
    assert_eq!(result["status"], "cancelled");
    assert!(runner.recorded().is_empty());
}

#[test]
fn an_artifact_channel_without_a_published_digest_is_refused_before_confirmation() {
    // Cursor on a host without any package manager has exactly one offered
    // channel: the vendor artifact. Its recipe declares no published digest, so
    // no staged file can be verified and the plan must not issue a token.
    let mut params = macos_params("cursor-artifact");
    params["agentId"] = json!("cursor");
    params["platformCapabilities"] = json!({
        "os": "linux",
        "architecture": "x86_64",
        "managers": [],
        "scanGeneration": 3
    });
    let planned = plan(&params).unwrap();
    assert_eq!(planned["ok"], false);
    assert_eq!(planned["status"], "unavailable");
    assert_eq!(planned["code"], "artifact_integrity_undeclared");
    assert_eq!(planned["operation"], "install");
    assert_eq!(planned["channelId"], "official-artifact");
    assert_eq!(planned["channelKind"], "official-artifact");
    assert!(
        planned.get("confirmation").is_none(),
        "an unsatisfiable plan must not issue a confirmation token: {planned}"
    );
    // The failure names the missing producer instead of leaking a literal
    // placeholder into an install path.
    assert!(!planned.to_string().contains("{artifact}"));

    let mut confirmed = params;
    confirmed["confirmation"] = json!("agent-hub:install:cursor:official-artifact:whatever");
    let applied = apply(&confirmed).unwrap();
    assert_eq!(applied["ok"], false);
    assert_eq!(applied["code"], "artifact_integrity_undeclared");
}

#[test]
fn a_confirmed_binary_install_stages_verified_bytes_before_installing() {
    let body = b"synthetic vendor archive".to_vec();
    let server = serve(vec![
        FixtureRoute {
            path: format!("/{ARCHIVE_NAME}.sha256"),
            reply: FixtureReply::Body(digest_document(ARCHIVE_NAME, &body)),
        },
        FixtureRoute {
            path: format!("/{ARCHIVE_NAME}"),
            reply: FixtureReply::Body(String::from_utf8(body.clone()).unwrap()),
        },
    ]);
    let base = server.base();
    let state_root = temp_dir("binary-install");
    let channel = fixture_artifact_channel(
        &base,
        Some(ArtifactIntegrity {
            algorithm: "sha256".to_string(),
            digest: None,
            digest_url_template: Some(format!("{base}/{ARCHIVE_NAME}.sha256")),
        }),
    );
    let registry = synthetic_registry(vec![synthetic_agent("synthetic", vec![channel])]);
    let params = bare_params(&state_root);
    let runner = RecordingArgvRunner::new();
    let ctx = HubContext::with_ports(
        &params,
        Arc::new(runner.clone()),
        Arc::new(VendorArtifactFetcher),
        Arc::new(registry),
    )
    .unwrap();

    let planned = plan_with(&ctx, &params).unwrap();
    assert_eq!(planned["status"], "planned");
    assert_eq!(planned["selectedChannel"]["kind"], "official-artifact");
    assert_eq!(
        planned["acquisition"]["sourceUrl"],
        format!("{base}/{ARCHIVE_NAME}")
    );
    assert_eq!(planned["acquisition"]["integrity"], "published-digest");
    assert_eq!(planned["acquisition"]["role"], "archive");

    let mut confirmed = params;
    confirmed["confirmation"] = planned["confirmation"].clone();
    let applied = apply_with(&ctx, &confirmed).unwrap();
    assert_eq!(applied["ok"], true, "{applied}");
    assert_eq!(applied["status"], "available");
    assert_eq!(applied["ownership"], "owned");
    assert_eq!(applied["stagedArtifact"]["role"], "archive");
    assert_eq!(applied["stagedArtifact"]["sha256"], sha256_hex(&body));
    assert_eq!(applied["stagedArtifact"]["bytes"], body.len());
    assert_eq!(applied["stagedArtifact"]["resumed"], false);

    // The install argv runs with the staged paths, then the channel's own
    // verification argv runs.
    let recorded = runner.recorded();
    assert_eq!(recorded.len(), 2, "{recorded:?}");
    assert_eq!(recorded[0].0, "tar");
    assert_eq!(recorded[0].1[0], "-xzf");
    assert_eq!(recorded[1].0, "synthetic-agent");
    assert_eq!(recorded[1].1, vec!["--version".to_string()]);
    let staged_path = std::path::PathBuf::from(&recorded[0].1[1]);
    assert!(
        staged_path.ends_with(ARCHIVE_NAME),
        "{staged_path:?} is not the staged vendor artifact"
    );
    assert_eq!(std::fs::read(&staged_path).unwrap(), body);
    assert!(
        staged_path.starts_with(state_root.join("agent-hub/staging")),
        "acquisition staged outside its own root: {staged_path:?}"
    );
    assert_eq!(recorded[0].1[2], "-C");
    assert!(std::path::Path::new(&recorded[0].1[3]).is_dir());
    assert!(
        recorded
            .iter()
            .all(|(program, args)| !program.contains('{')
                && args.iter().all(|arg| !arg.contains('{'))),
        "no placeholder may reach the install argv: {recorded:?}"
    );
    server.finish();
}

#[test]
fn a_confirmed_vendor_script_install_runs_the_staged_script() {
    let body = b"synthetic vendor installer".to_vec();
    let script_name = "install.sh";
    let server = serve(vec![
        FixtureRoute {
            path: format!("/{script_name}.sha256"),
            reply: FixtureReply::Body(digest_document(script_name, &body)),
        },
        FixtureRoute {
            path: format!("/{script_name}"),
            reply: FixtureReply::Body(String::from_utf8(body.clone()).unwrap()),
        },
    ]);
    let base = server.base();
    let state_root = temp_dir("script-install");
    let mut channel = fixture_artifact_channel(
        &base,
        Some(ArtifactIntegrity {
            algorithm: "sha256".to_string(),
            digest: None,
            digest_url_template: Some(format!("{base}/{script_name}.sha256")),
        }),
    );
    let artifact = channel.artifact.as_mut().unwrap();
    artifact.url_template = format!("{base}/{{installer}}");
    artifact.installer = [("macos".to_string(), script_name.to_string())]
        .into_iter()
        .collect();
    channel.install_argv = vec!["bash".to_string(), "{script}".to_string()];
    channel.update_argv = Vec::new();
    channel.uninstall_argv = Vec::new();
    channel.verify_argv = Vec::new();
    let registry = synthetic_registry(vec![synthetic_agent("synthetic", vec![channel])]);
    let params = bare_params(&state_root);
    let runner = RecordingArgvRunner::new();
    let ctx = HubContext::with_ports(
        &params,
        Arc::new(runner.clone()),
        Arc::new(VendorArtifactFetcher),
        Arc::new(registry),
    )
    .unwrap();

    let planned = plan_with(&ctx, &params).unwrap();
    assert_eq!(planned["status"], "planned", "{planned}");
    assert_eq!(planned["acquisition"]["role"], "script");
    assert_eq!(
        planned["acquisition"]["sourceUrl"],
        format!("{base}/{script_name}")
    );

    let mut confirmed = params;
    confirmed["confirmation"] = planned["confirmation"].clone();
    let applied = apply_with(&ctx, &confirmed).unwrap();
    assert_eq!(applied["ok"], true, "{applied}");

    let recorded = runner.recorded();
    assert_eq!(recorded.len(), 1, "{recorded:?}");
    assert_eq!(recorded[0].0, "bash");
    let staged_path = std::path::PathBuf::from(&recorded[0].1[0]);
    assert!(staged_path.ends_with(script_name), "{staged_path:?}");
    assert_eq!(std::fs::read(&staged_path).unwrap(), body);
    server.finish();
}

#[test]
fn an_unresolved_install_reference_is_refused_instead_of_running_a_literal_path() {
    // A channel that installs by reference has no declared destination, so the
    // reference is the caller's to supply and never a defaulted placeholder.
    let state_root = temp_dir("install-ref");
    let mut channel = fixture_artifact_channel("https://vendor.invalid", None);
    channel.install_argv = vec!["rm".to_string(), "{install}".to_string()];
    let registry = synthetic_registry(vec![synthetic_agent("synthetic", vec![channel])]);
    let params = bare_params(&state_root);
    let runner = RecordingArgvRunner::new();
    let ctx = HubContext::with_ports(
        &params,
        Arc::new(runner.clone()),
        Arc::new(RecordingArtifactFetcher::new()),
        Arc::new(registry),
    )
    .unwrap();

    let planned = plan_with(&ctx, &params).unwrap();
    assert_eq!(planned["ok"], false);
    assert_eq!(planned["code"], "install_reference_unresolved");
    assert!(runner.recorded().is_empty());

    let mut supplied = params;
    supplied["installRef"] = json!("/tmp/synthetic-agent-reference");
    let planned = plan_with(&ctx, &supplied).unwrap();
    assert_eq!(planned["status"], "planned");
    let mut confirmed = supplied;
    confirmed["confirmation"] = planned["confirmation"].clone();
    let applied = apply_with(&ctx, &confirmed).unwrap();
    assert_eq!(applied["ok"], true);
    assert_eq!(
        runner.recorded().first().cloned(),
        Some((
            "rm".to_string(),
            vec!["/tmp/synthetic-agent-reference".to_string()]
        ))
    );
}

#[test]
fn a_confirmation_token_does_not_authorize_a_different_requested_version() {
    // A vendor-binary channel fetches the artifact its requested version names,
    // so a token confirmed for one version must not apply another.
    let mut params = macos_params("token-version");
    params["agentId"] = json!("codex");
    params["channelId"] = json!("npm");
    let ctx = HubContext::with_runner(&params, Arc::new(RecordingArgvRunner::new())).unwrap();
    let planned = plan_with(&ctx, &params).unwrap();
    assert_eq!(planned["status"], "planned");

    let mut changed = params;
    changed["version"] = json!("9.9.9");
    changed["confirmation"] = planned["confirmation"].clone();
    let error = apply_with(&ctx, &changed).unwrap_err().to_string();
    assert!(error.contains("confirmation_mismatch"), "{error}");
}

#[test]
fn plan_honors_requested_install_channel_instead_of_auto_select() {
    let mut params = macos_params("channel-pick");
    params["agentId"] = json!("codex");
    params["channelId"] = json!("npm");
    params["version"] = json!("latest");
    let planned = plan(&params).unwrap();
    assert_eq!(planned["status"], "planned");
    assert_eq!(planned["selectedChannel"]["id"], "npm");
    assert_eq!(planned["selectedChannel"]["kind"], "npm");
    assert_eq!(
        planned["selectedChannel"]["argv"],
        json!(["npm", "install", "-g", "@openai/codex"])
    );
}

/// The body the recording port stages, with the digest that pins it.
fn staged_body() -> Vec<u8> {
    b"synthetic vendor archive".to_vec()
}

fn pinned_integrity() -> ArtifactIntegrity {
    ArtifactIntegrity {
        algorithm: "sha256".to_string(),
        digest: Some(sha256_hex(&staged_body())),
        digest_url_template: None,
    }
}

/// A fixture channel that declares where its executable belongs and places the
/// staged result there itself.
fn placed_channel() -> (InstallChannel, RecordingArtifactFetcher) {
    let mut channel = fixture_artifact_channel("https://vendor.invalid", Some(pinned_integrity()));
    channel.install = Some(InstallPlacement {
        binary: [("macos".to_string(), "synthetic-agent".to_string())]
            .into_iter()
            .collect(),
        dir: [("macos".to_string(), "{home}/.local/bin".to_string())]
            .into_iter()
            .collect(),
        argv: vec![
            "install".to_string(),
            "-m".to_string(),
            "0755".to_string(),
            format!("{{staging}}/{ARCHIVE_NAME}"),
            "{install}".to_string(),
        ],
    });
    channel.verify_argv = vec!["{install}".to_string(), "--version".to_string()];
    let fetcher = RecordingArtifactFetcher::new();
    fetcher.serve(ARCHIVE_NAME, staged_body());
    (channel, fetcher)
}

/// The destination the discovery owner's own roots derive for the fixture.
fn fixture_destination() -> std::path::PathBuf {
    licoup_agent_targets::domain::targets::scan_paths::HostRoots::from_environment()
        .home
        .expect("host home")
        .join(".local")
        .join("bin")
        .join("synthetic-agent")
}

#[test]
fn a_declared_destination_derives_the_install_reference_and_places_the_result() {
    let state_root = temp_dir("placement");
    let (channel, fetcher) = placed_channel();
    let registry = synthetic_registry(vec![synthetic_agent("synthetic", vec![channel])]);
    let params = bare_params(&state_root);
    let runner = RecordingArgvRunner::new();
    let ctx = HubContext::with_ports(
        &params,
        Arc::new(runner.clone()),
        Arc::new(fetcher),
        Arc::new(registry),
    )
    .unwrap();

    // No caller-supplied reference: the declaration is what resolves {install}.
    let planned = plan_with(&ctx, &params).unwrap();
    assert_eq!(planned["status"], "planned", "{planned}");
    assert_eq!(
        planned["selectedChannel"]["placementArgv"],
        json!([
            "install",
            "-m",
            "0755",
            format!("{{staging}}/{ARCHIVE_NAME}"),
            "{install}"
        ])
    );

    let mut confirmed = params;
    confirmed["confirmation"] = planned["confirmation"].clone();
    let applied = apply_with(&ctx, &confirmed).unwrap();
    assert_eq!(applied["ok"], true, "{applied}");

    let recorded = runner.recorded();
    assert_eq!(recorded.len(), 3, "{recorded:?}");
    assert_eq!(recorded[0].0, "tar");
    assert_eq!(recorded[1].0, "install");
    let destination = fixture_destination();
    assert_eq!(
        recorded[1].1.last().cloned(),
        Some(destination.to_string_lossy().to_string())
    );
    // The placement copies the file the first step staged, from the Hub's own
    // staging root, to that one declared destination.
    let staged_archive = recorded[0].1[1].clone();
    let staging_dir = recorded[0].1[3].clone();
    assert!(staged_archive.ends_with(ARCHIVE_NAME), "{staged_archive}");
    assert!(staged_archive.contains("agent-hub"), "{staged_archive}");
    assert_eq!(recorded[1].1[2], staged_archive);
    assert!(staged_archive.starts_with(&staging_dir), "{recorded:?}");
    // Verification runs the executable at the declared destination, so an
    // unrelated binary that happens to answer to the same name cannot pass it.
    assert_eq!(
        recorded[2].0,
        destination.to_string_lossy().to_string(),
        "{recorded:?}"
    );
    assert_eq!(recorded[2].1, vec!["--version".to_string()]);
}

#[test]
fn a_caller_supplied_install_reference_still_wins_over_the_declaration() {
    let state_root = temp_dir("placement-supplied");
    let (channel, fetcher) = placed_channel();
    let registry = synthetic_registry(vec![synthetic_agent("synthetic", vec![channel])]);
    let mut params = bare_params(&state_root);
    params["installRef"] = json!("/tmp/synthetic-agent/caller-owned-agent");
    let runner = RecordingArgvRunner::new();
    let ctx = HubContext::with_ports(
        &params,
        Arc::new(runner.clone()),
        Arc::new(fetcher),
        Arc::new(registry),
    )
    .unwrap();

    let planned = plan_with(&ctx, &params).unwrap();
    let mut confirmed = params;
    confirmed["confirmation"] = planned["confirmation"].clone();
    apply_with(&ctx, &confirmed).unwrap();

    let recorded = runner.recorded();
    assert_eq!(
        recorded[1].1.last().cloned(),
        Some("/tmp/synthetic-agent/caller-owned-agent".to_string()),
        "{recorded:?}"
    );
}

#[test]
fn an_install_destination_discovery_never_looks_in_is_refused() {
    let state_root = temp_dir("placement-unadmitted");
    let (mut channel, fetcher) = placed_channel();
    channel.install.as_mut().unwrap().dir =
        [("macos".to_string(), "{home}/Desktop/tools".to_string())]
            .into_iter()
            .collect();
    let registry = synthetic_registry(vec![synthetic_agent("synthetic", vec![channel])]);
    let params = bare_params(&state_root);
    let ctx = HubContext::with_ports(
        &params,
        Arc::new(RecordingArgvRunner::new()),
        Arc::new(fetcher),
        Arc::new(registry),
    )
    .unwrap();

    let planned = plan_with(&ctx, &params).unwrap();
    assert_eq!(planned["ok"], false, "{planned}");
    assert_eq!(planned["code"], "install_destination_unadmitted");
}

#[test]
fn a_destination_the_declaration_does_not_cover_for_this_host_is_refused() {
    let state_root = temp_dir("placement-other-os");
    let (mut channel, fetcher) = placed_channel();
    channel.install.as_mut().unwrap().dir =
        [("linux".to_string(), "{home}/.local/bin".to_string())]
            .into_iter()
            .collect();
    channel.install.as_mut().unwrap().binary =
        [("linux".to_string(), "synthetic-agent".to_string())]
            .into_iter()
            .collect();
    let registry = synthetic_registry(vec![synthetic_agent("synthetic", vec![channel])]);
    let params = bare_params(&state_root);
    let ctx = HubContext::with_ports(
        &params,
        Arc::new(RecordingArgvRunner::new()),
        Arc::new(fetcher),
        Arc::new(registry),
    )
    .unwrap();

    let planned = plan_with(&ctx, &params).unwrap();
    assert_eq!(planned["ok"], false, "{planned}");
    assert_eq!(planned["code"], "install_destination_undeclared");
}
