mod artifact;

use std::collections::BTreeSet;

use anyhow::{Result, anyhow, ensure};
use semver::Version;
use serde_json::Value;

use super::model::VerifiedArtifact;
use artifact::parse_artifact;

pub(super) fn is_sha256(value: &str) -> bool {
    artifact::is_sha256(value)
}

pub(super) struct SelectedRelease<'a> {
    pub release: &'a Value,
    pub artifact: VerifiedArtifact,
}

pub(super) fn select_highest_release<'a>(
    manifest: &'a Value,
    current_version: &str,
    target_id: &str,
    stable_track: bool,
) -> Result<Option<SelectedRelease<'a>>> {
    let current = Version::parse(current_version)
        .map_err(|_| anyhow!("current client version is not valid semantic versioning"))?;
    let releases = manifest
        .get("releases")
        .and_then(Value::as_array)
        .ok_or_else(|| anyhow!("client update manifest releases are required"))?;
    ensure!(
        !releases.is_empty(),
        "client update manifest has no releases"
    );
    let mut seen_versions = BTreeSet::new();
    let mut candidates = Vec::new();
    for (index, release) in releases.iter().enumerate() {
        let version_text = required_text(release, "version", "client update release version")?;
        let version = Version::parse(version_text).map_err(|_| {
            anyhow!("client update release version is not valid semantic versioning")
        })?;
        ensure!(
            !stable_track || version.pre.is_empty(),
            "client update stable manifest contains a prerelease version"
        );
        ensure!(
            seen_versions.insert(version_text.to_string()),
            "client update manifest contains a duplicate release version"
        );
        let minimum = Version::parse(required_text(
            release,
            "minimumSupportedVersion",
            "client update minimumSupportedVersion",
        )?)
        .map_err(|_| {
            anyhow!("client update minimumSupportedVersion is not valid semantic versioning")
        })?;
        let artifacts = release
            .get("artifacts")
            .and_then(Value::as_array)
            .ok_or_else(|| anyhow!("client update release artifacts are required"))?;
        ensure!(
            !artifacts.is_empty(),
            "client update release has no artifacts"
        );
        let mut seen_targets = BTreeSet::new();
        let mut selected_artifact = None;
        for artifact in artifacts {
            let parsed = parse_artifact(artifact)?;
            ensure!(
                seen_targets.insert(parsed.target_id.clone()),
                "client update release contains a duplicate artifact targetId"
            );
            if parsed.target_id == target_id {
                selected_artifact = Some(parsed);
            }
        }
        if current.cmp_precedence(&minimum).is_lt() || !version.cmp_precedence(&current).is_gt() {
            continue;
        }
        if let Some(artifact) = selected_artifact {
            candidates.push((version, index, artifact));
        }
    }
    candidates.sort_by(|left, right| left.0.cmp_precedence(&right.0));
    let Some((_, index, artifact)) = candidates.pop() else {
        return Ok(None);
    };
    Ok(Some(SelectedRelease {
        release: &releases[index],
        artifact,
    }))
}

fn required_text<'a>(value: &'a Value, field: &str, label: &str) -> Result<&'a str> {
    value
        .get(field)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|text| !text.is_empty())
        .ok_or_else(|| anyhow!("{label} is required"))
}
