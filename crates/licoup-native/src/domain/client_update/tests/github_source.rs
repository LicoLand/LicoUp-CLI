use super::support::*;

use base64::{Engine as _, engine::general_purpose};
use std::{
    io::{BufRead, BufReader, Write},
    net::{TcpListener, TcpStream},
    thread::{self, JoinHandle},
};

#[test]
fn bundled_public_keys_document_parses_with_decodable_ed25519_keys() {
    let document = super::super::github_source::bundled_public_keys_document().unwrap();
    let keys = document["keys"].as_object().unwrap();
    assert_eq!(keys.len(), 2);
    for entry in keys.values() {
        let encoded = entry["publicKey"].as_str().unwrap();
        let bytes = general_purpose::STANDARD.decode(encoded).unwrap();
        assert_eq!(bytes.len(), 32);
        ed25519_dalek::VerifyingKey::from_bytes(&bytes.try_into().unwrap()).unwrap();
    }
}

#[test]
fn github_release_source_uses_fixed_nightly_tag_and_latest_stable() {
    use crate::domain::client_state_migration::ReleaseTrack;

    assert_eq!(
        super::super::github_source::release_metadata_path_for_test(ReleaseTrack::Nightly),
        "/repos/LicoLand/LicoUp/releases/tags/nightly"
    );
    assert_eq!(
        super::super::github_source::release_metadata_path_for_test(ReleaseTrack::Stable),
        "/repos/LicoLand/LicoUp/releases/latest"
    );
}

#[test]
fn redirect_host_allowlist_rejects_foreign_hosts_and_accepts_github_and_loopback() {
    for url in [
        "https://evil.example.com/steal",
        "https://github.com.evil.example/steal",
        "http://foreign.example.test/steal",
    ] {
        assert!(super::super::github_source::validate_redirect_host_allowed_for_test(url).is_err());
    }
    for url in [
        "https://github.com/LicoLand/LicoUp/releases/download/v1/a.zip",
        "https://objects.githubusercontent.com/x",
        "https://api.github.com/x",
        "https://raw.githubusercontent.com/x",
        "http://127.0.0.1:54321/a",
        "http://localhost:54321/a",
    ] {
        assert!(super::super::github_source::validate_redirect_host_allowed_for_test(url).is_ok());
    }
}

enum FixtureReply {
    Body(String),
    Status(u16),
}

struct FixtureRoute {
    path: String,
    reply: FixtureReply,
}

struct FixtureServer {
    base: String,
    handle: JoinHandle<Vec<CapturedRequest>>,
}

struct CapturedRequest {
    path: String,
    user_agent: String,
}

/// Serves exactly the routes returned by `routes(base)` — the closure receives
/// the loopback base url first so fixture bodies (and signed manifests) can
/// embed the real url. Each request is answered by its matching route in order.
fn serve(routes: impl FnOnce(&str) -> Vec<FixtureRoute>) -> FixtureServer {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind loopback fixture");
    let base = format!("http://{}", listener.local_addr().expect("fixture address"));
    let resolved = routes(&base);
    let handle = thread::spawn(move || {
        let mut results = Vec::new();
        for route in resolved {
            let (mut stream, _) = listener.accept().expect("accept fixture request");
            let request = read_request(&mut stream);
            assert_eq!(request.path, route.path, "unexpected fixture request path");
            results.push(CapturedRequest {
                path: request.path,
                user_agent: request.user_agent,
            });
            match route.reply {
                FixtureReply::Body(body) => {
                    write!(
                        stream,
                        "HTTP/1.1 200 OK\r\ncontent-type: application/octet-stream\r\ncontent-length: {}\r\nconnection: close\r\n\r\n",
                        body.len()
                    )
                    .expect("write fixture headers");
                    stream
                        .write_all(body.as_bytes())
                        .expect("write fixture body");
                }
                FixtureReply::Status(status) => {
                    write!(
                        stream,
                        "HTTP/1.1 {status} Error\r\ncontent-length: 0\r\nconnection: close\r\n\r\n"
                    )
                    .expect("write fixture status");
                }
            }
        }
        results
    });
    FixtureServer { base, handle }
}

impl FixtureServer {
    fn finish(self) -> Vec<CapturedRequest> {
        self.handle.join().expect("fixture server thread")
    }
}

fn read_request(stream: &mut TcpStream) -> CapturedRequest {
    let mut reader = BufReader::new(stream.try_clone().expect("clone fixture stream"));
    let mut request_line = String::new();
    reader
        .read_line(&mut request_line)
        .expect("read request line");
    let mut parts = request_line.split_whitespace();
    let path = parts.nth(1).expect("request target").to_string();
    let mut user_agent = String::new();
    loop {
        let mut line = String::new();
        reader.read_line(&mut line).expect("read header");
        let line = line.trim_end_matches(['\r', '\n']);
        if line.is_empty() {
            break;
        }
        if let Some((name, value)) = line.split_once(':') {
            if name.trim().eq_ignore_ascii_case("user-agent") {
                user_agent = value.trim().to_string();
            }
        }
    }
    CapturedRequest { path, user_agent }
}

fn github_release_json(manifest_url: &str) -> String {
    serde_json::to_string(&json!({
        "tag_name": "v0.2.0",
        "html_url": "https://github.com/LicoLand/LicoUp/releases/tag/v0.2.0",
        "assets": [
            {
                "name": "LicoUp-update-manifest.json",
                "browser_download_url": manifest_url,
                "size": 1024,
            },
            {
                "name": "LicoUp-macos-arm64-update.zip",
                "browser_download_url": "https://github.com/LicoLand/LicoUp/releases/download/v0.2.0/LicoUp-macos-arm64-update.zip",
                "size": 4096,
            },
        ],
    }))
    .unwrap()
}

fn github_params(
    server: &FixtureServer,
    fixture: &UpdateFixture,
    state_root: &std::path::Path,
) -> Value {
    json!({
        "source": "github",
        "repo": "LicoLand/LicoUp",
        "githubApiBase": server.base,
        "targetReleaseTrack": "stable",
        "targetId": TARGET_ID,
        "stateRoot": state_root,
        "stagingRoot": fixture.staging,
        "publicKeys": fixture.public_keys(),
    })
}

fn manifest_routes(manifest: Value) -> impl FnOnce(&str) -> Vec<FixtureRoute> {
    move |base| {
        vec![
            FixtureRoute {
                path: "/repos/LicoLand/LicoUp/releases/latest".to_string(),
                reply: FixtureReply::Body(github_release_json(&format!("{base}/manifest.json"))),
            },
            FixtureRoute {
                path: "/manifest.json".to_string(),
                reply: FixtureReply::Body(serde_json::to_string(&manifest).unwrap()),
            },
        ]
    }
}

#[test]
fn client_update_github_check_fetches_verifies_and_caches_the_signed_manifest() {
    let fixture = UpdateFixture::new();
    let manifest = fixture.sign_manifest(
        fixture.unsigned_manifest(json!([release("999.0.0", fixture.artifact(TARGET_ID)),])),
    );
    let state_root = fixture.root.join("state");
    let server = serve(manifest_routes(manifest));

    let params = github_params(&server, &fixture, &state_root);
    let checked = super::super::github_source::check_github(&params).unwrap();
    assert_eq!(checked["ok"], true);
    assert_eq!(checked["source"], "github");
    assert_eq!(checked["updateAvailable"], true);
    assert_eq!(checked["availableVersion"], "999.0.0");
    assert_eq!(checked["githubReleaseTag"], "v0.2.0");
    assert_eq!(
        checked["githubReleaseUrl"],
        "https://github.com/LicoLand/LicoUp/releases/tag/v0.2.0"
    );
    assert_eq!(checked["artifactReceipt"]["targetId"], TARGET_ID);
    let requests = server.finish();
    assert!(
        requests
            .iter()
            .any(|request| request.user_agent.starts_with("LicoUpClientUpdate/"))
    );

    // The second check must hit the fresh cache: no server is reachable now.
    let cached = super::super::github_source::check_github(&params).unwrap();
    assert_eq!(cached["updateAvailable"], true);
    assert_eq!(cached["availableVersion"], "999.0.0");
    assert!(cached["cacheAgeSeconds"].as_u64().unwrap() <= 1);
}

#[test]
fn client_update_github_check_reports_up_to_date_when_no_eligible_release() {
    let fixture = UpdateFixture::new();
    let manifest = fixture.sign_manifest(
        fixture.unsigned_manifest(json!([release("0.0.0", fixture.artifact(TARGET_ID)),])),
    );
    let state_root = fixture.root.join("state");
    let server = serve(manifest_routes(manifest));

    let params = github_params(&server, &fixture, &state_root);
    let checked = super::super::github_source::check_github(&params).unwrap();
    assert_eq!(checked["ok"], true);
    assert_eq!(checked["updateAvailable"], false);
    assert_eq!(checked["phase"], "upToDate");
    server.finish();
}

#[test]
fn client_update_github_check_distinguishes_unavailable_metadata_from_no_new_release() {
    for (tag, track, phase) in [
        ("v0.0.0", "stable", "upToDate"),
        ("v999.0.0", "stable", "unavailable"),
        ("", "stable", "unavailable"),
        ("not-a-version", "stable", "unavailable"),
        ("nightly", "nightly", "unavailable"),
        ("v0.0.0", "nightly", "unavailable"),
    ] {
        let fixture = UpdateFixture::new();
        let state_root = fixture.root.join("state");
        let server = serve(|_| {
            vec![FixtureRoute {
                path: format!(
                    "/repos/LicoLand/LicoUp/releases/{}",
                    if track == "stable" {
                        "latest"
                    } else {
                        "tags/nightly"
                    }
                ),
                reply: FixtureReply::Body(
                    serde_json::to_string(&release_without_manifest(tag)).unwrap(),
                ),
            }]
        });
        let mut params = github_params(&server, &fixture, &state_root);
        params["targetReleaseTrack"] = json!(track);
        let checked = super::super::github_source::check_github(&params).unwrap();
        assert_eq!(checked["phase"], phase);
        assert_eq!(checked["updateAvailable"], false);
        assert_eq!(checked["availabilityEvidence"], "publishedReleaseMetadata");
        for field in [
            "artifactReceipt",
            "artifact",
            "verifiedKeyIds",
            "manifestSha256",
        ] {
            assert!(
                checked.get(field).is_none(),
                "unsigned observation contains {field}"
            );
        }
        if phase == "unavailable" {
            assert_eq!(checked["errorCode"], "client_update_metadata_unavailable");
        }
        assert!(!state_root.join("active-artifact-receipt.json").exists());
        server.finish();
    }
}

fn release_without_manifest(tag: &str) -> Value {
    json!({
        "tag_name": tag,
        "html_url": format!("https://github.com/LicoLand/LicoUp/releases/tag/{tag}"),
        "draft": false,
        "prerelease": false,
        "assets": [],
    })
}

#[test]
fn client_update_github_public_version_observation_requires_canonical_stable_metadata() {
    use super::super::github_source::stable_metadata_has_no_newer_release;
    let current = semver::Version::parse("1.2.3+installed").unwrap();
    for (tag, expected) in [
        ("v1.2.3", true),
        ("v1.2.3+published", true),
        ("v1.2.2", true),
        ("v1.2.4", false),
        ("v1.2.3-rc.1", false),
        ("nightly", false),
        ("", false),
    ] {
        let release = release_without_manifest(tag);
        assert_eq!(
            stable_metadata_has_no_newer_release(
                "LicoLand/LicoUp",
                &release,
                tag,
                release["html_url"].as_str().unwrap(),
                &current
            ),
            expected
        );
    }
    let tag = "v1.2.3";
    let release = release_without_manifest(tag);
    let url = release["html_url"].as_str().unwrap();
    for field in ["draft", "prerelease"] {
        let mut invalid = release.clone();
        invalid[field] = json!(true);
        assert!(!stable_metadata_has_no_newer_release(
            "LicoLand/LicoUp",
            &invalid,
            tag,
            url,
            &current
        ));
    }
    assert!(!stable_metadata_has_no_newer_release(
        "Other/Repo",
        &release,
        tag,
        url,
        &current
    ));
    assert!(!stable_metadata_has_no_newer_release(
        "LicoLand/LicoUp",
        &release,
        tag,
        "https://example.invalid/release",
        &current
    ));
}

#[test]
fn client_update_github_check_keeps_http_failures_as_failures() {
    for status in [401, 403, 404, 500] {
        let fixture = UpdateFixture::new();
        let state_root = fixture.root.join("state");
        let server = serve(|_| {
            vec![FixtureRoute {
                path: "/repos/LicoLand/LicoUp/releases/latest".to_string(),
                reply: FixtureReply::Status(status),
            }]
        });
        let params = github_params(&server, &fixture, &state_root);
        assert!(super::super::github_source::check_github(&params).is_err());
        server.finish();
    }
}

#[test]
fn client_update_github_no_update_observation_invalidates_previous_artifact_receipt() {
    let fixture = UpdateFixture::new();
    let state_root = fixture.root.join("state");
    let signed = fixture.manifest();
    let server = serve(manifest_routes(signed));
    let params = github_params(&server, &fixture, &state_root);
    super::super::github_source::check_github(&params).unwrap();
    server.finish();
    assert!(state_root.join("active-artifact-receipt.json").exists());
    expire_cache(&state_root);
    let server = serve(|_| {
        vec![FixtureRoute {
            path: "/repos/LicoLand/LicoUp/releases/latest".to_string(),
            reply: FixtureReply::Body(
                serde_json::to_string(&release_without_manifest("v0.0.0")).unwrap(),
            ),
        }]
    });
    let params = github_params(&server, &fixture, &state_root);
    let checked = super::super::github_source::check_github(&params).unwrap();
    assert_eq!(checked["phase"], "upToDate");
    assert!(!state_root.join("active-artifact-receipt.json").exists());
    let mut download_params = params;
    download_params
        .as_object_mut()
        .unwrap()
        .remove("targetReleaseTrack");
    assert!(
        super::super::github_source::download_github(&download_params)
            .unwrap_err()
            .to_string()
            .contains("signed check receipt is required")
    );
    server.finish();
}

fn expire_cache(state_root: &std::path::Path) {
    let path = state_root.join("client-update-github/stable/manifest.json");
    let contents = fs::read_to_string(&path).unwrap();
    let (entry, manifest) = contents.split_once('\n').unwrap();
    let mut entry: Value = serde_json::from_str(entry).unwrap();
    entry["checkedAtEpochSeconds"] = json!(0);
    fs::write(
        path,
        format!("{}\n{manifest}", serde_json::to_string(&entry).unwrap()),
    )
    .unwrap();
}

#[test]
fn client_update_github_check_rejects_tampered_manifest() {
    let fixture = UpdateFixture::new();
    let mut manifest = fixture.sign_manifest(
        fixture.unsigned_manifest(json!([release("0.0.0", fixture.artifact(TARGET_ID)),])),
    );
    let state_root = fixture.root.join("state");
    let server = serve(manifest_routes(manifest.clone()));
    let params = github_params(&server, &fixture, &state_root);
    let cached = super::super::github_source::check_github(&params).unwrap();
    assert_eq!(cached["phase"], "upToDate");
    server.finish();
    expire_cache(&state_root);

    manifest["releases"][0]["releaseNotesUrl"] = json!("https://updates.invalid/tampered");
    let server = serve(|base| {
        let mut release = release_without_manifest("v0.0.0");
        release["assets"] = json!([{
            "name": "LicoUp-update-manifest.json",
            "browser_download_url": format!("{base}/manifest.json"),
        }]);
        vec![
            FixtureRoute {
                path: "/repos/LicoLand/LicoUp/releases/latest".to_string(),
                reply: FixtureReply::Body(serde_json::to_string(&release).unwrap()),
            },
            FixtureRoute {
                path: "/manifest.json".to_string(),
                reply: FixtureReply::Body(serde_json::to_string(&manifest).unwrap()),
            },
        ]
    });

    let params = github_params(&server, &fixture, &state_root);
    let error = super::super::github_source::check_github(&params)
        .unwrap_err()
        .to_string();
    assert!(error.contains("verification failed") || error.contains("signature"));
    assert!(!state_root.join("active-artifact-receipt.json").exists());
    server.finish();
}

#[test]
fn client_update_github_download_stages_verified_artifact_bytes() {
    let fixture = UpdateFixture::new();
    let artifact_body = String::from_utf8(fs::read(&fixture.source).unwrap()).unwrap();
    let state_root = fixture.root.join("state");
    let server = serve(|base| {
        let artifact = json!({
            "targetId": TARGET_ID,
            "platform": "test",
            "osFamily": "test",
            "arch": "test",
            "installerStrategy": "portable-replacement",
            "url": format!("{base}/artifact.bin"),
            "fileName": "artifact.bin",
            "size": artifact_body.len(),
            "sha256": sha256_hex(artifact_body.as_bytes()),
        });
        let manifest = fixture
            .sign_manifest(fixture.unsigned_manifest(json!([release("999.0.0", artifact),])));
        vec![
            FixtureRoute {
                path: "/repos/LicoLand/LicoUp/releases/latest".to_string(),
                reply: FixtureReply::Body(github_release_json(&format!("{base}/manifest.json"))),
            },
            FixtureRoute {
                path: "/manifest.json".to_string(),
                reply: FixtureReply::Body(serde_json::to_string(&manifest).unwrap()),
            },
            FixtureRoute {
                path: "/artifact.bin".to_string(),
                reply: FixtureReply::Body(artifact_body.clone()),
            },
        ]
    });

    let mut params = github_params(&server, &fixture, &state_root);
    let checked = super::super::github_source::check_github(&params).unwrap();
    assert_eq!(checked["updateAvailable"], true);
    params.as_object_mut().unwrap().remove("targetReleaseTrack");

    let downloaded = super::super::github_source::download_github(&params).unwrap();
    assert_eq!(downloaded["ok"], true);
    assert_eq!(downloaded["phase"], "downloaded");
    assert_eq!(downloaded["resumed"], false);
    assert_eq!(
        downloaded["artifactSha256"],
        sha256_hex(artifact_body.as_bytes())
    );

    // Second download reuses the verified staged artifact without network.
    let resumed = super::super::github_source::download_github(&params).unwrap();
    assert_eq!(resumed["resumed"], true);
    server.finish();
}

#[test]
fn client_update_github_download_rejects_size_mismatch() {
    let fixture = UpdateFixture::new();
    let artifact_body = String::from_utf8(fs::read(&fixture.source).unwrap()).unwrap();
    let state_root = fixture.root.join("state");
    let server = serve(|base| {
        let artifact = json!({
            "targetId": TARGET_ID,
            "platform": "test",
            "osFamily": "test",
            "arch": "test",
            "installerStrategy": "portable-replacement",
            "url": format!("{base}/artifact.bin"),
            "fileName": "artifact.bin",
            "size": artifact_body.len() + 7,
            "sha256": sha256_hex(artifact_body.as_bytes()),
        });
        let manifest = fixture
            .sign_manifest(fixture.unsigned_manifest(json!([release("999.0.0", artifact),])));
        vec![
            FixtureRoute {
                path: "/repos/LicoLand/LicoUp/releases/latest".to_string(),
                reply: FixtureReply::Body(github_release_json(&format!("{base}/manifest.json"))),
            },
            FixtureRoute {
                path: "/manifest.json".to_string(),
                reply: FixtureReply::Body(serde_json::to_string(&manifest).unwrap()),
            },
            FixtureRoute {
                path: "/artifact.bin".to_string(),
                reply: FixtureReply::Body(artifact_body),
            },
        ]
    });

    let mut params = github_params(&server, &fixture, &state_root);
    let _ = super::super::github_source::check_github(&params).unwrap();
    params.as_object_mut().unwrap().remove("targetReleaseTrack");
    let error = super::super::github_source::download_github(&params)
        .unwrap_err()
        .to_string();
    assert!(error.contains("size does not match signed metadata"));
    server.finish();
}

#[test]
fn client_update_github_context_injects_cached_manifest_and_bundled_keys() {
    let fixture = UpdateFixture::new();
    let manifest = fixture.sign_manifest(
        fixture.unsigned_manifest(json!([release("999.0.0", fixture.artifact(TARGET_ID)),])),
    );
    let state_root = fixture.root.join("state");
    let server = serve(manifest_routes(manifest));

    let mut params = github_params(&server, &fixture, &state_root);
    let _ = super::super::github_source::check_github(&params).unwrap();
    params.as_object_mut().unwrap().remove("targetReleaseTrack");

    let effective = super::super::github_source::github_context_params(&params).unwrap();
    assert!(effective.get("manifestJson").is_some());
    assert!(effective.get("publicKeys").is_some());
    server.finish();
}
