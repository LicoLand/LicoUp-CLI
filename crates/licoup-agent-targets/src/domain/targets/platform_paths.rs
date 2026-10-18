use super::parameters::param_string;
use super::scan_paths::{self, HostRoots, probe_exists_for_os_with, probe_exists_with};
use licoup_foundation::platform::paths::user_home_from_env;
use serde_json::Value;
use std::env;
use std::fs;
use std::path::{Path, PathBuf};

#[cfg(test)]
pub(super) fn default_config_path(target: &str) -> Option<PathBuf> {
    default_config_path_with_params(target, &Value::Null)
}

pub(super) fn default_config_path_with_params(target: &str, params: &Value) -> Option<PathBuf> {
    let home = user_home_from_env()?;
    if target == "kimi-code"
        && let Some(root) = kimi_code_home_override(params, &home)
    {
        return Some(root.join("config.toml"));
    }
    let roots = HostRoots::from_home(&home);
    scan_paths::config_path(target, std::env::consts::OS, &roots)
}

pub(super) fn default_detection_path_with_params(target: &str, params: &Value) -> Option<PathBuf> {
    let home = user_home_from_env()?;
    if target == "kimi-code"
        && let Some(root) = kimi_code_home_override(params, &home)
    {
        return probe_exists_with(&root, &HostRoots::from_home(&home)).then_some(root);
    }
    default_detection_path_for_platform(
        target,
        std::env::consts::OS,
        &home,
        &default_app_data_dir(&home),
    )
}

pub(super) fn kimi_code_home_override(params: &Value, home: &Path) -> Option<PathBuf> {
    param_string(params, "kimiCodeHome")
        .or_else(|| env::var("KIMI_CODE_HOME").ok())
        .map(|value| expand_home_root(&value, home))
}

fn expand_home_root(value: &str, home: &Path) -> PathBuf {
    let trimmed = value.trim();
    if trimmed == "~" {
        return home.to_path_buf();
    }
    if let Some(relative) = trimmed
        .strip_prefix("~/")
        .or_else(|| trimmed.strip_prefix("~\\"))
    {
        return home.join(relative);
    }
    PathBuf::from(trimmed)
}

pub(super) fn default_app_data_dir(home: &Path) -> PathBuf {
    if let Ok(value) = env::var("APPDATA")
        && !value.trim().is_empty()
    {
        return PathBuf::from(value);
    }
    home.join("AppData").join("Roaming")
}

#[cfg(test)]
pub(super) fn default_config_path_for_platform(
    target: &str,
    platform: &str,
    home: &Path,
    app_data: &Path,
) -> Option<PathBuf> {
    let mut roots = HostRoots::from_home(home);
    if platform == "windows" {
        roots.appdata = Some(app_data.to_path_buf());
    }
    scan_paths::config_path(target, platform, &roots)
}

pub(super) fn default_detection_path_for_platform(
    target: &str,
    platform: &str,
    home: &Path,
    app_data: &Path,
) -> Option<PathBuf> {
    let mut roots = HostRoots::from_home(home);
    if platform == "windows" {
        roots.appdata = Some(app_data.to_path_buf());
    }
    default_detection_paths_for_platform(target, platform, home, app_data)
        .into_iter()
        .find(|path| probe_exists_for_os_with(path, platform, &roots))
        .or_else(|| {
            if target == "kilo-code" {
                scan_paths::extension_roots("kilo-code", platform, &roots)
                    .into_iter()
                    .find_map(existing_kilo_code_extension_dir)
            } else {
                None
            }
        })
}

pub(super) fn default_detection_paths_for_platform(
    target: &str,
    platform: &str,
    home: &Path,
    app_data: &Path,
) -> Vec<PathBuf> {
    let mut roots = HostRoots::from_home(home);
    if platform == "windows" {
        roots.appdata = Some(app_data.to_path_buf());
    }
    scan_paths::detection_paths(target, platform, &roots)
}

#[cfg(test)]
pub(super) fn kilo_code_extension_roots(home: &Path) -> Vec<PathBuf> {
    scan_paths::extension_roots(
        "kilo-code",
        std::env::consts::OS,
        &HostRoots::from_home(home),
    )
}

fn existing_kilo_code_extension_dir(root: PathBuf) -> Option<PathBuf> {
    if scan_paths::symlink_escapes_denied_location(&root) || !root.is_dir() {
        return None;
    }
    let entries = fs::read_dir(root).ok()?;
    for entry in entries.filter_map(|entry| entry.ok()) {
        let name = entry.file_name().to_string_lossy().to_ascii_lowercase();
        if name == "kilocode.kilo-code" || name.starts_with("kilocode.kilo-code-") {
            let path = entry.path();
            if !scan_paths::symlink_escapes_denied_location(&path) && path.is_dir() {
                return Some(path);
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn platform_mapping_keeps_config_and_detection_policies_separate() {
        let home = PathBuf::from("/profile");
        let app_data = PathBuf::from("/app-data");
        assert_eq!(
            default_config_path_for_platform("code", "windows", &home, &app_data),
            Some(app_data.join("Code/User/settings.json"))
        );
        assert_eq!(
            expand_home_root("~/.kimi-code", &home),
            home.join(".kimi-code")
        );
    }
}
