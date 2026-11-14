//! Allowlisted Agent discovery paths. The TOML manifest is the only automatic
//! search space; PATH, personal library roots, and network volumes are never
//! walked.
//!
//! The manifest and LicoUp's `{portable}` root stay here — they are LicoUp's
//! own inventory. The scan engine (template expansion, admission, deny and
//! symlink-escape rules, probes, executable resolution) lives in
//! `a4aa::discovery`, so every host that scans for agents shares one
//! implementation.

use a4aa::discovery::ScanManifest;
use licoup_foundation::platform::paths::portable_data_dir;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
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
        let roots = a4aa::discovery::HostRoots::from_environment();
        Self {
            home: roots.home,
            appdata: roots.appdata,
            local_appdata: roots.local_appdata,
            xdg_config: roots.xdg_config,
            xdg_data: roots.xdg_data,
            program_data: roots.program_data,
            program_files: roots.program_files,
            program_files_x86: roots.program_files_x86,
            portable: portable_data_dir().ok(),
        }
    }

    pub fn from_home(home: &Path) -> Self {
        let roots = a4aa::discovery::HostRoots::from_home(home);
        Self {
            home: roots.home,
            appdata: roots.appdata,
            local_appdata: roots.local_appdata,
            xdg_config: roots.xdg_config,
            xdg_data: roots.xdg_data,
            program_data: roots.program_data,
            program_files: roots.program_files,
            program_files_x86: roots.program_files_x86,
            portable: portable_data_dir().ok(),
        }
    }
}

#[derive(Clone, Debug)]
pub struct HistoryScanRoot {
    pub path: PathBuf,
    pub kind: String,
}

/// The parsed scan manifest. `toml` deserialization stays here because the
/// document is LicoUp's own; the schema the engine reads is a4aa's.
pub(crate) fn scan_manifest() -> &'static ScanManifest {
    static PARSED: OnceLock<ScanManifest> = OnceLock::new();
    PARSED.get_or_init(|| {
        let parsed: ScanManifest = toml::from_str(MANIFEST).expect("agent-scan-paths.toml");
        assert_eq!(parsed.schema_version, SCHEMA_VERSION);
        parsed
    })
}

/// LicoUp's host roots carried into the shared engine: the standard tokens
/// map field-for-field and `{portable}` travels through the engine's
/// host-declared `extra` root map.
fn engine_roots(roots: &HostRoots) -> a4aa::discovery::HostRoots {
    let mut engine = a4aa::discovery::HostRoots {
        home: roots.home.clone(),
        appdata: roots.appdata.clone(),
        local_appdata: roots.local_appdata.clone(),
        xdg_config: roots.xdg_config.clone(),
        xdg_data: roots.xdg_data.clone(),
        program_data: roots.program_data.clone(),
        program_files: roots.program_files.clone(),
        program_files_x86: roots.program_files_x86.clone(),
        extra: BTreeMap::new(),
    };
    if let Some(portable) = roots.portable.clone() {
        engine.extra.insert("portable".to_string(), portable);
    }
    engine
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
    a4aa::discovery::expand_template(template, &engine_roots(roots)).map(|path| {
        // Manifest templates spell separators portably (`{home}/.local/bin`);
        // componentizing gives the expanded path the host's own separator form.
        path.components().collect()
    })
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

/// The nvm default Node bin dir under `home`; the shared engine resolves the
/// `default` alias chain so discovery never enumerates installed versions.
pub fn nvm_default_bin_dirs(home: &Path) -> Vec<PathBuf> {
    scan_manifest().nvm_default_bin_dirs(home)
}

pub fn binary_dirs(os: &str, roots: &HostRoots) -> Vec<PathBuf> {
    scan_manifest().binary_dirs(os, &engine_roots(roots))
}

pub fn agent_binary_dirs(agent_id: &str, os: &str, roots: &HostRoots) -> Vec<PathBuf> {
    scan_manifest().agent_binary_dirs(agent_id, os, &engine_roots(roots))
}

pub fn config_path(agent_id: &str, os: &str, roots: &HostRoots) -> Option<PathBuf> {
    scan_manifest().config_path(agent_id, os, &engine_roots(roots))
}

pub fn detection_paths(agent_id: &str, os: &str, roots: &HostRoots) -> Vec<PathBuf> {
    scan_manifest().detection_paths(agent_id, os, &engine_roots(roots))
}

pub fn history_roots(agent_id: &str, roots: &HostRoots) -> Vec<HistoryScanRoot> {
    scan_manifest()
        .history_roots(agent_id, &engine_roots(roots))
        .into_iter()
        .map(|root| HistoryScanRoot {
            path: root.path,
            kind: root.kind,
        })
        .collect()
}

pub fn extension_roots(agent_id: &str, os: &str, roots: &HostRoots) -> Vec<PathBuf> {
    scan_manifest().extension_roots(agent_id, os, &engine_roots(roots))
}

pub fn app_executables(agent_id: &str, os: &str, roots: &HostRoots) -> Vec<PathBuf> {
    scan_manifest().app_executables(agent_id, os, &engine_roots(roots))
}

/// Every location the manifest covers, used as the admission allowlist.
pub fn allow_prefixes(os: &str, roots: &HostRoots) -> Vec<PathBuf> {
    scan_manifest().allow_prefixes(os, &engine_roots(roots))
}

#[cfg(test)]
pub fn admitted_scan_path_with(path: &Path, roots: &HostRoots) -> bool {
    admitted_scan_path_for_os_with(path, std::env::consts::OS, roots)
}

fn admitted_scan_path_for_os_with(path: &Path, os: &str, roots: &HostRoots) -> bool {
    scan_manifest().admitted_scan_path(path, os, &engine_roots(roots))
}

pub fn probe_exists(path: &Path) -> bool {
    automatic_probe_admitted(path) && path.exists()
}

/// A file chosen explicitly by the user is not constrained to automatic scan
/// allowlists. Symlinks are rejected so the selected identity cannot change
/// after validation.
pub fn explicit_file_exists(path: &Path) -> bool {
    a4aa::discovery::explicit_file_exists(path)
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
    scan_manifest().probe_exists_under_home(path, home)
}

/// Allowlisted paths may be stated. They may never be executed during
/// unused-agent discovery. A new Agent recipe cannot opt a binary into
/// launch-time execution by listing it in the manifest.
pub fn automatic_agent_execution_admitted() -> bool {
    a4aa::discovery::automatic_agent_execution_admitted()
}

/// User-triggered capability or model-catalog probes may execute a discovered
/// binary only when the caller opted in and the path is not a denied personal
/// or network location.
pub fn discovered_agent_may_execute(path: &Path, execution_requested: bool) -> bool {
    scan_manifest().discovered_agent_may_execute(path, execution_requested)
}

fn automatic_probe_admitted(path: &Path) -> bool {
    automatic_probe_admitted_with(path, &HostRoots::from_environment())
}

fn automatic_probe_admitted_with(path: &Path, roots: &HostRoots) -> bool {
    automatic_probe_admitted_for_os_with(path, std::env::consts::OS, roots)
}

fn automatic_probe_admitted_for_os_with(path: &Path, os: &str, roots: &HostRoots) -> bool {
    scan_manifest().automatic_probe_admitted(path, os, &engine_roots(roots))
}

/// Resolve links one path component at a time. Inspecting only the leaf misses
/// a linked parent and would let the later filesystem probe follow that parent
/// into a denied location.
pub fn symlink_escapes_denied_location(path: &Path) -> bool {
    symlink_escapes_denied_location_with(path, &HostRoots::from_environment())
}

fn symlink_escapes_denied_location_with(path: &Path, roots: &HostRoots) -> bool {
    scan_manifest().symlink_escapes_denied_location(path, &engine_roots(roots))
}

/// Other-app containers from the manifest deny spec. Lexical; does not stat.
/// Automatic unused-agent probes skip these; a selected agent may still read
/// its own store inside one.
pub fn is_other_app_container(path: &Path) -> bool {
    is_other_app_container_with(path, &HostRoots::from_environment())
}

pub fn is_other_app_container_under_home(path: &Path, home: &Path) -> bool {
    is_other_app_container_with(path, &HostRoots::from_home(home))
}

/// Named files inside another app's container. Unused-agent scans never stat
/// these; a selected agent may `exists` and read its own store.
pub fn selected_agent_named_store_exists(path: &Path, catalog_home: &Path, selected: bool) -> bool {
    scan_manifest().selected_agent_named_store_exists(path, catalog_home, selected)
}

fn is_other_app_container_with(path: &Path, roots: &HostRoots) -> bool {
    scan_manifest().is_other_app_container(path, &engine_roots(roots))
}

/// Whether `path` lands in a denied location: a network volume, a media
/// library, a personal document root under `home`, or `home` itself.
pub fn denied(path: &Path, home: Option<&Path>) -> bool {
    scan_manifest().denied(path, home)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture_home() -> PathBuf {
        PathBuf::from("/profile")
    }

    fn roots() -> HostRoots {
        HostRoots::from_home(&fixture_home())
    }

    #[test]
    fn the_packaged_manifest_parses_and_declares_its_schema() {
        assert_eq!(scan_manifest().schema_version, SCHEMA_VERSION);
    }

    #[test]
    fn manifest_agents_expand_against_a_synthetic_home() {
        let roots = roots();
        assert_eq!(
            config_path("claude-code", "macos", &roots),
            Some(fixture_home().join(".claude").join("settings.json"))
        );
        assert!(
            binary_dirs("macos", &roots)
                .iter()
                .any(|path| path.to_string_lossy().contains("Cursor.app"))
        );
        assert!(
            history_roots("claude-code", &roots)
                .iter()
                .any(|root| root.path == fixture_home().join(".claude").join("projects"))
        );
    }

    #[test]
    fn personal_and_network_locations_stay_denied_through_the_shared_engine() {
        let home = fixture_home();
        for path in [
            home.join("Downloads").join("bin"),
            home.join("Desktop").join("tools"),
            PathBuf::from("/Volumes/team-share/bin"),
        ] {
            assert!(denied(&path, Some(&home)), "{path:?}");
        }
        assert!(!denied(&home.join(".claude"), Some(&home)));
    }

    #[test]
    fn the_portable_root_is_a_host_declared_template_token() {
        assert!(PATH_TEMPLATE_ROOTS.contains(&"portable"));
        let mut roots = roots();
        roots.portable = Some(PathBuf::from("/portable"));
        assert_eq!(
            expand_path_template("{portable}/client-state", &roots),
            Some(PathBuf::from("/portable/client-state"))
        );
    }
}
