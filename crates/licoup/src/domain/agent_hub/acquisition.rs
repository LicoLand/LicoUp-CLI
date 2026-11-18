//! Vendor artifact acquisition for the `official-artifact` channels.
//!
//! Acquisition is the half of an Agent Hub lifecycle that turns the recipe's
//! declared official source into staged bytes. It never installs: the staged
//! file is the input of the install argv that [`super::engine`] runs, and it is
//! written only under the Hub's private staging root.
//!
//! Two rules are deliberate:
//!
//! * Every fetch is bounded, and its origin is pinned per request. The URL is
//!   built from the recipe's own `url_template`, the scheme is HTTPS and the
//!   first request must belong to the declared `origin_host`. A later redirect
//!   hop must belong to that origin too, or to a host the same recipe names in
//!   `redirect_hosts`; a hop anywhere else is refused. The redirect declaration
//!   exists because a vendor that hands its own downloads to a content network
//!   answers the first request with a `Location` on that network — GitHub
//!   release assets move to `release-assets.githubusercontent.com` — so a
//!   single origin can never describe the vendor's own publication. It widens
//!   redirect hops only: it cannot serve the first request, it is validated when
//!   the registry loads, and it is explicit per recipe, so no host becomes
//!   reachable for a channel that did not declare it. A loopback origin is
//!   accepted so the real fetch path can be exercised against a local fixture
//!   server; a bundled recipe never declares one.
//! * Integrity fails closed. A channel whose artifact declaration carries no
//!   published digest is refused with [`AcquisitionFailure::IntegrityUndeclared`]
//!   instead of staging unverified vendor bytes.
//!
//! Failures that the lifecycle reports to the caller carry a stable code in
//! [`AcquisitionFailure::code`].

use super::contract::{
    AgentRecipe, ArtifactIntegrity, ArtifactSpec, InstallChannel, PlatformInstallCapabilities,
};
use crate::platform::client_state::ClientStateStore;
use anyhow::{Result, ensure};
use licoup_foundation::platform::file_security::AtomicPrivateFile;
use sha2::{Digest, Sha256};
use std::fmt::{self, Write as _};
use std::io::{Read, Write};
use std::net::IpAddr;
use std::path::{Path, PathBuf};
use std::time::Duration;
use url::Url;

/// Upper bound of one vendor artifact. The largest declared archive is a
/// self-contained agent binary; a document larger than this is not one.
pub(crate) const MAX_ARTIFACT_BYTES: u64 = 512 * 1024 * 1024;

/// Upper bound of a published digest document.
pub(crate) const MAX_DIGEST_DOCUMENT_BYTES: u64 = 256 * 1024;

/// `sha256` is the only digest this acquirer verifies.
pub(crate) const INTEGRITY_ALGORITHM_SHA256: &str = "sha256";

const MAX_REDIRECTS: usize = 4;
const TRANSFER_BUFFER_BYTES: usize = 64 * 1024;
const MAX_ARTIFACT_NAME_CHARS: usize = 128;
const FETCH_USER_AGENT: &str = "LicoUpAgentHub/1.0";

/// Stable failure codes the lifecycle projects to the caller.
pub(crate) const ARTIFACT_SOURCE_UNDECLARED: &str = "artifact_source_undeclared";
pub(crate) const ARTIFACT_INTEGRITY_UNDECLARED: &str = "artifact_integrity_undeclared";
pub(crate) const ARTIFACT_INTEGRITY_ALGORITHM_UNSUPPORTED: &str =
    "artifact_integrity_algorithm_unsupported";
pub(crate) const ARTIFACT_INTEGRITY_MISMATCH: &str = "artifact_integrity_mismatch";
pub(crate) const ARTIFACT_DIGEST_UNAVAILABLE: &str = "artifact_digest_unavailable";
pub(crate) const ARTIFACT_URL_INCOMPLETE: &str = "artifact_url_incomplete";
pub(crate) const ARTIFACT_ORIGIN_MISMATCH: &str = "artifact_origin_mismatch";
pub(crate) const ARTIFACT_NAME_INVALID: &str = "artifact_name_invalid";
pub(crate) const ARTIFACT_ROLE_AMBIGUOUS: &str = "artifact_role_ambiguous";
pub(crate) const ARTIFACT_FETCH_FAILED: &str = "artifact_fetch_failed";
pub(crate) const ARTIFACT_SIZE_EXCEEDED: &str = "artifact_size_exceeded";
pub(crate) const ARTIFACT_STAGING_UNAVAILABLE: &str = "artifact_staging_unavailable";
pub(crate) const ARTIFACT_PLACEHOLDER_UNRESOLVED: &str = "artifact_placeholder_unresolved";
pub(crate) const INSTALL_REFERENCE_UNRESOLVED: &str = "install_reference_unresolved";
pub(crate) const INSTALL_DESTINATION_UNDECLARED: &str = "install_destination_undeclared";
pub(crate) const INSTALL_DESTINATION_UNADMITTED: &str = "install_destination_unadmitted";

/// Which argv placeholder a staged file fills.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ArtifactRole {
    /// `{artifact}` — an archive or single binary the install argv expands.
    Archive,
    /// `{script}` — a vendor installer script the install argv runs.
    Script,
}

impl ArtifactRole {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Archive => "archive",
            Self::Script => "script",
        }
    }
}

/// Bytes acquisition staged for one confirmed plan.
#[derive(Clone, Debug)]
pub(crate) struct StagedArtifact {
    pub staging_dir: PathBuf,
    pub file_path: PathBuf,
    pub sha256: String,
    pub bytes: u64,
    pub role: ArtifactRole,
    pub resumed: bool,
}

/// Whether a staged artifact was reused from an earlier acquisition.
pub(crate) fn is_regular_file(path: &Path) -> bool {
    std::fs::symlink_metadata(path)
        .map(|metadata| metadata.file_type().is_file())
        .unwrap_or(false)
}

/// A typed acquisition failure. `code` is the stable token the lifecycle
/// reports; `detail` is operator-facing and never carries a local path.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct AcquisitionFailure {
    pub code: &'static str,
    pub detail: String,
}

impl AcquisitionFailure {
    pub(crate) fn new(code: &'static str) -> Self {
        Self {
            code,
            detail: String::new(),
        }
    }

    pub(crate) fn with_detail(code: &'static str, detail: impl Into<String>) -> Self {
        Self {
            code,
            detail: detail.into(),
        }
    }
}

impl fmt::Display for AcquisitionFailure {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.detail.is_empty() {
            formatter.write_str(self.code)
        } else {
            write!(formatter, "{}: {}", self.code, self.detail)
        }
    }
}

impl std::error::Error for AcquisitionFailure {}

/// The hosts one artifact fetch may reach, in the order acquisition pins them.
///
/// The first request of a fetch must belong to the declared origin: that is the
/// vendor's own publication, and it is the only host a recipe's URL template can
/// name. A redirect hop is admitted on the same origin or on a host the recipe
/// declares in [`ArtifactSpec::redirect_hosts`], because a vendor may serve its
/// bytes through a content network of its own choosing. The declaration is
/// explicit per recipe, so a channel that does not name a host never reaches it,
/// and it is validated at registry load.
#[derive(Clone, Copy)]
pub(crate) struct ArtifactOrigin<'a> {
    origin_host: &'a str,
    redirect_hosts: &'a [String],
}

impl<'a> ArtifactOrigin<'a> {
    pub(crate) fn of(spec: &'a ArtifactSpec) -> Self {
        Self {
            origin_host: &spec.origin_host,
            redirect_hosts: &spec.redirect_hosts,
        }
    }

    /// Admits one request of a fetch. `hop` 0 is the URL the recipe built.
    fn admit(self, hop: usize, url: &str) -> Result<Url> {
        if hop == 0 {
            ensure_origin(url, self.origin_host, &[])
        } else {
            ensure_origin(url, self.origin_host, self.redirect_hosts)
        }
    }
}

/// Bounded, origin-pinned byte source. The production implementation is
/// [`VendorArtifactFetcher`]; tests substitute a deterministic port.
pub(crate) trait ArtifactFetcher: Send + Sync {
    /// Streams at most `max_bytes` of `url` into `output` and reports how many
    /// bytes were written. The first request is pinned to the declared origin
    /// and every redirect hop to that origin or to a declared redirect host.
    fn fetch(
        &self,
        url: &str,
        origin: ArtifactOrigin<'_>,
        max_bytes: u64,
        output: &mut dyn Write,
    ) -> Result<u64>;
}

/// HTTPS fetch of a vendor host with the client's own bounded reader.
pub(crate) struct VendorArtifactFetcher;

impl ArtifactFetcher for VendorArtifactFetcher {
    fn fetch(
        &self,
        url: &str,
        origin: ArtifactOrigin<'_>,
        max_bytes: u64,
        output: &mut dyn Write,
    ) -> Result<u64> {
        let agent = ureq::AgentBuilder::new()
            .redirects(0)
            .timeout_connect(Duration::from_secs(5))
            .timeout_read(Duration::from_secs(30))
            .user_agent(FETCH_USER_AGENT)
            .build();
        let mut current = url.to_string();
        for hop in 0..=MAX_REDIRECTS {
            origin.admit(hop, &current)?;
            let response = agent
                .get(&current)
                .set("User-Agent", FETCH_USER_AGENT)
                .set("Accept", "*/*")
                .call()
                .map_err(|error| {
                    AcquisitionFailure::with_detail(ARTIFACT_FETCH_FAILED, error.to_string())
                })?;
            let status = response.status();
            if matches!(status, 301 | 302 | 303 | 307 | 308) {
                let location = response
                    .header("location")
                    .ok_or_else(|| AcquisitionFailure::new(ARTIFACT_FETCH_FAILED))?;
                current = location.to_string();
                continue;
            }
            if status != 200 {
                return Err(AcquisitionFailure::with_detail(
                    ARTIFACT_FETCH_FAILED,
                    format!("vendor host returned status {status}"),
                )
                .into());
            }
            if let Some(length) = response.header("content-length") {
                let length: u64 = length
                    .parse()
                    .map_err(|_| AcquisitionFailure::new(ARTIFACT_FETCH_FAILED))?;
                if length > max_bytes {
                    return Err(AcquisitionFailure::new(ARTIFACT_SIZE_EXCEEDED).into());
                }
            }
            let mut reader = response.into_reader().take(max_bytes.saturating_add(1));
            let written = std::io::copy(&mut reader, output).map_err(|error| {
                AcquisitionFailure::with_detail(ARTIFACT_FETCH_FAILED, error.to_string())
            })?;
            if written > max_bytes {
                return Err(AcquisitionFailure::new(ARTIFACT_SIZE_EXCEEDED).into());
            }
            return Ok(written);
        }
        Err(AcquisitionFailure::with_detail(
            ARTIFACT_FETCH_FAILED,
            "vendor host redirect limit exceeded",
        )
        .into())
    }
}

/// The artifact declaration one channel needs, in the order acquisition uses it.
pub(crate) struct AcquisitionRequest<'a> {
    pub agent: &'a AgentRecipe,
    pub channel: &'a InstallChannel,
    pub capabilities: &'a PlatformInstallCapabilities,
}

/// Resolves, fetches and verifies the channel's artifact into the staging root.
pub(crate) fn stage(
    store: &ClientStateStore,
    params: &serde_json::Value,
    request: &AcquisitionRequest<'_>,
    role: ArtifactRole,
    fetcher: &dyn ArtifactFetcher,
) -> Result<StagedArtifact> {
    let spec = artifact_spec(request.channel)?;
    let version = requested_version(params)?;
    let url = artifact_url(spec, request.capabilities, &version)?;
    let file_name = artifact_file_name(&url)?;
    let expected = published_digest(spec, request.capabilities, &version, &url, fetcher)?;
    let staging_dir = staging_dir(store, params, request)?;
    let file_path = safe_staged_path(&staging_dir, &file_name)?;

    if is_regular_file(&file_path) {
        let staged = digest_of_file(&file_path)?;
        if staged.digest == expected.digest {
            return Ok(StagedArtifact {
                staging_dir,
                file_path,
                sha256: staged.digest,
                bytes: staged.bytes,
                role,
                resumed: true,
            });
        }
        std::fs::remove_file(&file_path)
            .map_err(|_| AcquisitionFailure::new(ARTIFACT_STAGING_UNAVAILABLE))?;
    }

    let mut writer = AtomicPrivateFile::create(&file_path).map_err(|error| {
        AcquisitionFailure::with_detail(ARTIFACT_STAGING_UNAVAILABLE, error.error().to_string())
    })?;
    let mut digesting = DigestingWriter::new(writer.file_mut());
    let fetched = match fetcher.fetch(
        &url,
        ArtifactOrigin::of(spec),
        MAX_ARTIFACT_BYTES,
        &mut digesting,
    ) {
        Ok(fetched) => fetched,
        Err(error) => {
            let _ = writer.discard();
            return Err(error);
        }
    };
    let actual = digesting.finish();
    if actual != expected.digest {
        let _ = writer.discard();
        return Err(AcquisitionFailure::with_detail(
            ARTIFACT_INTEGRITY_MISMATCH,
            format!(
                "{} published {} but the vendor host served {}",
                url, expected.digest, actual
            ),
        )
        .into());
    }
    writer.commit().map_err(|error| {
        AcquisitionFailure::with_detail(ARTIFACT_STAGING_UNAVAILABLE, error.error().to_string())
    })?;
    Ok(StagedArtifact {
        staging_dir,
        file_path,
        sha256: actual,
        bytes: fetched,
        role,
        resumed: false,
    })
}

/// Creates the staging directory for one channel without fetching anything.
pub(crate) fn prepare_staging_dir(
    store: &ClientStateStore,
    params: &serde_json::Value,
    request: &AcquisitionRequest<'_>,
) -> Result<PathBuf> {
    staging_dir(store, params, request)
}

/// The artifact declaration of a channel that installs from a vendor source.
pub(crate) fn artifact_spec(channel: &InstallChannel) -> Result<&ArtifactSpec, AcquisitionFailure> {
    channel
        .artifact
        .as_ref()
        .ok_or_else(|| AcquisitionFailure::new(ARTIFACT_SOURCE_UNDECLARED))
}

/// Resolves the declared URL template for this host.
///
/// The template may use `{version}`, `{vendorOs}`, `{vendorArch}` and
/// `{installer}`. A placeholder the recipe does not declare is a refusal, never
/// an empty string: an unresolved `{...}` would otherwise reach the install
/// argv as a path that does not exist.
pub(crate) fn artifact_url(
    spec: &ArtifactSpec,
    capabilities: &PlatformInstallCapabilities,
    version: &str,
) -> Result<String> {
    let resolved = resolve_template(&spec.url_template, spec, capabilities, version)?;
    // A URL template names the vendor's own publication, so it is admitted
    // against the declared origin alone: redirect hosts are for later hops.
    ArtifactOrigin::of(spec).admit(0, &resolved)?;
    Ok(resolved)
}

/// The requested vendor version, as one concrete URL path segment.
pub(crate) fn requested_version(params: &serde_json::Value) -> Result<String, AcquisitionFailure> {
    let version = params
        .get("version")
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or("latest");
    let valid = !version.is_empty()
        && version.len() <= 64
        && version
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || "._+-".contains(character));
    if !valid {
        return Err(AcquisitionFailure::new(ARTIFACT_URL_INCOMPLETE));
    }
    Ok(version.to_string())
}

/// The integrity binding the recipe declares, or the typed refusal.
///
/// Planning uses this without fetching, so an unsatisfiable plan is refused
/// before the user confirms it.
pub(crate) fn declared_integrity(
    spec: &ArtifactSpec,
) -> Result<&ArtifactIntegrity, AcquisitionFailure> {
    let integrity = spec
        .integrity
        .as_ref()
        .ok_or_else(|| AcquisitionFailure::new(ARTIFACT_INTEGRITY_UNDECLARED))?;
    if !integrity
        .algorithm
        .eq_ignore_ascii_case(INTEGRITY_ALGORITHM_SHA256)
    {
        return Err(AcquisitionFailure::new(
            ARTIFACT_INTEGRITY_ALGORITHM_UNSUPPORTED,
        ));
    }
    if integrity.digest.is_some() == integrity.digest_url_template.is_some() {
        return Err(AcquisitionFailure::new(ARTIFACT_INTEGRITY_UNDECLARED));
    }
    Ok(integrity)
}

/// The published digest document a recipe declares, resolved and origin-pinned.
///
/// Planning uses this without fetching, so an unresolvable or foreign digest
/// source is refused before the user confirms the install. `Ok(None)` means the
/// recipe pins the digest instead of publishing it.
pub(crate) fn digest_document_url(
    spec: &ArtifactSpec,
    capabilities: &PlatformInstallCapabilities,
    version: &str,
) -> Result<Option<String>> {
    let integrity = declared_integrity(spec)?;
    if integrity.digest.is_some() {
        return Ok(None);
    }
    let template = integrity
        .digest_url_template
        .as_deref()
        .ok_or_else(|| AcquisitionFailure::new(ARTIFACT_INTEGRITY_UNDECLARED))?;
    let url = resolve_template(template, spec, capabilities, version)?;
    // The digest document is addressed by the recipe, so it is pinned like the
    // artifact itself: the declared origin first, declared redirect hosts later.
    ArtifactOrigin::of(spec).admit(0, &url)?;
    Ok(Some(url))
}

/// The published digest of the artifact, or a refusal when the source declares none.
pub(crate) fn published_digest(
    spec: &ArtifactSpec,
    capabilities: &PlatformInstallCapabilities,
    version: &str,
    artifact_url: &str,
    fetcher: &dyn ArtifactFetcher,
) -> Result<PublishedDigest> {
    let integrity = declared_integrity(spec)?;
    if let Some(digest) = integrity.digest.as_deref() {
        let normalized = normalize_digest(digest)
            .ok_or_else(|| AcquisitionFailure::new(ARTIFACT_INTEGRITY_MISMATCH))?;
        return Ok(PublishedDigest { digest: normalized });
    }
    let url = digest_document_url(spec, capabilities, version)?
        .ok_or_else(|| AcquisitionFailure::new(ARTIFACT_DIGEST_UNAVAILABLE))?;
    let mut document = Vec::new();
    fetcher
        .fetch(
            &url,
            ArtifactOrigin::of(spec),
            MAX_DIGEST_DOCUMENT_BYTES,
            &mut document,
        )
        .map_err(|error| {
            AcquisitionFailure::with_detail(ARTIFACT_DIGEST_UNAVAILABLE, error.to_string())
        })?;
    let text = String::from_utf8(document)
        .map_err(|_| AcquisitionFailure::with_detail(ARTIFACT_DIGEST_UNAVAILABLE, "not text"))?;
    let file_name = artifact_file_name(artifact_url)?;
    let digest = parse_digest_document(&text, &file_name)
        .ok_or_else(|| AcquisitionFailure::new(ARTIFACT_DIGEST_UNAVAILABLE))?;
    Ok(PublishedDigest { digest })
}

/// A digest the vendor's own source published for this artifact.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct PublishedDigest {
    /// Lowercase hexadecimal sha256.
    pub digest: String,
}

fn resolve_template(
    template: &str,
    spec: &ArtifactSpec,
    capabilities: &PlatformInstallCapabilities,
    version: &str,
) -> Result<String> {
    let mut resolved = template.replace("{version}", version);
    for (placeholder, value) in [
        ("{vendorOs}", spec.vendor_os.get(&capabilities.os)),
        (
            "{vendorArch}",
            spec.vendor_arch.get(&capabilities.architecture),
        ),
        ("{installer}", spec.installer.get(&capabilities.os)),
    ] {
        // A template names the mappings it needs; a mapping the template does
        // not use is not a missing value.
        if !resolved.contains(placeholder) {
            continue;
        }
        let value = value.ok_or_else(|| AcquisitionFailure::new(ARTIFACT_URL_INCOMPLETE))?;
        resolved = resolved.replace(placeholder, value);
    }
    ensure!(
        !resolved.contains('{'),
        AcquisitionFailure::new(ARTIFACT_URL_INCOMPLETE)
    );
    Ok(resolved)
}

/// Pins one request to the hosts it may reach.
///
/// The declared origin always qualifies, with its own subdomains. A redirect hop
/// may additionally reach a host the recipe declared for that purpose; the first
/// request of a fetch passes no such list, so a declared redirect host can never
/// serve as the origin a recipe builds its URL on.
fn ensure_origin(url_text: &str, origin_host: &str, redirect_hosts: &[String]) -> Result<Url> {
    let url =
        Url::parse(url_text).map_err(|_| AcquisitionFailure::new(ARTIFACT_ORIGIN_MISMATCH))?;
    let origin = origin_host.trim().to_ascii_lowercase();
    let host = url.host_str().unwrap_or_default().to_ascii_lowercase();
    let on_origin = !origin.is_empty() && (host == origin || host.ends_with(&format!(".{origin}")));
    let declared = redirect_hosts
        .iter()
        .any(|allowed| allowed.trim().eq_ignore_ascii_case(&host));
    ensure!(
        on_origin || declared,
        AcquisitionFailure::new(ARTIFACT_ORIGIN_MISMATCH)
    );
    if origin_is_loopback(&origin) {
        return Ok(url);
    }
    ensure!(
        url.scheme() == "https",
        AcquisitionFailure::new(ARTIFACT_ORIGIN_MISMATCH)
    );
    Ok(url)
}

/// A loopback origin is the local fixture server used to exercise this path.
/// Bundled recipes never declare one, so production stays HTTPS-only.
fn origin_is_loopback(origin: &str) -> bool {
    origin.eq_ignore_ascii_case("localhost")
        || origin
            .parse::<IpAddr>()
            .map(|address| address.is_loopback())
            .unwrap_or(false)
}

pub(crate) fn artifact_file_name(url: &str) -> Result<String> {
    let parsed = Url::parse(url).map_err(|_| AcquisitionFailure::new(ARTIFACT_NAME_INVALID))?;
    let name = parsed
        .path_segments()
        .and_then(|mut segments| segments.next_back())
        .unwrap_or_default()
        .to_string();
    let valid = !name.is_empty()
        && name.len() <= MAX_ARTIFACT_NAME_CHARS
        && name != "."
        && name != ".."
        && name
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || "._+-".contains(character));
    ensure!(valid, AcquisitionFailure::new(ARTIFACT_NAME_INVALID));
    Ok(name)
}

fn staging_dir(
    store: &ClientStateStore,
    params: &serde_json::Value,
    request: &AcquisitionRequest<'_>,
) -> Result<PathBuf> {
    let root = params
        .get("stagingRoot")
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| super::ownership::hub_dir(store).join("staging"));
    let directory = root
        .join(safe_segment(&request.agent.id))
        .join(safe_segment(&request.channel.id));
    licoup_foundation::platform::file_security::ensure_private_dir(&directory).map_err(
        |error| AcquisitionFailure::with_detail(ARTIFACT_STAGING_UNAVAILABLE, error.to_string()),
    )?;
    Ok(directory)
}

fn safe_segment(value: &str) -> String {
    let sanitized = value
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() || "._+-".contains(character) {
                character
            } else {
                '-'
            }
        })
        .collect::<String>();
    if sanitized.is_empty() || sanitized == "." || sanitized == ".." {
        "unnamed".to_string()
    } else {
        sanitized
    }
}

fn safe_staged_path(directory: &Path, file_name: &str) -> Result<PathBuf> {
    ensure!(
        !file_name.is_empty() && !file_name.contains(['/', '\\']),
        AcquisitionFailure::new(ARTIFACT_NAME_INVALID)
    );
    let path = directory.join(file_name);
    let parent = path
        .parent()
        .ok_or_else(|| AcquisitionFailure::new(ARTIFACT_NAME_INVALID))?;
    ensure!(
        parent == directory,
        AcquisitionFailure::new(ARTIFACT_NAME_INVALID)
    );
    Ok(path)
}

struct StagedDigest {
    digest: String,
    bytes: u64,
}

fn digest_of_file(path: &Path) -> Result<StagedDigest> {
    let mut file = std::fs::File::open(path)
        .map_err(|_| AcquisitionFailure::new(ARTIFACT_STAGING_UNAVAILABLE))?;
    let mut digest = Sha256::new();
    let mut buffer = [0_u8; TRANSFER_BUFFER_BYTES];
    let mut total = 0_u64;
    loop {
        let read = file
            .read(&mut buffer)
            .map_err(|_| AcquisitionFailure::new(ARTIFACT_STAGING_UNAVAILABLE))?;
        if read == 0 {
            break;
        }
        total = total.saturating_add(read as u64);
        digest.update(&buffer[..read]);
    }
    Ok(StagedDigest {
        digest: hex_digest(&digest.finalize()),
        bytes: total,
    })
}

fn hex_digest(bytes: &[u8]) -> String {
    let mut output = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        let _ = write!(&mut output, "{byte:02x}");
    }
    output
}

fn normalize_digest(value: &str) -> Option<String> {
    let trimmed = value
        .trim()
        .trim_start_matches("sha256:")
        .trim()
        .to_ascii_lowercase();
    (trimmed.len() == 64
        && trimmed
            .chars()
            .all(|character| character.is_ascii_hexdigit()))
    .then_some(trimmed)
}

/// The published digest document may be a bare digest, a `sha256sum` line or a
/// multi-entry checksum file naming this artifact.
fn parse_digest_document(text: &str, file_name: &str) -> Option<String> {
    let mut digests = Vec::new();
    for line in text.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        if let Some(digest) = digest_token(trimmed) {
            digests.push((digest, trimmed.to_ascii_lowercase()));
        }
    }
    if digests.is_empty() {
        return None;
    }
    if digests.len() == 1 {
        return Some(digests[0].0.clone());
    }
    let name = file_name.to_ascii_lowercase();
    digests
        .into_iter()
        .find(|(_, line)| line.contains(&name))
        .map(|(digest, _)| digest)
}

fn digest_token(line: &str) -> Option<String> {
    if let Some(rest) = line.strip_prefix("SHA256 (")
        && let Some((_, digest)) = rest.split_once("= ")
        && let Some(digest) = normalize_digest(digest)
    {
        return Some(digest);
    }
    if let Some(first) = line.split_whitespace().next()
        && let Some(digest) = normalize_digest(first)
    {
        return Some(digest);
    }
    line.split(|character: char| !character.is_ascii_alphanumeric())
        .find_map(|token| {
            (token.len() == 64)
                .then(|| normalize_digest(token))
                .flatten()
        })
}

/// Hashes the bytes acquisition streams into the staged file.
struct DigestingWriter<'a> {
    inner: &'a mut dyn Write,
    digest: Sha256,
}

impl<'a> DigestingWriter<'a> {
    fn new(inner: &'a mut dyn Write) -> Self {
        Self {
            inner,
            digest: Sha256::new(),
        }
    }

    fn finish(self) -> String {
        hex_digest(&self.digest.finalize())
    }
}

impl Write for DigestingWriter<'_> {
    fn write(&mut self, buffer: &[u8]) -> std::io::Result<usize> {
        let written = self.inner.write(buffer)?;
        self.digest.update(&buffer[..written]);
        Ok(written)
    }

    fn flush(&mut self) -> std::io::Result<()> {
        self.inner.flush()
    }
}

/// Records what a lifecycle asked acquisition for without touching the network.
#[cfg(test)]
#[derive(Default)]
pub(crate) struct RecordingArtifactFetcher {
    requests: std::sync::Mutex<Vec<(String, String)>>,
    bodies: std::sync::Mutex<std::collections::BTreeMap<String, Vec<u8>>>,
}

#[cfg(test)]
impl RecordingArtifactFetcher {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    pub(crate) fn serve(&self, name: &str, body: Vec<u8>) {
        self.bodies
            .lock()
            .expect("recording fetcher")
            .insert(name.to_string(), body);
    }

    pub(crate) fn requests(&self) -> Vec<(String, String)> {
        self.requests.lock().expect("recording fetcher").clone()
    }
}

#[cfg(test)]
impl ArtifactFetcher for RecordingArtifactFetcher {
    fn fetch(
        &self,
        url: &str,
        origin: ArtifactOrigin<'_>,
        max_bytes: u64,
        output: &mut dyn Write,
    ) -> Result<u64> {
        origin.admit(0, url)?;
        self.requests
            .lock()
            .expect("recording fetcher")
            .push((url.to_string(), origin.origin_host.to_string()));
        let name = artifact_file_name(url)?;
        let body = self
            .bodies
            .lock()
            .expect("recording fetcher")
            .get(&name)
            .cloned()
            .ok_or_else(|| {
                AcquisitionFailure::with_detail(ARTIFACT_FETCH_FAILED, "fixture body missing")
            })?;
        ensure!(
            (body.len() as u64) <= max_bytes,
            AcquisitionFailure::new(ARTIFACT_SIZE_EXCEEDED)
        );
        output.write_all(&body)?;
        Ok(body.len() as u64)
    }
}
