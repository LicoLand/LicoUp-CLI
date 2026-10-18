use anyhow::{Result, anyhow, ensure};
use serde_json::Value;
use url::Url;

use crate::domain::client_update::{
    model::VerifiedArtifact,
    params::{validate_bundle_id, validate_public_identifier, validate_relative_file_name},
};

pub(super) fn parse_artifact(value: &Value) -> Result<VerifiedArtifact> {
    let target_id = required_text(value, "targetId", "client update artifact targetId")?;
    validate_public_identifier(target_id, "client update artifact targetId")?;
    let platform = required_text(value, "platform", "client update artifact platform")?;
    validate_public_identifier(platform, "client update artifact platform")?;
    let os_family = required_text(value, "osFamily", "client update artifact osFamily")?;
    validate_public_identifier(os_family, "client update artifact osFamily")?;
    let arch = required_text(value, "arch", "client update artifact arch")?;
    validate_public_identifier(arch, "client update artifact arch")?;
    let installer_strategy = required_text(
        value,
        "installerStrategy",
        "client update artifact installerStrategy",
    )?;
    validate_public_identifier(
        installer_strategy,
        "client update artifact installerStrategy",
    )?;
    let url_text = required_text(value, "url", "client update artifact url")?;
    let url = Url::parse(url_text).map_err(|_| anyhow!("client update artifact url is invalid"))?;
    ensure!(
        matches!(url.scheme(), "https" | "file")
            || (url.scheme() == "http" && is_loopback_url(&url)),
        "client update artifact url scheme is unsupported"
    );
    let file_name = validate_relative_file_name(
        required_text(value, "fileName", "client update artifact fileName")?,
        "client update artifact fileName",
    )?;
    ensure!(
        url.path_segments()
            .and_then(|mut segments| segments.next_back())
            == Some(file_name.as_str()),
        "client update artifact fileName must match its signed url"
    );
    let size = value
        .get("size")
        .and_then(Value::as_u64)
        .ok_or_else(|| anyhow!("client update artifact size is required"))?;
    ensure!(size > 0, "client update artifact size must be positive");
    let sha256 = required_text(value, "sha256", "client update artifact sha256")?;
    ensure!(
        is_sha256(sha256),
        "client update artifact sha256 is invalid"
    );
    let application_name = value
        .get("applicationName")
        .and_then(Value::as_str)
        .map(|name| validate_relative_file_name(name, "client update artifact applicationName"))
        .transpose()?;
    let bundle_id = value
        .get("bundleId")
        .and_then(Value::as_str)
        .map(validate_bundle_id)
        .transpose()?;
    if installer_strategy == "app-bundle-replacement" {
        ensure!(
            application_name.is_some() && bundle_id.is_some(),
            "macOS app-bundle update metadata requires applicationName and bundleId"
        );
    }
    Ok(VerifiedArtifact {
        target_id: target_id.to_string(),
        platform: platform.to_string(),
        os_family: os_family.to_string(),
        arch: arch.to_string(),
        installer_strategy: installer_strategy.to_string(),
        url: url_text.to_string(),
        file_name,
        size,
        sha256: sha256.to_string(),
        application_name,
        bundle_id,
    })
}

pub(super) fn is_sha256(value: &str) -> bool {
    value.len() == 71
        && value.starts_with("sha256:")
        && value[7..]
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
}

fn is_loopback_url(url: &Url) -> bool {
    use std::net::IpAddr;
    match url.host() {
        Some(url::Host::Ipv4(address)) => IpAddr::V4(address).is_loopback(),
        Some(url::Host::Ipv6(address)) => IpAddr::V6(address).is_loopback(),
        Some(url::Host::Domain(domain)) => domain.eq_ignore_ascii_case("localhost"),
        None => false,
    }
}

fn required_text<'a>(value: &'a Value, field: &str, label: &str) -> Result<&'a str> {
    value
        .get(field)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|text| !text.is_empty())
        .ok_or_else(|| anyhow!("{label} is required"))
}
