use super::super::acquisition::{
    self, AcquisitionFailure, AcquisitionRequest, ArtifactRole, RecordingArtifactFetcher,
    VendorArtifactFetcher,
};
use super::super::contract::{
    AgentRecipe, ArtifactIntegrity, InstallChannel, PlatformInstallCapabilities,
};
use super::support::{
    FixtureReply, FixtureRoute, bare_host_capabilities, digest_document, fixture_artifact_channel,
    fixture_params, serve, serve_with, sha256_hex, synthetic_agent, temp_dir,
};
use crate::platform::client_state::ClientStateStore;

const ARCHIVE_NAME: &str = "agent-darwin-arm64.tar.gz";

fn integrity_document(base: &str) -> ArtifactIntegrity {
    ArtifactIntegrity {
        algorithm: "sha256".to_string(),
        digest: None,
        digest_url_template: Some(format!("{base}/{ARCHIVE_NAME}.sha256")),
    }
}

fn pinned_integrity(digest: &str) -> ArtifactIntegrity {
    ArtifactIntegrity {
        algorithm: "sha256".to_string(),
        digest: Some(digest.to_string()),
        digest_url_template: None,
    }
}

fn stage_request<'a>(
    agent: &'a AgentRecipe,
    channel: &'a InstallChannel,
    capabilities: &'a PlatformInstallCapabilities,
) -> AcquisitionRequest<'a> {
    AcquisitionRequest {
        agent,
        channel,
        capabilities,
    }
}

fn failure_of(error: anyhow::Error) -> AcquisitionFailure {
    error
        .downcast::<AcquisitionFailure>()
        .unwrap_or_else(|error| panic!("expected a typed acquisition failure, got {error}"))
}

#[test]
fn a_published_digest_document_stages_verified_vendor_bytes() {
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
        FixtureRoute {
            path: format!("/{ARCHIVE_NAME}.sha256"),
            reply: FixtureReply::Body(digest_document(ARCHIVE_NAME, &body)),
        },
    ]);
    let base = server.base();
    let state_root = temp_dir("stage-verified");
    let store = ClientStateStore::new(state_root.clone()).unwrap();
    let channel = fixture_artifact_channel(&base, Some(integrity_document(&base)));
    let agent = synthetic_agent("synthetic", vec![channel.clone()]);
    let capabilities = bare_host_capabilities("macos", "aarch64");
    let params = fixture_params(&state_root);

    let staged = acquisition::stage(
        &store,
        &params,
        &stage_request(&agent, &channel, &capabilities),
        ArtifactRole::Archive,
        &VendorArtifactFetcher,
    )
    .unwrap();

    assert_eq!(staged.role, ArtifactRole::Archive);
    assert_eq!(staged.bytes, body.len() as u64);
    assert_eq!(staged.sha256, sha256_hex(&body));
    assert_eq!(std::fs::read(&staged.file_path).unwrap(), body);
    // Acquisition stages inside the Hub's own state, never an install location.
    assert!(
        staged.file_path.starts_with(&staged.staging_dir),
        "{:?}",
        staged.file_path
    );
    assert!(staged.staging_dir.starts_with(state_root.join("agent-hub")));
    assert!(!staged.resumed);

    // The published digest of a moving release is re-read on every acquisition,
    // and the bytes that already match it are reused instead of re-fetched.
    let resumed = acquisition::stage(
        &store,
        &params,
        &stage_request(&agent, &channel, &capabilities),
        ArtifactRole::Archive,
        &VendorArtifactFetcher,
    )
    .unwrap();
    assert!(resumed.resumed);
    assert_eq!(resumed.sha256, staged.sha256);
    assert_eq!(
        server.finish(),
        vec![
            format!("/{ARCHIVE_NAME}.sha256"),
            format!("/{ARCHIVE_NAME}"),
            format!("/{ARCHIVE_NAME}.sha256")
        ]
    );
}

#[test]
fn a_digest_pinned_by_the_recipe_resumes_without_any_source_request() {
    let body = b"synthetic vendor archive".to_vec();
    let server = serve(vec![FixtureRoute {
        path: format!("/{ARCHIVE_NAME}"),
        reply: FixtureReply::Body(String::from_utf8(body.clone()).unwrap()),
    }]);
    let base = server.base();
    let state_root = temp_dir("pinned-resume");
    let store = ClientStateStore::new(state_root.clone()).unwrap();
    let channel = fixture_artifact_channel(&base, Some(pinned_integrity(&sha256_hex(&body))));
    let agent = synthetic_agent("synthetic", vec![channel.clone()]);
    let capabilities = bare_host_capabilities("macos", "aarch64");
    let params = fixture_params(&state_root);

    let staged = acquisition::stage(
        &store,
        &params,
        &stage_request(&agent, &channel, &capabilities),
        ArtifactRole::Archive,
        &VendorArtifactFetcher,
    )
    .unwrap();
    assert!(!staged.resumed);

    // The recipe pins the digest, so a resume needs no source request at all:
    // the fixture listener is closed, and any fetch would fail the test.
    let resumed = acquisition::stage(
        &store,
        &params,
        &stage_request(&agent, &channel, &capabilities),
        ArtifactRole::Archive,
        &VendorArtifactFetcher,
    )
    .unwrap();
    assert!(resumed.resumed);
    assert_eq!(resumed.sha256, staged.sha256);
    server.finish();
}

#[test]
fn a_published_digest_that_disagrees_with_the_bytes_stages_nothing() {
    let body = "synthetic vendor archive".to_string();
    let server = serve(vec![
        FixtureRoute {
            path: format!("/{ARCHIVE_NAME}.sha256"),
            reply: FixtureReply::Body(format!("{}  {ARCHIVE_NAME}\n", "a".repeat(64))),
        },
        FixtureRoute {
            path: format!("/{ARCHIVE_NAME}"),
            reply: FixtureReply::Body(body),
        },
    ]);
    let base = server.base();
    let state_root = temp_dir("stage-mismatch");
    let store = ClientStateStore::new(state_root.clone()).unwrap();
    let channel = fixture_artifact_channel(&base, Some(integrity_document(&base)));
    let agent = synthetic_agent("synthetic", vec![channel.clone()]);
    let capabilities = bare_host_capabilities("macos", "aarch64");
    let params = fixture_params(&state_root);

    let error = acquisition::stage(
        &store,
        &params,
        &stage_request(&agent, &channel, &capabilities),
        ArtifactRole::Archive,
        &VendorArtifactFetcher,
    )
    .unwrap_err();
    assert_eq!(
        failure_of(error).code,
        acquisition::ARTIFACT_INTEGRITY_MISMATCH
    );
    assert_eq!(server.finish().len(), 2);
    let staged_path = state_root
        .join("agent-hub/staging/synthetic/official-artifact")
        .join(ARCHIVE_NAME);
    assert!(
        !staged_path.exists(),
        "an unverified artifact must not stay staged: {staged_path:?}"
    );
}

#[test]
fn a_source_without_a_published_digest_is_refused_before_any_fetch() {
    let state_root = temp_dir("integrity-undeclared");
    let store = ClientStateStore::new(state_root.clone()).unwrap();
    let fetcher = RecordingArtifactFetcher::new();
    let channel = fixture_artifact_channel("https://vendor.invalid", None);
    let agent = synthetic_agent("synthetic", vec![channel.clone()]);
    let capabilities = bare_host_capabilities("macos", "aarch64");
    let params = fixture_params(&state_root);

    let error = acquisition::stage(
        &store,
        &params,
        &stage_request(&agent, &channel, &capabilities),
        ArtifactRole::Archive,
        &fetcher,
    )
    .unwrap_err();
    assert_eq!(
        failure_of(error).code,
        acquisition::ARTIFACT_INTEGRITY_UNDECLARED
    );
    assert!(fetcher.requests().is_empty());
}

#[test]
fn a_digest_document_that_publishes_nothing_is_refused() {
    let server = serve(vec![FixtureRoute {
        path: format!("/{ARCHIVE_NAME}.sha256"),
        reply: FixtureReply::Body("# no digest here\n".to_string()),
    }]);
    let base = server.base();
    let state_root = temp_dir("digest-empty");
    let store = ClientStateStore::new(state_root.clone()).unwrap();
    let channel = fixture_artifact_channel(&base, Some(integrity_document(&base)));
    let agent = synthetic_agent("synthetic", vec![channel.clone()]);
    let capabilities = bare_host_capabilities("macos", "aarch64");
    let params = fixture_params(&state_root);

    let error = acquisition::stage(
        &store,
        &params,
        &stage_request(&agent, &channel, &capabilities),
        ArtifactRole::Archive,
        &VendorArtifactFetcher,
    )
    .unwrap_err();
    assert_eq!(
        failure_of(error).code,
        acquisition::ARTIFACT_DIGEST_UNAVAILABLE
    );
    server.finish();
}

#[test]
fn a_vendor_artifact_beyond_the_bound_is_refused() {
    let server = serve(vec![
        FixtureRoute {
            path: format!("/{ARCHIVE_NAME}.sha256"),
            reply: FixtureReply::Body(digest_document(ARCHIVE_NAME, b"x")),
        },
        FixtureRoute {
            path: format!("/{ARCHIVE_NAME}"),
            reply: FixtureReply::OversizedLength,
        },
    ]);
    let base = server.base();
    let state_root = temp_dir("oversized");
    let store = ClientStateStore::new(state_root.clone()).unwrap();
    let channel = fixture_artifact_channel(&base, Some(integrity_document(&base)));
    let agent = synthetic_agent("synthetic", vec![channel.clone()]);
    let capabilities = bare_host_capabilities("macos", "aarch64");
    let params = fixture_params(&state_root);

    let error = acquisition::stage(
        &store,
        &params,
        &stage_request(&agent, &channel, &capabilities),
        ArtifactRole::Archive,
        &VendorArtifactFetcher,
    )
    .unwrap_err();
    assert_eq!(failure_of(error).code, acquisition::ARTIFACT_SIZE_EXCEEDED);
    server.finish();
}

#[test]
fn a_redirect_that_leaves_the_declared_origin_is_refused() {
    let server = serve(vec![FixtureRoute {
        path: format!("/{ARCHIVE_NAME}"),
        reply: FixtureReply::Redirect("https://downloads.other.invalid/agent.tar.gz".to_string()),
    }]);
    let base = server.base();
    let state_root = temp_dir("redirect");
    let store = ClientStateStore::new(state_root.clone()).unwrap();
    // A pinned digest keeps this test on the artifact fetch alone.
    let mut channel = fixture_artifact_channel(&base, Some(pinned_integrity(&"b".repeat(64))));
    // A declared redirect host admits that host alone, not whichever host a
    // vendor answer happens to name.
    channel.artifact.as_mut().unwrap().redirect_hosts = vec!["assets.vendor.invalid".to_string()];
    let agent = synthetic_agent("synthetic", vec![channel.clone()]);
    let capabilities = bare_host_capabilities("macos", "aarch64");
    let params = fixture_params(&state_root);

    let error = acquisition::stage(
        &store,
        &params,
        &stage_request(&agent, &channel, &capabilities),
        ArtifactRole::Archive,
        &VendorArtifactFetcher,
    )
    .unwrap_err();
    assert_eq!(
        failure_of(error).code,
        acquisition::ARTIFACT_ORIGIN_MISMATCH
    );
    server.finish();
}

#[test]
fn a_redirect_to_a_host_the_recipe_declares_is_followed() {
    let body = "synthetic vendor archive";
    // One fixture listener answers under two names: the declared origin and the
    // second hostname the recipe declares, which is what a vendor's own content
    // network is. Only the declaration makes the second one reachable.
    let server = serve_with(|base| {
        let redirected = base.replace("127.0.0.1", "localhost");
        vec![
            FixtureRoute {
                path: format!("/{ARCHIVE_NAME}"),
                reply: FixtureReply::Redirect(format!("{redirected}/{ARCHIVE_NAME}")),
            },
            FixtureRoute {
                path: format!("/{ARCHIVE_NAME}"),
                reply: FixtureReply::Body(body.to_string()),
            },
        ]
    });
    let base = server.base();
    let state_root = temp_dir("redirect-declared");
    let store = ClientStateStore::new(state_root.clone()).unwrap();
    // A pinned digest keeps this test on the artifact fetch alone.
    let mut channel =
        fixture_artifact_channel(&base, Some(pinned_integrity(&sha256_hex(body.as_bytes()))));
    channel.artifact.as_mut().unwrap().redirect_hosts = vec!["localhost".to_string()];
    let agent = synthetic_agent("synthetic", vec![channel.clone()]);
    let capabilities = bare_host_capabilities("macos", "aarch64");
    let params = fixture_params(&state_root);

    let staged = acquisition::stage(
        &store,
        &params,
        &stage_request(&agent, &channel, &capabilities),
        ArtifactRole::Archive,
        &VendorArtifactFetcher,
    )
    .unwrap();
    assert_eq!(staged.sha256, sha256_hex(body.as_bytes()));
    assert_eq!(staged.bytes, body.len() as u64);
    assert_eq!(std::fs::read(&staged.file_path).unwrap(), body.as_bytes());
    assert_eq!(
        server.finish(),
        vec![format!("/{ARCHIVE_NAME}"), format!("/{ARCHIVE_NAME}")]
    );
}

#[test]
fn a_first_request_to_a_declared_redirect_host_is_refused() {
    // Declaring a redirect host widens hops, never the origin: a URL template
    // that names the declared host instead of the origin is refused before any
    // request is made.
    let mut channel = fixture_artifact_channel("http://127.0.0.1:9", None);
    let artifact = channel.artifact.as_mut().unwrap();
    artifact.origin_host = "127.0.0.1".to_string();
    artifact.url_template = "http://localhost:9/agent-{vendorOs}-{vendorArch}.tar.gz".to_string();
    artifact.redirect_hosts = vec!["localhost".to_string()];
    let capabilities = bare_host_capabilities("macos", "aarch64");

    let error =
        acquisition::artifact_url(channel.artifact.as_ref().unwrap(), &capabilities, "latest")
            .unwrap_err();
    assert_eq!(
        failure_of(error).code,
        acquisition::ARTIFACT_ORIGIN_MISMATCH
    );
}

#[test]
fn a_template_the_recipe_cannot_resolve_is_refused() {
    let state_root = temp_dir("template");
    let store = ClientStateStore::new(state_root.clone()).unwrap();
    let mut channel = fixture_artifact_channel("https://vendor.invalid", None);
    channel.artifact.as_mut().unwrap().url_template =
        "https://vendor.invalid/agent-{vendorOs}-{vendorArch}-{missing}.tar.gz".to_string();
    let agent = synthetic_agent("synthetic", vec![channel.clone()]);
    let capabilities = bare_host_capabilities("macos", "aarch64");
    let params = fixture_params(&state_root);
    let error = acquisition::stage(
        &store,
        &params,
        &stage_request(&agent, &channel, &capabilities),
        ArtifactRole::Archive,
        &RecordingArtifactFetcher::new(),
    )
    .unwrap_err();
    assert_eq!(failure_of(error).code, acquisition::ARTIFACT_URL_INCOMPLETE);
}

#[test]
fn a_source_host_outside_the_declared_origin_is_refused() {
    let state_root = temp_dir("origin");
    let store = ClientStateStore::new(state_root.clone()).unwrap();
    let mut channel = fixture_artifact_channel("https://vendor.invalid", None);
    channel.artifact.as_mut().unwrap().url_template =
        "https://downloads.other.invalid/agent-{vendorOs}-{vendorArch}.tar.gz".to_string();
    let agent = synthetic_agent("synthetic", vec![channel.clone()]);
    let capabilities = bare_host_capabilities("macos", "aarch64");
    let params = fixture_params(&state_root);
    let error = acquisition::stage(
        &store,
        &params,
        &stage_request(&agent, &channel, &capabilities),
        ArtifactRole::Archive,
        &RecordingArtifactFetcher::new(),
    )
    .unwrap_err();
    assert_eq!(
        failure_of(error).code,
        acquisition::ARTIFACT_ORIGIN_MISMATCH
    );
}

#[test]
fn a_plain_http_origin_is_refused_outside_the_local_fixture() {
    let state_root = temp_dir("plain-http");
    let store = ClientStateStore::new(state_root.clone()).unwrap();
    let mut channel = fixture_artifact_channel("https://vendor.invalid", None);
    channel.artifact.as_mut().unwrap().url_template =
        "http://vendor.invalid/agent-{vendorOs}-{vendorArch}.tar.gz".to_string();
    let agent = synthetic_agent("synthetic", vec![channel.clone()]);
    let capabilities = bare_host_capabilities("macos", "aarch64");
    let params = fixture_params(&state_root);
    let error = acquisition::stage(
        &store,
        &params,
        &stage_request(&agent, &channel, &capabilities),
        ArtifactRole::Archive,
        &RecordingArtifactFetcher::new(),
    )
    .unwrap_err();
    assert_eq!(
        failure_of(error).code,
        acquisition::ARTIFACT_ORIGIN_MISMATCH
    );
}

#[test]
fn a_multi_entry_checksum_document_binds_the_named_artifact() {
    let body = b"synthetic vendor archive".to_vec();
    let document = format!(
        "{}  other-agent.tar.gz\n{}  {ARCHIVE_NAME}\n",
        "c".repeat(64),
        sha256_hex(&body)
    );
    let server = serve(vec![
        FixtureRoute {
            path: format!("/{ARCHIVE_NAME}.sha256"),
            reply: FixtureReply::Body(document),
        },
        FixtureRoute {
            path: format!("/{ARCHIVE_NAME}"),
            reply: FixtureReply::Body(String::from_utf8(body.clone()).unwrap()),
        },
    ]);
    let base = server.base();
    let state_root = temp_dir("checksum-named");
    let store = ClientStateStore::new(state_root.clone()).unwrap();
    let channel = fixture_artifact_channel(&base, Some(integrity_document(&base)));
    let agent = synthetic_agent("synthetic", vec![channel.clone()]);
    let capabilities = bare_host_capabilities("macos", "aarch64");
    let params = fixture_params(&state_root);
    let staged = acquisition::stage(
        &store,
        &params,
        &stage_request(&agent, &channel, &capabilities),
        ArtifactRole::Archive,
        &VendorArtifactFetcher,
    )
    .unwrap();
    assert_eq!(staged.sha256, sha256_hex(&body));
    server.finish();
}

#[test]
fn the_recording_port_stages_a_script_role_from_synthetic_bytes() {
    let state_root = temp_dir("recording");
    let store = ClientStateStore::new(state_root.clone()).unwrap();
    let fetcher = RecordingArtifactFetcher::new();
    let body = b"install script body".to_vec();
    fetcher.serve(ARCHIVE_NAME, body.clone());
    fetcher.serve(
        &format!("{ARCHIVE_NAME}.sha256"),
        digest_document(ARCHIVE_NAME, &body).into_bytes(),
    );
    let channel = fixture_artifact_channel(
        "https://vendor.invalid",
        Some(integrity_document("https://vendor.invalid")),
    );
    let agent = synthetic_agent("synthetic", vec![channel.clone()]);
    let capabilities = bare_host_capabilities("macos", "aarch64");
    let params = fixture_params(&state_root);
    let staged = acquisition::stage(
        &store,
        &params,
        &stage_request(&agent, &channel, &capabilities),
        ArtifactRole::Script,
        &fetcher,
    )
    .unwrap();
    assert_eq!(staged.role, ArtifactRole::Script);
    assert_eq!(staged.bytes, body.len() as u64);
    assert_eq!(
        fetcher.requests().len(),
        2,
        "the digest document and the artifact are both published sources"
    );
}

#[test]
fn an_artifact_url_without_a_usable_file_name_is_refused() {
    let state_root = temp_dir("file-name");
    let store = ClientStateStore::new(state_root.clone()).unwrap();
    let mut channel = fixture_artifact_channel("https://vendor.invalid", None);
    channel.artifact.as_mut().unwrap().url_template = "https://vendor.invalid/".to_string();
    let agent = synthetic_agent("synthetic", vec![channel.clone()]);
    let capabilities = bare_host_capabilities("macos", "aarch64");
    let params = fixture_params(&state_root);
    let error = acquisition::stage(
        &store,
        &params,
        &stage_request(&agent, &channel, &capabilities),
        ArtifactRole::Archive,
        &RecordingArtifactFetcher::new(),
    )
    .unwrap_err();
    assert_eq!(failure_of(error).code, acquisition::ARTIFACT_NAME_INVALID);
}

#[test]
fn a_digest_document_outside_the_declared_origin_is_refused() {
    let state_root = temp_dir("digest-origin");
    let store = ClientStateStore::new(state_root.clone()).unwrap();
    let channel = fixture_artifact_channel(
        "https://vendor.invalid",
        Some(ArtifactIntegrity {
            algorithm: "sha256".to_string(),
            digest: None,
            digest_url_template: Some("https://downloads.other.invalid/a.sha256".to_string()),
        }),
    );
    let agent = synthetic_agent("synthetic", vec![channel.clone()]);
    let capabilities = bare_host_capabilities("macos", "aarch64");
    let params = fixture_params(&state_root);
    let error = acquisition::stage(
        &store,
        &params,
        &stage_request(&agent, &channel, &capabilities),
        ArtifactRole::Archive,
        &RecordingArtifactFetcher::new(),
    )
    .unwrap_err();
    assert_eq!(
        failure_of(error).code,
        acquisition::ARTIFACT_ORIGIN_MISMATCH
    );
}

#[test]
fn an_unsupported_integrity_algorithm_is_refused() {
    let state_root = temp_dir("algorithm");
    let store = ClientStateStore::new(state_root.clone()).unwrap();
    let channel = fixture_artifact_channel(
        "https://vendor.invalid",
        Some(ArtifactIntegrity {
            algorithm: "md5".to_string(),
            digest: Some("d".repeat(32)),
            digest_url_template: None,
        }),
    );
    let agent = synthetic_agent("synthetic", vec![channel.clone()]);
    let capabilities = bare_host_capabilities("macos", "aarch64");
    let params = fixture_params(&state_root);
    let error = acquisition::stage(
        &store,
        &params,
        &stage_request(&agent, &channel, &capabilities),
        ArtifactRole::Archive,
        &RecordingArtifactFetcher::new(),
    )
    .unwrap_err();
    assert_eq!(
        failure_of(error).code,
        acquisition::ARTIFACT_INTEGRITY_ALGORITHM_UNSUPPORTED
    );
}
