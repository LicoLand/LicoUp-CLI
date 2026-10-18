//! Local package-manager metadata. Catalog refresh only; never scrape per card.

use super::contract::InstallChannel;
use super::version;
use serde_json::Value;
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ChannelVersions {
    pub installed: String,
    pub latest: String,
}

pub fn from_params(params: &Value, agent_id: &str) -> ChannelVersions {
    let entry = params
        .get("packageMetadata")
        .and_then(Value::as_object)
        .and_then(|map| map.get(agent_id));
    ChannelVersions {
        installed: entry
            .and_then(|item| item.get("installedVersion"))
            .and_then(Value::as_str)
            .map(version::concrete_display)
            .unwrap_or_default(),
        latest: entry
            .and_then(|item| item.get("latestVersion"))
            .and_then(Value::as_str)
            .map(version::concrete_display)
            .unwrap_or_default(),
    }
}

pub fn package_roots(params: &Value) -> Vec<PathBuf> {
    if let Some(items) = params.get("packageRoots").and_then(Value::as_array) {
        return items
            .iter()
            .filter_map(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(PathBuf::from)
            .collect();
    }
    vec![
        PathBuf::from("/opt/homebrew"),
        PathBuf::from("/usr/local"),
        PathBuf::from("/home/linuxbrew/.linuxbrew"),
    ]
}

pub fn lookup_local(channel: &InstallChannel, roots: &[PathBuf]) -> ChannelVersions {
    match channel.kind.as_str() {
        "homebrew" => lookup_homebrew(channel, roots),
        "npm" => lookup_npm(channel, roots),
        _ => ChannelVersions::default(),
    }
}

fn lookup_homebrew(channel: &InstallChannel, roots: &[PathBuf]) -> ChannelVersions {
    let token = formula_token(&channel.package_coordinate);
    if token.is_empty() {
        return ChannelVersions::default();
    }
    let prefer_cask = channel
        .package_form
        .as_deref()
        .is_some_and(|form| form.eq_ignore_ascii_case("cask"));
    let mut installed = String::new();
    let mut latest = String::new();
    for root in roots {
        if installed.is_empty() {
            installed = if prefer_cask {
                first_concrete([
                    newest_dir_version(&root.join("Caskroom").join(token)),
                    newest_dir_version(&root.join("Cellar").join(token)),
                ])
            } else {
                first_concrete([
                    newest_dir_version(&root.join("Cellar").join(token)),
                    newest_dir_version(&root.join("Caskroom").join(token)),
                ])
            };
        }
        if latest.is_empty() {
            latest = brew_recipe_version(root, &channel.package_coordinate, prefer_cask);
        }
        if !installed.is_empty() && !latest.is_empty() {
            break;
        }
    }
    ChannelVersions { installed, latest }
}

fn lookup_npm(channel: &InstallChannel, roots: &[PathBuf]) -> ChannelVersions {
    let coordinate = channel.package_coordinate.trim();
    if coordinate.is_empty() {
        return ChannelVersions::default();
    }
    for root in roots {
        let candidates = [
            root.join("lib")
                .join("node_modules")
                .join(coordinate)
                .join("package.json"),
            root.join("node_modules")
                .join(coordinate)
                .join("package.json"),
        ];
        for path in candidates {
            let installed = npm_package_version(&path);
            if !installed.is_empty() {
                return ChannelVersions {
                    installed,
                    latest: String::new(),
                };
            }
        }
    }
    ChannelVersions::default()
}

fn brew_recipe_version(root: &Path, coordinate: &str, prefer_cask: bool) -> String {
    let token = formula_token(coordinate);
    let owner_tap = tap_segments(coordinate);
    let mut paths = Vec::new();
    if prefer_cask {
        push_cask_paths(&mut paths, root, token, owner_tap);
        push_formula_paths(&mut paths, root, token, owner_tap);
    } else {
        push_formula_paths(&mut paths, root, token, owner_tap);
        push_cask_paths(&mut paths, root, token, owner_tap);
    }
    for path in paths {
        if let Ok(raw) = fs::read_to_string(path) {
            let version = parse_brew_ruby_version(&raw);
            if !version.is_empty() {
                return version;
            }
        }
    }
    String::new()
}

fn push_cask_paths(paths: &mut Vec<PathBuf>, root: &Path, token: &str, tap: Option<(&str, &str)>) {
    let first = first_letter(token);
    if let Some((owner, tap_name)) = tap {
        let tap_root = root
            .join("Library")
            .join("Taps")
            .join(owner)
            .join(format!("homebrew-{tap_name}"));
        paths.push(
            tap_root
                .join("Casks")
                .join(&first)
                .join(format!("{token}.rb")),
        );
        paths.push(tap_root.join("Casks").join(format!("{token}.rb")));
    }
    let core = root
        .join("Library")
        .join("Taps")
        .join("homebrew")
        .join("homebrew-cask");
    paths.push(core.join("Casks").join(&first).join(format!("{token}.rb")));
    paths.push(core.join("Casks").join(format!("{token}.rb")));
}

fn push_formula_paths(
    paths: &mut Vec<PathBuf>,
    root: &Path,
    token: &str,
    tap: Option<(&str, &str)>,
) {
    let first = first_letter(token);
    if let Some((owner, tap_name)) = tap {
        let tap_root = root
            .join("Library")
            .join("Taps")
            .join(owner)
            .join(format!("homebrew-{tap_name}"));
        paths.push(
            tap_root
                .join("Formula")
                .join(&first)
                .join(format!("{token}.rb")),
        );
        paths.push(tap_root.join("Formula").join(format!("{token}.rb")));
    }
    let core = root
        .join("Library")
        .join("Taps")
        .join("homebrew")
        .join("homebrew-core");
    paths.push(
        core.join("Formula")
            .join(&first)
            .join(format!("{token}.rb")),
    );
    paths.push(core.join("Formula").join(format!("{token}.rb")));
}

fn parse_brew_ruby_version(raw: &str) -> String {
    for line in raw.lines() {
        let line = line.trim();
        let Some(rest) = line.strip_prefix("version ") else {
            continue;
        };
        let token = rest
            .trim()
            .trim_matches(|ch| ch == '"' || ch == '\'')
            .trim();
        if token == ":latest" || version::is_policy_token(token) {
            return String::new();
        }
        let version = token.split(',').next().unwrap_or(token).trim();
        let display = version::concrete_display(version);
        if !display.is_empty() {
            return display;
        }
    }
    String::new()
}

fn newest_dir_version(dir: &Path) -> String {
    let Ok(entries) = fs::read_dir(dir) else {
        return String::new();
    };
    let mut best: Option<(semver::Version, String)> = None;
    for entry in entries.flatten() {
        let Ok(file_type) = entry.file_type() else {
            continue;
        };
        if !file_type.is_dir() {
            continue;
        }
        let name = entry.file_name().to_string_lossy().into_owned();
        let Some(parsed) = version::parse_comparable(&name) else {
            continue;
        };
        let display = version::concrete_display(&name);
        if display.is_empty() {
            continue;
        }
        if best
            .as_ref()
            .map(|(current, _)| parsed > *current)
            .unwrap_or(true)
        {
            best = Some((parsed, display));
        }
    }
    best.map(|(_, display)| display).unwrap_or_default()
}

fn npm_package_version(path: &Path) -> String {
    let Ok(raw) = fs::read_to_string(path) else {
        return String::new();
    };
    let Ok(value) = serde_json::from_str::<Value>(&raw) else {
        return String::new();
    };
    value
        .get("version")
        .and_then(Value::as_str)
        .map(version::concrete_display)
        .unwrap_or_default()
}

fn formula_token(coordinate: &str) -> &str {
    coordinate.rsplit('/').next().unwrap_or(coordinate).trim()
}

fn tap_segments(coordinate: &str) -> Option<(&str, &str)> {
    let mut parts = coordinate.split('/');
    let owner = parts.next()?.trim();
    let tap = parts.next()?.trim();
    let name = parts.next()?.trim();
    if owner.is_empty() || tap.is_empty() || name.is_empty() || parts.next().is_some() {
        return None;
    }
    Some((owner, tap))
}

fn first_letter(token: &str) -> String {
    token
        .chars()
        .next()
        .map(|ch| ch.to_ascii_lowercase().to_string())
        .unwrap_or_default()
}

fn first_concrete<const N: usize>(values: [String; N]) -> String {
    values
        .into_iter()
        .find(|value| !value.is_empty())
        .unwrap_or_default()
}
