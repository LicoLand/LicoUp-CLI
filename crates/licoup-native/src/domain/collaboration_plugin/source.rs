use super::manifest::validate_relative_path;
use super::package::{InspectedPackage, inspect_package, write_inspected_package};
use anyhow::{Result, anyhow, ensure};
use serde_json::Value;
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::time::Duration;
use url::Url;

const MAX_GITHUB_ARCHIVE_BYTES: u64 = 64 * 1024 * 1024;
const MAX_GITHUB_ARCHIVE_ENTRIES: usize = 4096;
const MAX_GITHUB_ARCHIVE_DEPTH: usize = 32;

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct GitHubSource {
    pub normalized_url: String,
    pub(super) owner: String,
    pub(super) repository: String,
    pub ref_name: Option<String>,
    pub plugin_path: Option<PathBuf>,
}

impl GitHubSource {
    pub fn from_params(params: &Value) -> Result<Self> {
        let raw_url = text_param(params, &["githubUrl", "sourceUrl"])
            .ok_or_else(|| anyhow!("collaboration_plugin_github_url_required"))?;
        let (normalized_url, owner, repository) = normalize_github_repository(raw_url)?;
        let ref_name = text_param(params, &["ref", "gitRef"])
            .ok_or_else(|| anyhow!("collaboration_plugin_immutable_commit_required"))?;
        ensure!(
            ref_name.len() == 40
                && ref_name
                    .bytes()
                    .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f')),
            "collaboration_plugin_immutable_commit_required"
        );
        let plugin_path = text_param(params, &["pluginPath", "path"])
            .map(|value| {
                validate_relative_path(value, "collaboration_plugin_github_package_path_invalid")
            })
            .transpose()?;
        Ok(Self {
            normalized_url,
            owner,
            repository,
            ref_name: Some(ref_name.to_owned()),
            plugin_path,
        })
    }

    fn archive_url(&self) -> Result<Url> {
        let mut url = Url::parse("https://api.github.com")
            .map_err(|_| anyhow!("collaboration_plugin_github_archive_url_invalid"))?;
        {
            let mut segments = url
                .path_segments_mut()
                .map_err(|_| anyhow!("collaboration_plugin_github_archive_url_invalid"))?;
            segments.extend(["repos", &self.owner, &self.repository, "tarball"]);
            if let Some(ref_name) = &self.ref_name {
                segments.push(ref_name);
            }
        }
        Ok(url)
    }
}

pub(super) fn normalized_github_repository_url(raw_url: &str) -> Result<String> {
    normalize_github_repository(raw_url).map(|(url, _, _)| url)
}

fn normalize_github_repository(raw_url: &str) -> Result<(String, String, String)> {
    ensure!(
        raw_url == raw_url.trim(),
        "collaboration_plugin_github_url_invalid"
    );
    let parsed =
        Url::parse(raw_url).map_err(|_| anyhow!("collaboration_plugin_github_url_invalid"))?;
    ensure!(
        parsed.scheme() == "https"
            && parsed.host_str() == Some("github.com")
            && parsed.username().is_empty()
            && parsed.password().is_none()
            && parsed.port().is_none()
            && parsed.query().is_none()
            && parsed.fragment().is_none(),
        "collaboration_plugin_github_url_invalid"
    );
    let segments = parsed
        .path_segments()
        .ok_or_else(|| anyhow!("collaboration_plugin_github_url_invalid"))?
        .filter(|segment| !segment.is_empty())
        .collect::<Vec<_>>();
    ensure!(
        segments.len() == 2,
        "collaboration_plugin_github_url_invalid"
    );
    let owner = validate_github_segment(segments[0], "collaboration_plugin_github_owner_invalid")?;
    let repository = validate_github_segment(
        segments[1].trim_end_matches(".git"),
        "collaboration_plugin_github_repository_invalid",
    )?;
    let normalized_url = format!("https://github.com/{owner}/{repository}.git");
    Ok((normalized_url, owner, repository))
}

pub(super) fn stage_github_package(
    source: &GitHubSource,
    plan_root: &Path,
) -> Result<InspectedPackage> {
    licoup_foundation::platform::file_security::ensure_private_dir(plan_root)?;
    let archive_root = plan_root.join("archive");
    ensure!(
        !archive_root.exists(),
        "collaboration_plugin_plan_archive_exists"
    );
    let archive_url = source.archive_url()?;
    let response = ureq::AgentBuilder::new()
        .timeout_connect(Duration::from_secs(10))
        .timeout_read(Duration::from_secs(30))
        .timeout_write(Duration::from_secs(10))
        .redirects(3)
        .build()
        .get(archive_url.as_str())
        .set("Accept", "application/vnd.github+json")
        .set("User-Agent", "LicoUp-Optional-Collaboration")
        .call()
        .map_err(|_| anyhow!("collaboration_plugin_github_fetch_failed"))?;
    let final_url = Url::parse(response.get_url())
        .map_err(|_| anyhow!("collaboration_plugin_github_redirect_invalid"))?;
    ensure!(
        final_url.scheme() == "https"
            && matches!(
                final_url.host_str(),
                Some("api.github.com" | "github.com" | "codeload.github.com")
            ),
        "collaboration_plugin_github_redirect_invalid"
    );
    let expected_commit = source
        .ref_name
        .as_deref()
        .ok_or_else(|| anyhow!("collaboration_plugin_immutable_commit_required"))?;
    ensure!(
        final_url
            .path_segments()
            .and_then(|segments| segments.filter(|segment| !segment.is_empty()).next_back())
            == Some(expected_commit),
        "collaboration_plugin_github_resolved_commit_mismatch"
    );
    let mut archive = Vec::new();
    response
        .into_reader()
        .take(MAX_GITHUB_ARCHIVE_BYTES + 1)
        .read_to_end(&mut archive)
        .map_err(|_| anyhow!("collaboration_plugin_github_archive_read_failed"))?;
    ensure!(
        archive.len() as u64 <= MAX_GITHUB_ARCHIVE_BYTES,
        "collaboration_plugin_github_archive_too_large"
    );
    let repository_root = extract_github_archive(&archive, &archive_root)?;

    let package_root = source
        .plugin_path
        .as_ref()
        .map(|path| repository_root.join(path))
        .unwrap_or_else(|| repository_root.clone());
    let package = inspect_package(&package_root)?;
    let staged_package = plan_root.join("package");
    write_inspected_package(&package, &staged_package)?;
    fs::remove_dir_all(&archive_root)
        .map_err(|_| anyhow!("collaboration_plugin_plan_cleanup_failed"))?;
    Ok(package)
}

/// The downloaded-byte boundary, also exercised without network or installation.
fn extract_github_archive(archive: &[u8], archive_root: &Path) -> Result<PathBuf> {
    licoup_foundation::core::safe_archive::extract_tar_gz_safe(
        archive,
        archive_root,
        Some(MAX_GITHUB_ARCHIVE_BYTES),
        Some(MAX_GITHUB_ARCHIVE_ENTRIES),
        Some(MAX_GITHUB_ARCHIVE_DEPTH),
    )?;
    let entries = fs::read_dir(archive_root)?.collect::<std::io::Result<Vec<_>>>()?;
    ensure!(
        entries.len() == 1,
        "collaboration_plugin_github_archive_root_invalid"
    );
    let entry = &entries[0];
    let metadata = fs::symlink_metadata(entry.path())?;
    ensure!(
        metadata.file_type().is_dir() && !metadata.file_type().is_symlink(),
        "collaboration_plugin_github_archive_root_invalid"
    );
    Ok(entry.path())
}

fn validate_github_segment(value: &str, code: &'static str) -> Result<String> {
    ensure!(
        !value.is_empty()
            && value.len() <= 100
            && value != "."
            && value != ".."
            && value
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.')),
        code
    );
    Ok(value.to_owned())
}

fn text_param<'a>(params: &'a Value, keys: &[&str]) -> Option<&'a str> {
    keys.iter()
        .find_map(|key| params.get(*key).and_then(Value::as_str))
        .filter(|value| !value.trim().is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn downloaded_archive_extraction_checks_payload_and_gzip_completion_without_network() {
        use std::io::Write;
        let root =
            std::env::temp_dir().join(format!("lico-plugin-archive-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&root).unwrap();
        let root = root.canonicalize().unwrap();
        let name = format!("repository/{}/payload.txt", "p".repeat(150));
        let mut tar = tar::Builder::new(Vec::new());
        let mut header = tar::Header::new_gnu();
        header.set_size(7);
        header.set_mode(0o600);
        tar.append_data(&mut header, &name, &b"payload"[..])
            .unwrap();
        let mut gz = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
        gz.write_all(&tar.into_inner().unwrap()).unwrap();
        let archive = gz.finish().unwrap();
        let output = root.join("valid");
        assert_eq!(
            extract_github_archive(&archive, &output).unwrap(),
            output.join("repository")
        );
        assert_eq!(fs::read(output.join(&name)).unwrap(), b"payload");
        let refused = root.join("truncated");
        assert!(extract_github_archive(&archive[..archive.len() - 4], &refused).is_err());
        assert!(!refused.exists());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn github_source_accepts_only_a_repository_origin_and_bounded_ref() {
        let source = GitHubSource::from_params(&json!({
            "githubUrl": "https://github.com/LicoLand/collaboration-plugin",
            "ref": "0123456789abcdef0123456789abcdef01234567",
            "pluginPath": "plugins/licomesh"
        }))
        .unwrap();
        assert_eq!(
            source.normalized_url,
            "https://github.com/LicoLand/collaboration-plugin.git"
        );
        assert_eq!(
            source.ref_name.as_deref(),
            Some("0123456789abcdef0123456789abcdef01234567")
        );
        let archive_url = source.archive_url().unwrap();
        assert_eq!(archive_url.host_str(), Some("api.github.com"));
        assert!(archive_url.path().contains("/tarball/"));
    }

    #[test]
    fn github_source_rejects_credentials_queries_and_tree_urls() {
        let credential_url = ["https://", "token", "@", "github.com/LicoLand/plugin"].concat();
        for url in [
            credential_url.as_str(),
            "https://github.com/LicoLand/plugin?token=value",
            "https://github.com/LicoLand/plugin/tree/main",
            "http://github.com/LicoLand/plugin",
        ] {
            assert!(GitHubSource::from_params(&json!({"githubUrl": url})).is_err());
        }
    }

    #[test]
    fn github_source_rejects_mutable_runner_refs() {
        for git_ref in ["main", "release/v1", "HEAD", "v1.0.0"] {
            assert!(
                GitHubSource::from_params(&json!({
                    "githubUrl": "https://github.com/LicoLand/plugin",
                    "ref": git_ref
                }))
                .is_err()
            );
        }
    }
}
