//! GitHub release source detection and artifact fetching for client updates.
//!
//! The client trusts only the bundled Ed25519 public keys; the signed update
//! manifest is fetched from the latest GitHub release of the configured
//! repository, verified through the same signature chain as the local flow,
//! and the selected artifact is streamed into staging under the same
//! exact-size + sha256 gate as the local flow.

use std::{
    fs,
    io::{Read, Write},
    net::IpAddr,
    path::PathBuf,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use anyhow::{Context, Result, anyhow, bail, ensure};
use semver::Version;
use serde_json::{Value, json};
use ureq::{Agent, AgentBuilder};

use super::{
    constants::{CLIENT_UPDATE_MODE, MAX_UPDATE_METADATA_BYTES},
    download::download_result_json,
    params::{json_text, validate_public_identifier},
    selection::require_available_selection,
    staging::{finalize_partial_artifact, prepare_staging_root, staged_artifact_paths},
};

pub(super) const UPDATE_MANIFEST_ASSET: &str = "LicoUp-update-manifest.json";
/// Contract constant for the release tooling asset name; the client never
/// fetches the keys document (keys are bundled at build time).
#[allow(dead_code)]
pub(super) const UPDATE_KEYS_ASSET: &str = "LicoUp-update-public-keys.json";
const DEFAULT_REPO: &str = "LicoLand/LicoUp";
const DEFAULT_API_BASE: &str = "https://api.github.com";
const MAX_RELEASE_METADATA_BYTES: u64 = 2 * 1024 * 1024;
const RELEASE_CACHE_TTL_SECONDS: u64 = 6 * 60 * 60;
const MAX_REDIRECTS: usize = 4;
const UPDATE_UA: &str = "LicoUpClientUpdate/0.1 (LicoLand; self-update)";
const MAX_ARTIFACT_DOWNLOAD_BYTES: u64 = 4 * 1024 * 1024 * 1024;
const REDIRECT_ALLOWED_HOSTS: &[&str] = &[
    "github.com",
    "api.github.com",
    "raw.githubusercontent.com",
    "objects.githubusercontent.com",
    "githubusercontent.com",
];

/// `check --source github`: fetch the signed update manifest from the latest
/// GitHub release, verify it with the bundled keys, and select the highest
/// eligible release for this client. A fresh cache (6 h TTL) skips the network
/// hop. Once refreshed, a fetch or verification failure stays a failure.
pub(super) fn check_github(params: &Value) -> Result<Value> {
    let repo = github_repo(params)?;
    // A new observation replaces the previous check, including when it fails.
    // Cached manifests alone cannot authorize download, verify, or apply.
    super::receipt::bind_check_result(params, &json!({"updateAvailable": false}))?;
    let cached = read_cached_release(params)?;
    if let Some(cached) = cached.as_ref() {
        if is_fresh(cached.checked_at) {
            let checked = verify_with_document(params, &cached.manifest)?;
            return bind_github_check(
                params,
                decorate_check(
                    checked,
                    cached.tag.clone(),
                    cached.url.clone(),
                    Some(cached.age),
                ),
            );
        }
    }
    let target_track = super::params::target_release_track(params)?;
    let release = fetch_release_metadata(&repo, &github_api_base(params), target_track)?;
    let tag = release
        .get("tag_name")
        .and_then(Value::as_str)
        .map(str::trim)
        .unwrap_or_default()
        .to_string();
    let release_url = release
        .get("html_url")
        .and_then(Value::as_str)
        .map(str::trim)
        .unwrap_or_default()
        .to_string();
    let Some(asset) = manifest_asset(&release)? else {
        let checked = check_without_manifest(params, &repo, &release, &tag, &release_url)?;
        return bind_github_check(params, decorate_check(checked, tag, release_url, None));
    };
    ensure!(!tag.is_empty(), "GitHub release tag is required");
    let asset_url = required_release_text(
        asset,
        "browser_download_url",
        "GitHub release update manifest asset url",
    )?
    .to_string();
    let raw = fetch_bounded_bytes(&update_agent(), &asset_url, MAX_UPDATE_METADATA_BYTES)?;
    let manifest: Value =
        serde_json::from_slice(&raw).context("GitHub release update manifest is not valid JSON")?;
    let checked = verify_with_document(params, &manifest)?;
    write_cache(params, &manifest, &tag, &release_url)?;
    bind_github_check(params, decorate_check(checked, tag, release_url, None))
}

fn check_without_manifest(
    params: &Value,
    repo: &str,
    release: &Value,
    tag: &str,
    release_url: &str,
) -> Result<Value> {
    use crate::domain::client_state_migration::ReleaseTrack;

    let running_track = ReleaseTrack::running()?;
    let target_track = super::params::target_release_track(params)?;
    let running_version = super::params::product_version(params)?;
    let current = Version::parse(&running_version)
        .context("current client version is not valid semantic versioning")?;
    let no_newer_release = target_track == ReleaseTrack::Stable
        && stable_metadata_has_no_newer_release(repo, release, tag, release_url, &current);
    Ok(json!({
        "ok": true,
        "mode": CLIENT_UPDATE_MODE,
        "phase": if no_newer_release { "upToDate" } else { "unavailable" },
        "runningReleaseTrack": running_track.as_str(),
        "targetReleaseTrack": target_track.as_str(),
        "runningVersion": running_version,
        "updateAvailable": false,
        "errorCode": if no_newer_release { "" } else { "client_update_metadata_unavailable" },
        "availabilityEvidence": "publishedReleaseMetadata",
        "productionReady": false,
        "publicMetadataOnly": true,
        "storeCredentialsRequired": false,
    }))
}

pub(super) fn stable_metadata_has_no_newer_release(
    repo: &str,
    release: &Value,
    tag: &str,
    release_url: &str,
    current: &Version,
) -> bool {
    let published = tag
        .strip_prefix('v')
        .and_then(|tag| Version::parse(tag).ok());
    // This is a public version observation, never a verified update selection.
    // In particular the replaceable `nightly` tag cannot establish precedence.
    repo == DEFAULT_REPO
        && release.get("draft") == Some(&Value::Bool(false))
        && release.get("prerelease") == Some(&Value::Bool(false))
        && release_url == format!("https://github.com/{repo}/releases/tag/{tag}")
        && published.is_some_and(|version| {
            version.pre.is_empty() && !version.cmp_precedence(current).is_gt()
        })
}

/// `download --source github`: stream the selected artifact from its signed
/// HTTPS url into staging, enforcing the signed size and sha256 before the
/// partial file is renamed into place.
pub(super) fn download_github(params: &Value) -> Result<Value> {
    let effective = github_context_params(params)?;
    let selection = require_available_selection(&effective)?;
    let artifact = &selection.artifact;
    ensure!(
        artifact.url.starts_with("https://") || is_loopback_url(&artifact.url),
        "client update github artifact url must use https"
    );
    let root = prepare_staging_root(params)?;
    let (final_path, partial_path) = staged_artifact_paths(&root, artifact)?;
    if final_path.exists() {
        super::staging::validate_staged_regular_file(&root, &final_path)?;
        ensure!(
            super::canonical::sha256_file_exact(&final_path, artifact.size)? == artifact.sha256,
            "client update staged artifact digest does not match signed metadata"
        );
        return Ok(download_result_json(selection, true));
    }
    if partial_path.exists() {
        fs::remove_file(&partial_path)
            .context("failed to remove stale partial client update artifact")?;
    }
    let output = fs::File::create(&partial_path)
        .context("failed to create client update artifact partial file")?;
    fetch_to_file(&update_agent(), &artifact.url, output, artifact.size)?;
    finalize_partial_artifact(&root, &partial_path, &final_path, artifact)?;
    Ok(download_result_json(selection, false))
}

/// Injects the cached signed manifest and the bundled public keys into a
/// params clone so verify and apply run through the exact same signature
/// chain as the GitHub check.
pub(super) fn github_context_params(params: &Value) -> Result<Value> {
    let (mut effective, _) = super::receipt::params_with_bound_track(params)?;
    if effective.get("manifestJson").is_none() && effective.get("manifestPath").is_none() {
        let cached = read_cached_release(&effective)?
            .ok_or_else(|| anyhow!("client update github check is required before this step"))?;
        effective["manifestJson"] = cached.manifest;
    }
    inject_bundled_keys(&mut effective)?;
    Ok(effective)
}

/// `status --source github` additionally reports the last known available
/// version and cache age without touching the network.
pub(super) fn status_github(params: &Value) -> Result<Value> {
    let mut output = super::status::status(params)?;
    if let Some(cached) = read_cached_release(params)? {
        let verified = verify_with_document(params, &cached.manifest)?;
        output["source"] = json!("github");
        output["lastKnownAvailableVersion"] = json!(
            verified
                .get("availableVersion")
                .and_then(Value::as_str)
                .unwrap_or_default()
        );
        output["lastCheckAgeSeconds"] = json!(cached.age);
    }
    Ok(output)
}

fn verify_with_document(params: &Value, manifest: &Value) -> Result<Value> {
    let mut effective = params.clone();
    effective["manifestJson"] = manifest.clone();
    effective["deferReceiptBinding"] = Value::Bool(true);
    inject_bundled_keys(&mut effective)?;
    super::check::check(&effective)
}

fn bind_github_check(params: &Value, checked: Value) -> Result<Value> {
    super::receipt::bind_check_result(params, &checked)?;
    Ok(checked)
}

fn decorate_check(mut checked: Value, tag: String, url: String, age: Option<u64>) -> Value {
    checked["source"] = json!("github");
    checked["githubReleaseTag"] = json!(tag);
    checked["githubReleaseUrl"] = json!(url);
    if let Some(age) = age {
        checked["cacheAgeSeconds"] = json!(age);
    }
    checked
}

fn github_repo(params: &Value) -> Result<String> {
    let repo = json_text(params, &["repo"]).unwrap_or_else(|| DEFAULT_REPO.to_string());
    let mut parts = repo.splitn(2, '/');
    let owner = parts.next().unwrap_or_default();
    let name = parts.next().unwrap_or_default();
    validate_public_identifier(owner, "client update github repository owner")?;
    validate_public_identifier(name, "client update github repository name")?;
    Ok(format!("{owner}/{name}"))
}

fn github_api_base(params: &Value) -> String {
    json_text(params, &["githubApiBase"]).unwrap_or_else(|| DEFAULT_API_BASE.to_string())
}

fn manifest_asset(release: &Value) -> Result<Option<&Value>> {
    let assets = release
        .get("assets")
        .and_then(Value::as_array)
        .ok_or_else(|| anyhow!("GitHub release assets are required"))?;
    Ok(assets
        .iter()
        .find(|asset| asset.get("name").and_then(Value::as_str) == Some(UPDATE_MANIFEST_ASSET)))
}

fn required_release_text<'a>(value: &'a Value, field: &str, label: &str) -> Result<&'a str> {
    value
        .get(field)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|text| !text.is_empty())
        .ok_or_else(|| anyhow!("{label} is required"))
}

fn update_agent() -> Agent {
    AgentBuilder::new()
        .redirects(0)
        .timeout_connect(Duration::from_secs(5))
        .timeout_read(Duration::from_secs(15))
        .user_agent(UPDATE_UA)
        .build()
}

fn fetch_release_metadata(
    repo: &str,
    api_base: &str,
    track: crate::domain::client_state_migration::ReleaseTrack,
) -> Result<Value> {
    // GitHub's `latest` endpoint intentionally excludes prereleases. Nightly
    // therefore has one fixed, replaceable prerelease tag while Stable keeps
    // the ordinary latest-release source. The signed manifest remains the
    // authority for the exact candidate version and migration frontier.
    let selector = match track {
        crate::domain::client_state_migration::ReleaseTrack::Nightly => "tags/nightly",
        crate::domain::client_state_migration::ReleaseTrack::Stable => "latest",
    };
    let url = format!("{api_base}/repos/{repo}/releases/{selector}");
    let response = resolve_and_get(&update_agent(), &url, true)?;
    let bytes = bounded_body(
        response,
        MAX_RELEASE_METADATA_BYTES,
        "GitHub release metadata",
    )?;
    serde_json::from_slice(&bytes).context("GitHub release metadata is not valid JSON")
}

#[cfg(test)]
pub(super) fn release_metadata_path_for_test(
    track: crate::domain::client_state_migration::ReleaseTrack,
) -> String {
    let selector = match track {
        crate::domain::client_state_migration::ReleaseTrack::Nightly => "tags/nightly",
        crate::domain::client_state_migration::ReleaseTrack::Stable => "latest",
    };
    format!("/repos/{DEFAULT_REPO}/releases/{selector}")
}

fn fetch_bounded_bytes(agent: &Agent, url: &str, max_bytes: u64) -> Result<Vec<u8>> {
    let response = resolve_and_get(agent, url, false)?;
    bounded_body(response, max_bytes, "GitHub release asset")
}

fn resolve_and_get(agent: &Agent, url: &str, api: bool) -> Result<ureq::Response> {
    let mut current = url.to_string();
    for _ in 0..=MAX_REDIRECTS {
        let mut request = agent.get(&current).set("User-Agent", UPDATE_UA);
        if api {
            request = request
                .set("Accept", "application/vnd.github+json")
                .set("X-GitHub-Api-Version", "2022-11-28");
        } else {
            request = request.set("Accept", "*/*");
        }
        let response = request
            .call()
            .map_err(|error| anyhow!("client update github fetch failed: {error}"))?;
        let status = response.status();
        if matches!(status, 301 | 302 | 303 | 307 | 308) {
            let location = response
                .header("location")
                .ok_or_else(|| anyhow!("client update github redirect is missing a location"))?;
            ensure_redirect_host_allowed(location)?;
            current = location.to_string();
            continue;
        }
        ensure!(
            status == 200,
            "client update github fetch returned status {status}"
        );
        return Ok(response);
    }
    bail!("client update github fetch exceeded the redirect limit")
}

fn ensure_redirect_host_allowed(url_text: &str) -> Result<()> {
    let url = url::Url::parse(url_text).context("client update github redirect url is invalid")?;
    let host = url.host_str().unwrap_or_default().to_ascii_lowercase();
    let allowed = REDIRECT_ALLOWED_HOSTS
        .iter()
        .any(|suffix| host == *suffix || host.ends_with(&format!(".{suffix}")));
    if allowed {
        return Ok(());
    }
    ensure!(
        is_loopback_url(url_text),
        "client update github redirect target host is not allowlisted"
    );
    Ok(())
}

fn is_loopback_url(url_text: &str) -> bool {
    let Ok(url) = url::Url::parse(url_text) else {
        return false;
    };
    match url.host() {
        Some(url::Host::Ipv4(address)) => IpAddr::V4(address).is_loopback(),
        Some(url::Host::Ipv6(address)) => IpAddr::V6(address).is_loopback(),
        Some(url::Host::Domain(domain)) => domain.eq_ignore_ascii_case("localhost"),
        None => false,
    }
}

fn bounded_body(response: ureq::Response, max_bytes: u64, label: &str) -> Result<Vec<u8>> {
    let mut bytes = Vec::new();
    response
        .into_reader()
        .take(max_bytes.saturating_add(1))
        .read_to_end(&mut bytes)
        .with_context(|| format!("failed to read {label}"))?;
    ensure!(
        bytes.len() as u64 <= max_bytes,
        "{label} exceeds its size limit"
    );
    Ok(bytes)
}

fn fetch_to_file(agent: &Agent, url: &str, mut output: fs::File, expected_size: u64) -> Result<()> {
    ensure!(
        expected_size <= MAX_ARTIFACT_DOWNLOAD_BYTES,
        "client update github artifact exceeds the download limit"
    );
    let response = resolve_and_get(agent, url, false)?;
    if let Some(length) = response.header("content-length") {
        let length: u64 = length
            .parse()
            .context("client update github content-length is invalid")?;
        ensure!(
            length == expected_size,
            "client update github artifact size does not match signed metadata"
        );
    }
    let mut reader = response.into_reader().take(expected_size.saturating_add(1));
    let written = std::io::copy(&mut reader, &mut output)
        .context("failed to stream client update github artifact")?;
    ensure!(
        written == expected_size,
        "client update github artifact size does not match signed metadata"
    );
    output
        .flush()
        .context("failed to flush client update github artifact")
}

fn inject_bundled_keys(params: &mut Value) -> Result<()> {
    if params.get("publicKeysPath").is_none() && params.get("publicKeys").is_none() {
        params["publicKeys"] = bundled_public_keys_document()?;
    }
    Ok(())
}

pub(super) fn bundled_public_keys_document() -> Result<Value> {
    let raw = include_str!("../../../resources/client-update-public-keys.json");
    serde_json::from_str(raw).context("bundled client update public keys document is invalid")
}

struct CachedRelease {
    manifest: Value,
    tag: String,
    url: String,
    checked_at: u64,
    age: u64,
}

fn cache_root(params: &Value) -> Result<PathBuf> {
    let track = super::params::target_release_track(params)?;
    Ok(json_text(params, &["stateRoot", "state-root"])
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(".licoup-update-state"))
        .join("client-update-github")
        .join(track.as_str()))
}

fn cache_manifest_path(params: &Value) -> Result<PathBuf> {
    Ok(cache_root(params)?.join("manifest.json"))
}

fn write_cache(params: &Value, manifest: &Value, tag: &str, release_url: &str) -> Result<()> {
    let root = cache_root(params)?;
    fs::create_dir_all(&root).context("failed to create client update github cache root")?;
    let now = now_epoch_seconds();
    let entry = json!({
        "checkedAtEpochSeconds": now,
        "tag": tag,
        "url": release_url,
    });
    let entry_text = serde_json::to_string(&entry).context("failed to serialize github cache")?;
    let manifest_text =
        serde_json::to_string(manifest).context("failed to serialize cached manifest")?;
    let temporary = root.join(format!(".cache-{now}"));
    fs::write(&temporary, format!("{entry_text}\n{manifest_text}\n"))
        .context("failed to write client update github cache")?;
    fs::rename(&temporary, cache_manifest_path(params)?)
        .context("failed to finalize client update github cache")
}

fn read_cached_release(params: &Value) -> Result<Option<CachedRelease>> {
    let path = cache_manifest_path(params)?;
    let metadata = match fs::metadata(&path) {
        Ok(metadata) => metadata,
        Err(_) => return Ok(None),
    };
    ensure!(
        metadata.is_file() && metadata.len() <= MAX_UPDATE_METADATA_BYTES * 2,
        "client update github cache is invalid"
    );
    let raw = fs::read_to_string(&path).context("failed to read client update github cache")?;
    let (entry_line, rest) = raw
        .split_once('\n')
        .ok_or_else(|| anyhow!("client update github cache is invalid"))?;
    let manifest_line = rest
        .trim_end_matches(['\r', '\n'])
        .split('\n')
        .next()
        .unwrap_or_default();
    let entry: Value =
        serde_json::from_str(entry_line).context("client update github cache entry is invalid")?;
    let checked_at = entry
        .get("checkedAtEpochSeconds")
        .and_then(Value::as_u64)
        .ok_or_else(|| anyhow!("client update github cache timestamp is invalid"))?;
    let tag = entry
        .get("tag")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    let url = entry
        .get("url")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    ensure!(
        manifest_line.len() as u64 <= MAX_UPDATE_METADATA_BYTES,
        "client update github cached manifest exceeds the size limit"
    );
    let manifest: Value = serde_json::from_str(manifest_line)
        .context("client update github cached manifest is invalid")?;
    let now = now_epoch_seconds();
    let age = now.saturating_sub(checked_at);
    Ok(Some(CachedRelease {
        manifest,
        tag,
        url,
        checked_at,
        age,
    }))
}

fn is_fresh(checked_at: u64) -> bool {
    now_epoch_seconds().saturating_sub(checked_at) <= RELEASE_CACHE_TTL_SECONDS
}

fn now_epoch_seconds() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or(0)
}

#[cfg(test)]
pub(super) fn validate_redirect_host_allowed_for_test(url_text: &str) -> Result<()> {
    ensure_redirect_host_allowed(url_text)
}
