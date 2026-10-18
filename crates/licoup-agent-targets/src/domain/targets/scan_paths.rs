//! Allowlisted Agent discovery paths. The TOML manifest is the only automatic
//! search space; PATH, personal library roots, and network volumes are never
//! walked.

use licoup_foundation::platform::paths::{
    portable_data_dir, strip_macos_data_volume, user_home_from_env,
};
use serde::Deserialize;
use std::collections::{BTreeSet, VecDeque};
use std::ffi::OsString;
use std::fs;
use std::path::{Component, Path, PathBuf};
use std::sync::OnceLock;

const MANIFEST: &str = include_str!("../../../resources/agent-scan-paths.toml");
pub const SCHEMA_VERSION: &str = "licoup.agent-scan-paths.v2";

#[derive(Clone, Debug, Default)]
pub struct HostRoots {
    pub home: Option<PathBuf>,
    pub appdata: Option<PathBuf>,
    pub local_appdata: Option<PathBuf>,
    pub xdg_config: Option<PathBuf>,
    pub xdg_data: Option<PathBuf>,
    pub program_data: Option<PathBuf>,
    pub program_files: Option<PathBuf>,
    pub program_files_x86: Option<PathBuf>,
    pub portable: Option<PathBuf>,
}

impl HostRoots {
    pub fn from_environment() -> Self {
        let home = user_home_from_env();
        Self {
            appdata: env_path("APPDATA").or_else(|| {
                home.as_ref().map(|home| {
                    if cfg!(windows) {
                        home.join("AppData").join("Roaming")
                    } else {
                        home.join(".config")
                    }
                })
            }),
            local_appdata: env_path("LOCALAPPDATA").or_else(|| {
                home.as_ref().map(|home| {
                    if cfg!(windows) {
                        home.join("AppData").join("Local")
                    } else {
                        home.join(".local").join("share")
                    }
                })
            }),
            xdg_config: env_path("XDG_CONFIG_HOME")
                .or_else(|| home.as_ref().map(|home| home.join(".config"))),
            xdg_data: env_path("XDG_DATA_HOME")
                .or_else(|| home.as_ref().map(|home| home.join(".local").join("share"))),
            program_data: env_path("ProgramData"),
            program_files: env_path("ProgramFiles"),
            program_files_x86: env_path("ProgramFiles(x86)"),
            portable: portable_data_dir().ok(),
            home,
        }
    }

    pub fn from_home(home: &Path) -> Self {
        Self {
            home: Some(home.to_path_buf()),
            appdata: Some(if cfg!(windows) {
                home.join("AppData").join("Roaming")
            } else {
                home.join(".config")
            }),
            local_appdata: Some(if cfg!(windows) {
                home.join("AppData").join("Local")
            } else {
                home.join(".local").join("share")
            }),
            xdg_config: Some(home.join(".config")),
            xdg_data: Some(home.join(".local").join("share")),
            program_data: None,
            program_files: None,
            program_files_x86: None,
            portable: portable_data_dir().ok(),
        }
    }
}

#[derive(Clone, Debug, Deserialize)]
struct Manifest {
    schema_version: String,
    deny: DenySpec,
    #[serde(default)]
    binaries: Vec<BinaryGroup>,
    #[serde(default)]
    agents: Vec<AgentPaths>,
}

#[derive(Clone, Debug, Deserialize)]
struct DenySpec {
    #[serde(default)]
    network_prefixes: Vec<String>,
    #[serde(default)]
    home_roots: Vec<String>,
    #[serde(default)]
    media_library_extensions: Vec<String>,
    #[serde(default)]
    other_app_home_prefixes: Vec<String>,
}

#[derive(Clone, Debug, Deserialize)]
struct BinaryGroup {
    #[serde(default)]
    oses: Vec<String>,
    #[serde(default)]
    dirs: Vec<String>,
}

#[derive(Clone, Debug, Deserialize)]
struct AgentPaths {
    id: String,
    #[serde(default)]
    binaries: Vec<OsPath>,
    #[serde(default)]
    apps: Vec<OsPath>,
    #[serde(default)]
    extension_roots: Vec<OsPath>,
    #[serde(default)]
    config: Vec<OsPath>,
    #[serde(default)]
    detection: Vec<OsPath>,
    #[serde(default)]
    history: Vec<HistoryPath>,
}

#[derive(Clone, Debug, Deserialize)]
struct OsPath {
    path: String,
    #[serde(default)]
    oses: Vec<String>,
}

#[derive(Clone, Debug, Deserialize)]
struct HistoryPath {
    path: String,
    #[serde(default)]
    kind: String,
}

#[derive(Clone, Debug)]
pub struct HistoryScanRoot {
    pub path: PathBuf,
    pub kind: String,
}

fn manifest() -> &'static Manifest {
    static PARSED: OnceLock<Manifest> = OnceLock::new();
    PARSED.get_or_init(|| {
        let parsed: Manifest = toml::from_str(MANIFEST).expect("agent-scan-paths.toml");
        assert_eq!(parsed.schema_version, SCHEMA_VERSION);
        parsed
    })
}

fn env_path(name: &str) -> Option<PathBuf> {
    std::env::var_os(name)
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
}

fn os_matches(oses: &[String], os: &str) -> bool {
    oses.is_empty() || oses.iter().any(|value| value == os)
}

/// The root tokens a path template may begin with, in this manifest's own
/// vocabulary. A template that starts with none of them has no host root, so it
/// names nothing this owner can expand or admit.
pub const PATH_TEMPLATE_ROOTS: &[&str] = &[
    "home",
    "appdata",
    "local_appdata",
    "xdg_config",
    "xdg_data",
    "program_data",
    "program_files",
    "program_files_x86",
    "portable",
];

/// Expands one path template against the host roots, in this manifest's own
/// vocabulary. A caller that needs the same roots the scanner uses — an install
/// destination that must be a location discovery looks in — expands it here
/// rather than inventing a second path vocabulary.
pub fn expand_path_template(template: &str, roots: &HostRoots) -> Option<PathBuf> {
    expand_template(template, roots)
}

/// Whether discovery admits `path` on `os`: a location whose own scan prefixes
/// cover it and that the deny list does not exclude.
///
/// An install destination the Hub cannot state here is a destination discovery
/// may never look in, which would make the install invisible to the Agent that
/// scans for it.
pub fn install_destination_admitted(path: &Path, os: &str, roots: &HostRoots) -> bool {
    admitted_scan_path_for_os_with(path, os, roots)
}

fn expand_template(template: &str, roots: &HostRoots) -> Option<PathBuf> {
    let mut remaining = template;
    let mut path = PathBuf::new();
    if let Some(token) = remaining.strip_prefix('{') {
        let (name, rest) = token.split_once('}')?;
        let base = match name {
            "home" => roots.home.as_ref()?,
            "appdata" => roots.appdata.as_ref()?,
            "local_appdata" => roots.local_appdata.as_ref()?,
            "xdg_config" => roots.xdg_config.as_ref()?,
            "xdg_data" => roots.xdg_data.as_ref()?,
            "program_data" => roots.program_data.as_ref()?,
            "program_files" => roots.program_files.as_ref()?,
            "program_files_x86" => roots.program_files_x86.as_ref()?,
            "portable" => roots.portable.as_ref()?,
            _ => return None,
        };
        path.push(base);
        remaining = rest.trim_start_matches(['/', '\\']);
    }
    if !remaining.is_empty() {
        path.push(remaining);
    }
    Some(path)
}

fn expand_all(templates: &[String], roots: &HostRoots) -> Vec<PathBuf> {
    templates
        .iter()
        .filter_map(|template| expand_template(template, roots))
        .collect()
}

fn expand_os_paths(entries: &[OsPath], os: &str, roots: &HostRoots) -> Vec<PathBuf> {
    entries
        .iter()
        .filter(|entry| os_matches(&entry.oses, os))
        .filter_map(|entry| expand_template(&entry.path, roots))
        .collect()
}

fn nvm_default_bin_dirs(home: &Path) -> Vec<PathBuf> {
    nvm_default_version(home)
        .map(|version| {
            home.join(".nvm")
                .join("versions")
                .join("node")
                .join(version)
                .join("bin")
        })
        .into_iter()
        .collect()
}

fn nvm_default_version(home: &Path) -> Option<String> {
    let nvm = home.join(".nvm");
    let mut seen = BTreeSet::new();
    let mut current = String::from("default");
    for _ in 0..8 {
        if !seen.insert(current.clone()) {
            return None;
        }
        if let Some(version) = nvm_node_version_dir_name(&current) {
            return Some(version);
        }
        if current.contains("..") || Path::new(&current).is_absolute() {
            return None;
        }
        let alias = nvm.join("alias").join(&current);
        if denied(&alias, Some(home)) {
            return None;
        }
        let text = fs::read_to_string(alias).ok()?;
        current = text.trim().to_string();
        if current.is_empty() || current.len() > 64 {
            return None;
        }
    }
    None
}

fn nvm_node_version_dir_name(value: &str) -> Option<String> {
    let trimmed = value.trim().trim_start_matches('v');
    let mut parts = trimmed.split('.');
    let major = parts.next()?;
    let minor = parts.next()?;
    if major.is_empty()
        || !major.chars().all(|item| item.is_ascii_digit())
        || minor.is_empty()
        || !minor.chars().next()?.is_ascii_digit()
    {
        return None;
    }
    Some(format!("v{trimmed}"))
}

fn agent<'a>(id: &str) -> Option<&'a AgentPaths> {
    manifest().agents.iter().find(|agent| agent.id == id)
}

pub fn binary_dirs(os: &str, roots: &HostRoots) -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    for group in &manifest().binaries {
        if !os_matches(&group.oses, os) {
            continue;
        }
        dirs.extend(expand_all(&group.dirs, roots));
    }
    for agent in &manifest().agents {
        dirs.extend(expand_os_paths(&agent.binaries, os, roots));
    }
    if let Some(home) = roots.home.as_deref() {
        dirs.extend(nvm_default_bin_dirs(home));
    }
    dedupe(dirs)
        .into_iter()
        .filter(|path| !denied(path, roots.home.as_deref()))
        .collect()
}

pub fn agent_binary_dirs(agent_id: &str, os: &str, roots: &HostRoots) -> Vec<PathBuf> {
    agent(agent_id)
        .map(|agent| expand_os_paths(&agent.binaries, os, roots))
        .unwrap_or_default()
}

pub fn config_path(agent_id: &str, os: &str, roots: &HostRoots) -> Option<PathBuf> {
    agent(agent_id).and_then(|agent| {
        agent.config.iter().find_map(|entry| {
            os_matches(&entry.oses, os)
                .then(|| expand_template(&entry.path, roots))
                .flatten()
        })
    })
}

pub fn detection_paths(agent_id: &str, os: &str, roots: &HostRoots) -> Vec<PathBuf> {
    let Some(agent) = agent(agent_id) else {
        return Vec::new();
    };
    agent
        .detection
        .iter()
        .filter(|entry| os_matches(&entry.oses, os))
        .filter_map(|entry| expand_template(&entry.path, roots))
        .collect()
}

pub fn history_roots(agent_id: &str, roots: &HostRoots) -> Vec<HistoryScanRoot> {
    let Some(agent) = agent(agent_id) else {
        return Vec::new();
    };
    agent
        .history
        .iter()
        .filter_map(|entry| {
            Some(HistoryScanRoot {
                path: expand_template(&entry.path, roots)?,
                kind: entry.kind.clone(),
            })
        })
        .collect()
}

pub fn extension_roots(agent_id: &str, os: &str, roots: &HostRoots) -> Vec<PathBuf> {
    agent(agent_id)
        .map(|agent| expand_os_paths(&agent.extension_roots, os, roots))
        .unwrap_or_default()
}

pub fn app_executables(agent_id: &str, os: &str, roots: &HostRoots) -> Vec<PathBuf> {
    agent(agent_id)
        .map(|agent| expand_os_paths(&agent.apps, os, roots))
        .unwrap_or_default()
}

pub fn allow_prefixes(os: &str, roots: &HostRoots) -> Vec<PathBuf> {
    let mut prefixes = Vec::new();
    for group in &manifest().binaries {
        if os_matches(&group.oses, os) {
            prefixes.extend(expand_all(&group.dirs, roots));
        }
    }
    for agent in &manifest().agents {
        prefixes.extend(expand_os_paths(&agent.binaries, os, roots));
        prefixes.extend(expand_os_paths(&agent.apps, os, roots));
        prefixes.extend(expand_os_paths(&agent.extension_roots, os, roots));
        for entry in agent
            .config
            .iter()
            .chain(agent.detection.iter())
            .filter(|entry| os_matches(&entry.oses, os))
        {
            if let Some(path) = expand_template(&entry.path, roots) {
                prefixes.push(path);
            }
        }
        for entry in &agent.history {
            if let Some(path) = expand_template(&entry.path, roots) {
                prefixes.push(path);
            }
        }
    }
    if let Some(home) = roots.home.as_deref() {
        prefixes.extend(nvm_default_bin_dirs(home));
    }
    dedupe(prefixes)
        .into_iter()
        .filter(|path| !denied(path, roots.home.as_deref()))
        .collect()
}

#[cfg(test)]
pub fn admitted_scan_path_with(path: &Path, roots: &HostRoots) -> bool {
    admitted_scan_path_for_os_with(path, std::env::consts::OS, roots)
}

fn admitted_scan_path_for_os_with(path: &Path, os: &str, roots: &HostRoots) -> bool {
    let normalized = lexical(path);
    if denied(&normalized, roots.home.as_deref()) {
        return false;
    }
    allow_prefixes(os, roots).iter().any(|prefix| {
        let prefix = lexical(prefix);
        normalized == prefix || normalized.starts_with(&prefix)
    })
}

pub fn probe_exists(path: &Path) -> bool {
    automatic_probe_admitted(path) && path.exists()
}

/// A file chosen explicitly by the user is not constrained to automatic scan
/// allowlists. Symlinks are rejected so the selected identity cannot change
/// after validation.
pub fn explicit_file_exists(path: &Path) -> bool {
    fs::symlink_metadata(path)
        .is_ok_and(|metadata| metadata.file_type().is_file() && !metadata.file_type().is_symlink())
}

pub fn probe_exists_with(path: &Path, roots: &HostRoots) -> bool {
    automatic_probe_admitted_with(path, roots) && path.exists()
}

pub fn probe_exists_for_os_with(path: &Path, os: &str, roots: &HostRoots) -> bool {
    automatic_probe_admitted_for_os_with(path, os, roots) && path.exists()
}

pub fn probe_is_file(path: &Path) -> bool {
    automatic_probe_admitted(path) && path.is_file()
}

#[cfg(test)]
pub fn probe_is_dir(path: &Path) -> bool {
    automatic_probe_admitted(path) && path.is_dir()
}

/// Exact files constructed under a caller-supplied catalog home may be stated
/// when they are not denied personal or network locations. This is not unused
/// Agent discovery: the caller already named the home and the file.
pub fn probe_exists_under_home(path: &Path, home: &Path) -> bool {
    let normalized = lexical(path);
    let home = lexical(home);
    let real_home = user_home_from_env();
    if catalog_home_denied(&home, real_home.as_deref())
        || denied(&normalized, real_home.as_deref())
        || denied(&normalized, Some(&home))
    {
        return false;
    }
    if symlink_escapes_denied_location(path)
        || symlink_escapes_denied_location_with(path, &HostRoots::from_home(&home))
    {
        return false;
    }
    if !(normalized == home || normalized.starts_with(&home)) {
        return probe_exists(path);
    }
    path.exists()
}

fn catalog_home_denied(home: &Path, real_home: Option<&Path>) -> bool {
    if denied(home, None) {
        return true;
    }
    if real_home.is_some_and(|real_home| lexical(home) != lexical(real_home))
        && denied(home, real_home)
    {
        return true;
    }
    home.file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| manifest().deny.home_roots.iter().any(|root| root == name))
}

/// Allowlisted paths may be stated. They may never be executed during
/// unused-agent discovery. A new Agent recipe cannot opt a binary into
/// launch-time execution by listing it in the manifest.
pub fn automatic_agent_execution_admitted() -> bool {
    false
}

/// User-triggered capability or model-catalog probes may execute a discovered
/// binary only when the caller opted in and the path is not a denied personal
/// or network location.
pub fn discovered_agent_may_execute(path: &Path, execution_requested: bool) -> bool {
    execution_requested
        && !automatic_agent_execution_admitted()
        && !denied(
            path,
            licoup_foundation::platform::paths::user_home_from_env().as_deref(),
        )
}

fn automatic_probe_admitted(path: &Path) -> bool {
    automatic_probe_admitted_with(path, &HostRoots::from_environment())
}

fn automatic_probe_admitted_with(path: &Path, roots: &HostRoots) -> bool {
    automatic_probe_admitted_for_os_with(path, std::env::consts::OS, roots)
}

fn automatic_probe_admitted_for_os_with(path: &Path, os: &str, roots: &HostRoots) -> bool {
    admitted_scan_path_for_os_with(path, os, roots)
        && !is_other_app_container_with(path, roots)
        && !symlink_escapes_denied_location_with(path, roots)
}

/// Resolve links one path component at a time. Inspecting only the leaf misses
/// a linked parent and would let the later filesystem probe follow that parent
/// into a denied location.
pub fn symlink_escapes_denied_location(path: &Path) -> bool {
    symlink_escapes_denied_location_with(path, &HostRoots::from_environment())
}

fn symlink_escapes_denied_location_with(path: &Path, roots: &HostRoots) -> bool {
    let mut pending = components_of(path);
    let mut resolved = PathBuf::new();
    let mut seen_links = BTreeSet::new();

    while let Some(component) = pending.pop_front() {
        resolved.push(component);
        let metadata = match fs::symlink_metadata(&resolved) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return false,
            Err(_) => return true,
        };
        if !metadata.file_type().is_symlink() {
            continue;
        }
        if seen_links.len() >= 40 || !seen_links.insert(lexical(&resolved)) {
            return true;
        }

        let Ok(target) = fs::read_link(&resolved) else {
            return true;
        };
        let target = if target.is_absolute() {
            lexical(&target)
        } else {
            lexical(
                &resolved
                    .parent()
                    .map(|parent| parent.join(&target))
                    .unwrap_or(target),
            )
        };
        if denied(&target, roots.home.as_deref()) || is_other_app_container_with(&target, roots) {
            return true;
        }

        let mut expanded = components_of(&target);
        expanded.extend(pending);
        pending = expanded;
        resolved.clear();
    }
    false
}

fn components_of(path: &Path) -> VecDeque<OsString> {
    lexical(path)
        .components()
        .map(|component| component.as_os_str().to_os_string())
        .collect()
}

/// Other-app containers listed in the manifest. Lexical; does not stat.
/// Automatic unused-agent probes skip these. Opening that Agent's conversation
/// interface may still read its store.
pub fn is_other_app_container(path: &Path) -> bool {
    is_other_app_container_with(path, &HostRoots::from_environment())
}

pub fn is_other_app_container_under_home(path: &Path, home: &Path) -> bool {
    is_other_app_container_with(path, &HostRoots::from_home(home))
}

/// Named files inside another app's container. Unused-agent scans never stat
/// these. Opening that Agent's conversation interface may `exists` and read them.
pub fn selected_agent_named_store_exists(path: &Path, catalog_home: &Path, selected: bool) -> bool {
    let real_home = user_home_from_env();
    if denied(path, Some(catalog_home)) || denied(path, real_home.as_deref()) {
        return false;
    }
    if !selected && is_other_app_container_under_home(path, catalog_home) {
        return false;
    }
    path.exists()
}

fn is_other_app_container_with(path: &Path, roots: &HostRoots) -> bool {
    let Some(home) = roots
        .home
        .as_deref()
        .map(|home| strip_macos_data_volume(&lexical(home)))
    else {
        return false;
    };
    let normalized = strip_macos_data_volume(&lexical(path));
    manifest()
        .deny
        .other_app_home_prefixes
        .iter()
        .any(|relative| {
            let root = home.join(relative);
            normalized == root || normalized.starts_with(&root)
        })
}

pub fn denied(path: &Path, home: Option<&Path>) -> bool {
    if is_windows_network_path(path) {
        return true;
    }
    let normalized = strip_macos_data_volume(&lexical(path));
    let deny = &manifest().deny;
    if deny.network_prefixes.iter().any(|prefix| {
        let prefix = Path::new(prefix);
        normalized == prefix || normalized.starts_with(prefix)
    }) {
        return true;
    }
    if is_media_library(&normalized, &deny.media_library_extensions) {
        return true;
    }
    let Some(home) = home.map(|home| strip_macos_data_volume(&lexical(home))) else {
        return false;
    };
    if home.starts_with(&normalized) {
        return true;
    }
    deny.home_roots.iter().any(|name| {
        let root = home.join(name);
        normalized == root || normalized.starts_with(&root)
    })
}

fn is_windows_network_path(path: &Path) -> bool {
    let raw = path.as_os_str().to_string_lossy();
    let normalized = raw.replace('\\', "/");
    let Some(root) = normalized.strip_prefix("//") else {
        return false;
    };
    root.strip_prefix("?/")
        .or_else(|| root.strip_prefix("./"))
        .is_none_or(|verbatim| {
            verbatim
                .get(..4)
                .is_some_and(|prefix| prefix.eq_ignore_ascii_case("UNC/"))
        })
}

fn is_media_library(path: &Path, extensions: &[String]) -> bool {
    path.extension()
        .and_then(|value| value.to_str())
        .map(str::to_ascii_lowercase)
        .is_some_and(|extension| extensions.iter().any(|item| item == &extension))
}

fn lexical(path: &Path) -> PathBuf {
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                normalized.pop();
            }
            other => normalized.push(other.as_os_str()),
        }
    }
    if normalized.as_os_str().is_empty() {
        path.to_path_buf()
    } else {
        normalized
    }
}

fn dedupe(paths: Vec<PathBuf>) -> Vec<PathBuf> {
    let mut seen = BTreeSet::<String>::new();
    let mut out = Vec::new();
    for path in paths {
        let key = path.to_string_lossy().to_ascii_lowercase();
        if seen.insert(key) {
            out.push(path);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn posix_absolute(parts: &[&str]) -> PathBuf {
        PathBuf::from(format!("/{}", parts.join("/")))
    }

    fn fixture_roots() -> HostRoots {
        let home = PathBuf::from("/profile");
        HostRoots {
            home: Some(home.clone()),
            appdata: Some(PathBuf::from("/app-data")),
            local_appdata: Some(PathBuf::from("/local-app-data")),
            xdg_config: Some(home.join(".config")),
            xdg_data: Some(home.join(".local/share")),
            program_data: Some(PathBuf::from("/program-data")),
            program_files: Some(PathBuf::from("/program-files")),
            program_files_x86: None,
            portable: Some(PathBuf::from("/portable")),
        }
    }

    fn unc_path(host: &str, rest: &str) -> PathBuf {
        PathBuf::from(format!(r"\\{host}\{rest}"))
    }

    fn extended_drive_path(segment: &str) -> PathBuf {
        PathBuf::from(format!(r"\\?\C:{segment}"))
    }

    #[test]
    fn manifest_parses_with_the_published_schema() {
        assert_eq!(manifest().schema_version, SCHEMA_VERSION);
        assert!(!manifest().agents.is_empty());
        assert!(!manifest().binaries.is_empty());
    }

    #[test]
    fn personal_and_network_locations_are_denied_without_stating() {
        let home = PathBuf::from("/profile");
        for denied_path in [
            PathBuf::from("/profile/Downloads/bin"),
            PathBuf::from("/profile/Desktop/tools"),
            PathBuf::from("/profile/Documents/scripts"),
            PathBuf::from("/profile/Pictures"),
            PathBuf::from("/profile/Music/library"),
            PathBuf::from("/profile/Pictures/Personal.photoslibrary"),
            posix_absolute(&["Volumes", "team-share", "bin"]),
            posix_absolute(&["System", "Volumes", "Data", "profile", "Desktop", "tools"]),
            posix_absolute(&["System", "Volumes", "Data", "profile", "Pictures"]),
            posix_absolute(&["System", "Volumes", "Data", "profile", "Music", "library"]),
        ] {
            assert!(denied(&denied_path, Some(&home)));
        }
        assert!(denied(
            &PathBuf::from("/profile/Desktop/tools"),
            Some(&posix_absolute(&["System", "Volumes", "Data", "profile"])),
        ));
        assert!(denied(
            &Path::new(&unc_path("server", r"redirected-profile\AppData")),
            Some(&home),
        ));
        assert!(denied(
            Path::new(r"\\?\UNC\server\redirected-profile\AppData"),
            Some(&home),
        ));
        assert!(!denied(
            &Path::new(&extended_drive_path(r"\Profile\lico\AppData")),
            Some(&home),
        ));
    }

    #[test]
    fn agent_store_locations_are_admitted() {
        let roots = fixture_roots();
        let cursor = PathBuf::from("/profile/.cursor/chats");
        let homebrew = PathBuf::from("/opt/homebrew/bin");
        assert!(!denied(&cursor, roots.home.as_deref()));
        assert!(!denied(&homebrew, roots.home.as_deref()));
        assert!(admitted_scan_path_with(&cursor, &roots));
        assert!(!probe_is_dir(&posix_absolute(&["Volumes", "team-share"])));
        assert!(
            binary_dirs("macos", &roots).contains(&PathBuf::from("/opt/homebrew/bin"))
                || binary_dirs("linux", &roots).contains(&PathBuf::from("/usr/bin"))
        );
        assert!(denied(
            &PathBuf::from("/profile/.local/bin/../../Desktop/tool"),
            roots.home.as_deref()
        ));
        assert!(!admitted_scan_path_with(
            &PathBuf::from("/opt/homebrew/bin/../Desktop/tool"),
            &roots
        ));
    }

    #[test]
    fn history_roots_come_from_the_manifest() {
        let roots = HostRoots::from_home(&PathBuf::from("synthetic-home"));
        let cursor = history_roots("cursor", &roots);
        assert!(cursor.iter().any(|root| {
            root.kind == "cursor-cli-chats"
                && root.path == Path::new("synthetic-home/.cursor/chats")
        }));
        let xdg = history_roots("cursor", &roots);
        assert!(xdg.iter().any(|root| {
            root.path == Path::new("synthetic-home/.config/Cursor/User/workspaceStorage")
        }));
    }

    #[test]
    fn windows_package_manager_dirs_are_listed() {
        let roots = fixture_roots();
        let dirs = binary_dirs("windows", &roots);
        assert!(dirs.contains(&PathBuf::from("/local-app-data/Microsoft/WindowsApps")));
        assert!(dirs.contains(&PathBuf::from("/profile/scoop/shims")));
        assert!(dirs.contains(&PathBuf::from("/program-data/chocolatey/bin")));
    }

    #[test]
    fn agent_specific_paths_apply_os_filters() {
        let roots = fixture_roots();
        let codex_macos = agent_binary_dirs("codex", "macos", &roots);
        assert_eq!(
            codex_macos,
            vec![
                PathBuf::from("/Applications/ChatGPT.app/Contents/Resources/codex-cli/bin"),
                PathBuf::from("/profile/Applications/ChatGPT.app/Contents/Resources/codex-cli/bin"),
            ]
        );
        assert!(agent_binary_dirs("codex", "linux", &roots).is_empty());
        assert!(
            binary_dirs("macos", &roots)
                .iter()
                .any(|path| path.to_string_lossy().contains("Cursor.app"))
        );
        assert!(
            !binary_dirs("linux", &roots)
                .iter()
                .any(|path| path.to_string_lossy().contains("Cursor.app"))
        );
        assert!(!app_executables("cursor", "macos", &roots).is_empty());
        assert!(app_executables("cursor", "linux", &roots).is_empty());
    }

    #[cfg(unix)]
    #[test]
    fn automatic_probe_rejects_symlink_chain_in_a_parent_component() {
        use std::os::unix::fs::symlink;

        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default();
        let home = std::env::temp_dir().join(format!(
            "lico-scan-symlink-{}-{}",
            stamp.as_secs(),
            stamp.subsec_nanos()
        ));
        let denied_target = home.join("Documents").join("claude");
        let chained_link = home.join("catalog-link");
        let allowlisted_link = home.join(".claude");
        let nested_probe = allowlisted_link.join("settings.json");
        fs::create_dir_all(&denied_target).unwrap();
        fs::write(denied_target.join("settings.json"), "{}").unwrap();
        symlink(&denied_target, &chained_link).unwrap();
        symlink(&chained_link, &allowlisted_link).unwrap();

        let roots = HostRoots::from_home(&home);
        assert!(admitted_scan_path_with(&nested_probe, &roots));
        assert!(symlink_escapes_denied_location_with(&nested_probe, &roots));
        assert!(!probe_exists_with(&nested_probe, &roots));

        let _ = fs::remove_dir_all(&home);
    }

    #[test]
    fn nvm_default_alias_adds_that_node_bin_dir() {
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default();
        let home = std::env::temp_dir().join(format!(
            "lico-scan-nvm-{}-{}",
            stamp.as_secs(),
            stamp.subsec_nanos()
        ));
        fs::create_dir_all(home.join(".nvm/alias/lts")).unwrap();
        fs::write(home.join(".nvm/alias/default"), "lts/krypton\n").unwrap();
        fs::write(home.join(".nvm/alias/lts/krypton"), "22.14.0\n").unwrap();
        let roots = HostRoots::from_home(&home);
        let expected = home.join(".nvm/versions/node/v22.14.0/bin");
        assert!(binary_dirs("macos", &roots).contains(&expected));
        assert!(allow_prefixes("macos", &roots).contains(&expected));
        fs::write(home.join(".nvm/alias/default"), "../Desktop\n").unwrap();
        let roots = HostRoots::from_home(&home);
        assert!(
            !binary_dirs("macos", &roots)
                .iter()
                .any(|path| path.ends_with("Desktop") || path.ends_with("Desktop/bin"))
        );
        let _ = fs::remove_dir_all(&home);
    }

    #[test]
    fn named_catalog_files_under_an_explicit_home_may_exist_without_the_host_allowlist() {
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default();
        let home = std::env::temp_dir().join(format!(
            "lico-scan-catalog-home-{}-{}",
            stamp.as_secs(),
            stamp.subsec_nanos()
        ));
        let settings = home.join(".claude").join("settings.json");
        fs::create_dir_all(settings.parent().unwrap()).unwrap();
        fs::write(&settings, "{}\n").unwrap();
        assert!(probe_exists_under_home(&settings, &home));
        assert!(!probe_exists(&settings));
        assert!(!probe_exists_under_home(
            Path::new("/profile/Desktop/.claude/settings.json"),
            Path::new("/profile/Desktop"),
        ));
        assert!(!probe_exists_under_home(
            &posix_absolute(&["Volumes", "team-share", ".claude", "settings.json"]),
            &posix_absolute(&["Volumes", "team-share"]),
        ));
        let _ = fs::remove_dir_all(&home);
    }

    #[test]
    fn the_real_home_is_a_valid_named_catalog_base() {
        let home = PathBuf::from("/profile");
        assert!(!catalog_home_denied(&home, Some(&home)));
        assert!(catalog_home_denied(&home.join("Desktop"), Some(&home)));
        assert!(catalog_home_denied(
            &unc_path("server", "redirected-profile"),
            Some(&home)
        ));
    }

    #[test]
    fn windows_network_roots_never_enter_the_binary_probe_set() {
        let mut roots = fixture_roots();
        roots.appdata = Some(unc_path("server", r"profile\AppData\Roaming"));
        roots.local_appdata = Some(unc_path("server", r"profile\AppData\Local"));
        let dirs = binary_dirs("windows", &roots);
        assert!(dirs.iter().all(|path| !is_windows_network_path(path)));
    }

    #[test]
    fn unused_agent_discovery_never_authorizes_automatic_execution() {
        assert!(!automatic_agent_execution_admitted());
        assert!(!discovered_agent_may_execute(
            Path::new("/opt/homebrew/bin/claude"),
            false
        ));
        assert!(discovered_agent_may_execute(
            Path::new("/opt/homebrew/bin/claude"),
            true
        ));
        assert!(!discovered_agent_may_execute(
            &posix_absolute(&["Volumes", "team-share", "claude"]),
            true
        ));
    }

    #[test]
    fn unused_agent_probes_skip_other_app_containers_without_denying_them() {
        let roots = fixture_roots();
        let cursor_support =
            PathBuf::from("/profile/Library/Application Support/Cursor/User/workspaceStorage");
        let kimi_logs = PathBuf::from("/profile/Library/Logs/Kimi");
        let pnpm = PathBuf::from("/profile/Library/pnpm");
        let claude = PathBuf::from("/profile/.claude");
        assert!(admitted_scan_path_with(&cursor_support, &roots));
        assert!(is_other_app_container_with(&cursor_support, &roots));
        assert!(is_other_app_container_with(&kimi_logs, &roots));
        assert!(!is_other_app_container_with(&pnpm, &roots));
        assert!(!is_other_app_container_with(&claude, &roots));
        assert!(!denied(&cursor_support, roots.home.as_deref()));
        assert!(is_other_app_container_with(
            &PathBuf::from("/profile/Library/Mobile Documents/com~apple~CloudDocs"),
            &roots
        ));
        assert!(is_other_app_container_with(
            &PathBuf::from("/profile/Library/CloudStorage/iCloudDrive"),
            &roots
        ));
    }

    #[test]
    fn selected_agent_named_store_exists_skips_other_app_until_selected() {
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default();
        let home = std::env::temp_dir().join(format!(
            "lico-selected-agent-store-{}-{}",
            stamp.as_secs(),
            stamp.subsec_nanos()
        ));
        let other_app = home
            .join("Library")
            .join("Application Support")
            .join("Code")
            .join("User")
            .join("globalStorage")
            .join("state.vscdb");
        fs::create_dir_all(other_app.parent().unwrap()).unwrap();
        fs::write(&other_app, []).unwrap();
        let own_store = home
            .join(".local")
            .join("share")
            .join("kilo")
            .join("kilo.db");
        fs::create_dir_all(own_store.parent().unwrap()).unwrap();
        fs::write(&own_store, []).unwrap();

        assert!(!selected_agent_named_store_exists(&other_app, &home, false));
        assert!(selected_agent_named_store_exists(&other_app, &home, true));
        assert!(selected_agent_named_store_exists(&own_store, &home, false));
        let _ = fs::remove_dir_all(&home);
    }
}
