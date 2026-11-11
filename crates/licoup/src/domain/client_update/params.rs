use std::path::{Component, Path, PathBuf};

use anyhow::{Result, anyhow, bail, ensure};
use serde_json::Value;

use crate::domain::client_state_migration::ReleaseTrack;

pub(super) fn product_version(params: &Value) -> Result<String> {
    ensure!(
        params.get("currentVersion").is_none() && params.get("current-version").is_none(),
        "client update currentVersion is caller-controlled and unsupported"
    );
    Ok(crate::domain::client_state_migration::running_product_version()?.to_owned())
}

pub(super) fn json_text(params: &Value, keys: &[&str]) -> Option<String> {
    keys.iter().find_map(|key| {
        params
            .get(*key)
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(ToOwned::to_owned)
    })
}

pub(super) fn target_release_track(params: &Value) -> Result<ReleaseTrack> {
    ensure!(
        params.get("channel").is_none(),
        "client update channel is unsupported"
    );
    let running = ReleaseTrack::running()?;
    let requested = json_text(params, &["targetReleaseTrack", "target-release-track"]);
    let bound = json_text(params, &["boundTargetReleaseTrack"]);
    ensure!(
        requested.is_none() || bound.is_none(),
        "client update target release track is ambiguous"
    );
    let target = match requested.or(bound).as_deref() {
        None => running,
        Some("nightly") => ReleaseTrack::Nightly,
        Some("stable") => ReleaseTrack::Stable,
        Some(_) => bail!("client update target release track is invalid"),
    };
    ensure!(
        allowed_transition(running, target),
        "client update release track transition is forbidden"
    );
    Ok(target)
}

pub(super) fn state_root(params: &Value) -> PathBuf {
    json_text(params, &["stateRoot", "state-root"])
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(".licoup-update-state"))
}

/// The data root one live apply would change.
///
/// It is required and absolute: the maintenance decision is taken about a root
/// this caller names, never about whatever directory the process happens to run
/// in, and the barrier the switch takes is written under it.
pub(super) fn live_data_root(params: &Value) -> Result<PathBuf> {
    let root = json_text(params, &["dataRoot", "data-root"])
        .map(PathBuf::from)
        .ok_or_else(|| anyhow!("client update data root is required for live apply"))?;
    ensure!(
        root.is_absolute(),
        "client update data root must be absolute"
    );
    Ok(root)
}

pub(super) const fn allowed_transition(running: ReleaseTrack, target: ReleaseTrack) -> bool {
    matches!(
        (running, target),
        (ReleaseTrack::Nightly, ReleaseTrack::Nightly)
            | (ReleaseTrack::Nightly, ReleaseTrack::Stable)
            | (ReleaseTrack::Stable, ReleaseTrack::Stable)
    )
}

pub(super) fn staging_root(params: &Value) -> Result<PathBuf> {
    if let Some(path) = json_text(params, &["stagingRoot", "staging-root", "stageRoot"]) {
        return Ok(PathBuf::from(path));
    }
    let state_root = json_text(params, &["stateRoot", "state-root"])
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(".licoup-update-staging"));
    Ok(state_root.join("client-update-staging"))
}

pub(super) fn selected_target_id(params: &Value) -> Result<String> {
    if let Some(target_id) = json_text(params, &["targetId", "target-id"]) {
        validate_public_identifier(&target_id, "client update targetId")?;
        return Ok(target_id);
    }
    default_target_id()
}

fn default_target_id() -> Result<String> {
    let target = match (std::env::consts::OS, std::env::consts::ARCH) {
        ("macos", "aarch64") => "macos-arm64",
        ("windows", "aarch64") => "windows-arm64",
        ("windows", "x86_64") => "windows-x64",
        ("linux", "aarch64") => "linux-glibc-arm64",
        ("linux", "x86_64") => "linux-glibc-x64",
        ("android", "aarch64") => "android-arm64",
        ("ios", "aarch64") => "ios-arm64",
        _ => bail!("client update target is unsupported on this build"),
    };
    Ok(target.to_string())
}

pub(super) fn validate_public_identifier(value: &str, label: &str) -> Result<()> {
    ensure!(
        !value.is_empty()
            && value.len() <= 128
            && value
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-')),
        "{label} is invalid"
    );
    Ok(())
}

pub(super) fn validate_relative_file_name(value: &str, label: &str) -> Result<String> {
    ensure!(
        !value.contains('/') && !value.contains('\\'),
        "{label} must be a single relative file name"
    );
    let path = Path::new(value);
    let mut components = path.components();
    let first = components.next();
    ensure!(
        matches!(first, Some(Component::Normal(_))) && components.next().is_none(),
        "{label} must be a single relative file name"
    );
    Ok(value.to_string())
}

pub(super) fn bool_param(params: &Value, key: &str) -> Result<bool> {
    match params.get(key) {
        None => Ok(false),
        Some(Value::Bool(value)) => Ok(*value),
        Some(Value::String(text)) => match text.trim() {
            "true" => Ok(true),
            "false" => Ok(false),
            _ => bail!("{key} must be true or false"),
        },
        Some(_) => bail!("{key} must be true or false"),
    }
}

pub(super) fn validate_bundle_id(value: &str) -> Result<String> {
    ensure!(
        value.len() <= 255
            && value.split('.').count() >= 2
            && value.split('.').all(|part| {
                !part.is_empty()
                    && part.len() <= 63
                    && part
                        .bytes()
                        .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
            }),
        "client update bundleId is invalid"
    );
    Ok(value.to_string())
}
