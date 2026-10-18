//! One platform capability snapshot per Hub scan generation.

use super::contract::PlatformInstallCapabilities;
use anyhow::Result;
use serde_json::Value;
use std::env;
use std::path::{Path, PathBuf};

pub fn capabilities_from_params(params: &Value) -> Result<PlatformInstallCapabilities> {
    if let Some(injected) = params.get("platformCapabilities") {
        let snapshot: PlatformInstallCapabilities = serde_json::from_value(injected.clone())?;
        return Ok(snapshot);
    }
    Ok(probe_host(params))
}

pub fn probe_host(params: &Value) -> PlatformInstallCapabilities {
    let os = params
        .get("os")
        .and_then(Value::as_str)
        .unwrap_or(env::consts::OS)
        .to_string();
    let architecture = params
        .get("arch")
        .or_else(|| params.get("architecture"))
        .and_then(Value::as_str)
        .unwrap_or(env::consts::ARCH)
        .to_string();
    let mut managers = Vec::new();
    if which("brew").is_some() {
        managers.push("homebrew".to_string());
    }
    if which("npm").is_some() {
        managers.push("npm".to_string());
    }
    if which("winget").is_some() || which("winget.exe").is_some() {
        managers.push("winget".to_string());
    }
    let scan_generation = params
        .get("scanGeneration")
        .and_then(Value::as_u64)
        .unwrap_or(1);
    PlatformInstallCapabilities {
        os,
        architecture,
        managers,
        scan_generation,
    }
}

fn which(name: &str) -> Option<PathBuf> {
    let path_var = env::var_os("PATH")?;
    env::split_paths(&path_var).find_map(|dir| {
        let candidate = dir.join(name);
        file_exists(&candidate).then_some(candidate)
    })
}

fn file_exists(path: &Path) -> bool {
    path.is_file()
}

pub fn desktop_os(os: &str) -> bool {
    matches!(os, "macos" | "linux" | "windows")
}
