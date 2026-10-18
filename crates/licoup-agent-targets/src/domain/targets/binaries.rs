use super::catalog::TargetDef;
use super::parameters::{discovered_agent_execution_requested, param_string};
use super::scan_paths::{self, HostRoots, probe_is_file};
use crate::port::AgentTargetPort;
use serde_json::Value;
use std::collections::BTreeSet;
use std::env;
use std::fs;
use std::path::{Path, PathBuf};

pub(super) const BINARY_SOURCE_APPLICATION_STORE: &str = "application-store";
pub(super) const BINARY_SOURCE_PACKAGE_MANAGER: &str = "package-manager";
pub(super) const BINARY_SOURCE_EXECUTABLE_PATH: &str = "executable-path";

pub fn find_binary(names: &[&str]) -> Option<PathBuf> {
    find_binary_with_path_dirs(
        names,
        &crate::platform::user_shell_environment::search_path_dirs(),
    )
}

/// The user shell PATH (with the process PATH fallback) is the default
/// command authority (ADR 0007): a CLI the user can run from their terminal
/// is found even when the LicoUp process PATH lacks it. The scan manifest
/// dirs stay supplementary and are searched after the PATH dirs.
fn find_binary_with_path_dirs(names: &[&str], path_dirs: &[PathBuf]) -> Option<PathBuf> {
    if let Some(found) = find_binary_in_dirs(names, path_dirs) {
        return Some(found);
    }
    find_binary_in_dirs(names, &binary_search_dirs())
}

pub(super) fn find_target_binary(def: &TargetDef, params: &Value) -> Option<PathBuf> {
    if def.id == "command-code" {
        return find_command_code_binary();
    }
    if def.id == "codex" {
        return find_codex_binary_with_source().map(|(path, _)| path);
    }
    if def.id != "cursor" {
        return find_binary(def.binary_names);
    }
    let path_dirs = crate::platform::user_shell_environment::search_path_dirs();
    find_cursor_binary_in_dirs(&path_dirs, params)
        .or_else(|| find_cursor_binary_in_dirs(&binary_search_dirs(), params))
}

fn find_command_code_binary() -> Option<PathBuf> {
    let path_dirs = crate::platform::user_shell_environment::search_path_dirs();
    find_command_code_binary_in_dirs(&path_dirs)
        .or_else(|| find_command_code_binary_in_dirs(&binary_search_dirs()))
}

fn find_command_code_binary_in_dirs(dirs: &[PathBuf]) -> Option<PathBuf> {
    find_binary_in_dirs(&["command-code", "cmdc"], dirs).or_else(|| {
        find_binary_in_dirs(&["cmd"], dirs).filter(|path| !is_system_command_shell(path))
    })
}

fn is_system_command_shell(path: &Path) -> bool {
    let display = path
        .to_string_lossy()
        .replace('\\', "/")
        .to_ascii_lowercase();
    let file_name = display.rsplit('/').next().unwrap_or(display.as_str());
    if file_name != "cmd" && file_name != "cmd.exe" {
        return false;
    }
    display.contains("/system32/")
        || display.contains("/syswow64/")
        || display.contains("/windows/")
        || display.contains("/sbin/")
        || matches!(display.as_str(), "/bin/cmd" | "/usr/bin/cmd" | "/sbin/cmd")
}

fn find_cursor_binary_in_dirs(dirs: &[PathBuf], _params: &Value) -> Option<PathBuf> {
    // Presence is lexical plus an allowlisted file probe. Never execute a
    // third-party binary to decide whether Cursor is installed, and never bind
    // the IDE `cursor` shim as the Agent CLI lane.
    find_binary_in_dirs(&["cursor-agent"], dirs)
}

pub(super) fn find_target_binary_with_source(
    def: &TargetDef,
    params: &Value,
) -> Option<(PathBuf, &'static str)> {
    if def.id == "codex" {
        return find_codex_binary_with_source();
    }
    if let Some(path) = find_target_binary(def, params) {
        let source = classify_binary_source(&path);
        return Some((path, source));
    }
    find_extension_bundled_binary(def).map(|path| {
        let source = classify_binary_source(&path);
        (path, source)
    })
}

/// Prefer the official desktop package entrypoint; standalone Codex CLIs stay
/// available when no packaged runtime is installed.
fn find_codex_binary_with_source() -> Option<(PathBuf, &'static str)> {
    let roots = HostRoots::from_environment();
    let os = std::env::consts::OS;
    let bundled_dirs = scan_paths::agent_binary_dirs("codex", os, &roots);
    find_codex_binary_with_source_from_dirs(
        &bundled_dirs,
        || crate::platform::user_shell_environment::search_path_dirs(),
        || scan_paths::binary_dirs(os, &roots),
    )
}

fn find_codex_binary_with_source_from_dirs(
    bundled_dirs: &[PathBuf],
    path_dirs: impl FnOnce() -> Vec<PathBuf>,
    fallback_dirs: impl FnOnce() -> Vec<PathBuf>,
) -> Option<(PathBuf, &'static str)> {
    let candidate = find_binary_in_dirs(&["codex"], bundled_dirs)
        .or_else(|| find_binary_in_dirs(&["codex"], &path_dirs()))
        .or_else(|| find_binary_in_dirs(&["codex"], &fallback_dirs()))?;
    let source = classify_binary_source(&candidate);
    Some((candidate, source))
}

/// Some targets ship their official executable inside a product package
/// instead of a PATH-visible install. Kilo Code bundles `bin/kilo` (or
/// `bin/kilo.exe` on Windows) inside the VS Code extension directory, and
/// that binary speaks the same `kilo serve` contract as the standalone CLI,
/// so an extension-only install is a full runtime source.
pub(super) fn find_extension_bundled_binary(def: &TargetDef) -> Option<PathBuf> {
    match def.id {
        "kilo-code" => find_kilo_code_extension_cli(&scan_paths::extension_roots(
            "kilo-code",
            std::env::consts::OS,
            &HostRoots::from_environment(),
        )),
        _ => None,
    }
}

/// Desktop application presence for agents whose desktop product is a
/// verified macOS bundle. Agents without a mapped desktop product never
/// report detection.
pub(super) fn desktop_app_executable(agent_id: &str) -> Option<PathBuf> {
    scan_paths::app_executables(
        agent_id,
        std::env::consts::OS,
        &HostRoots::from_environment(),
    )
    .into_iter()
    .find(|path| probe_is_file(path))
}

pub(super) fn find_kilo_code_extension_cli(roots: &[PathBuf]) -> Option<PathBuf> {
    let mut best: Option<(Vec<u64>, usize, PathBuf)> = None;
    for (root_index, root) in roots.iter().enumerate() {
        if !automatic_path_is_dir(root) {
            continue;
        }
        let Ok(entries) = fs::read_dir(root) else {
            continue;
        };
        for entry in entries.filter_map(|entry| entry.ok()) {
            let name = entry.file_name().to_string_lossy().to_ascii_lowercase();
            if name != "kilocode.kilo-code" && !name.starts_with("kilocode.kilo-code-") {
                continue;
            }
            let Some(binary) = kilo_code_bundled_binary(&entry.path()) else {
                continue;
            };
            let rank = kilo_extension_version_rank(&name);
            let replace = match &best {
                None => true,
                Some((best_rank, best_root, _)) => {
                    rank > *best_rank || (rank == *best_rank && root_index < *best_root)
                }
            };
            if replace {
                best = Some((rank, root_index, binary));
            }
        }
    }
    best.map(|(_, _, path)| path)
}

fn kilo_code_bundled_binary(extension_dir: &Path) -> Option<PathBuf> {
    kilo_bundled_binary_names()
        .into_iter()
        .map(|name| extension_dir.join("bin").join(name))
        .find(|candidate| automatic_path_is_file(candidate))
}

#[cfg(not(target_os = "windows"))]
fn kilo_bundled_binary_names() -> [&'static str; 1] {
    ["kilo"]
}

#[cfg(target_os = "windows")]
fn kilo_bundled_binary_names() -> [&'static str; 2] {
    ["kilo.exe", "kilo"]
}

fn kilo_extension_version_rank(extension_dir_name: &str) -> Vec<u64> {
    extension_dir_name
        .strip_prefix("kilocode.kilo-code-")
        .unwrap_or("")
        .split('-')
        .next()
        .unwrap_or("")
        .split('.')
        .map(|part| part.parse::<u64>().unwrap_or(0))
        .collect()
}

pub(super) fn cursor_binary_supports_acp(
    port: &AgentTargetPort,
    binary: &Path,
    params: &Value,
) -> bool {
    if !discovered_agent_execution_requested(params)
        || !scan_paths::discovered_agent_may_execute(binary, true)
    {
        return false;
    }
    // The Cursor capability probe only runs `--version` / `--help` and does not
    // need a project workspace. Keep a cwd for the shared probe API, but never
    // treat a missing default workspace as "binary unsupported" — that hid a
    // working `cursor-agent` from Adaptive Flywheel and conversation relays.
    let cwd = param_string(params, "workingDirectory")
        .or_else(|| param_string(params, "cwd"))
        .map(PathBuf::from)
        .filter(|path| path.is_absolute())
        .or_else(|| (port.default_local_agent_workspace)("cursor"))
        .unwrap_or_else(env::temp_dir);
    (port.probe_runtime_driver)("cursor", binary, &cwd)
        .get("supported")
        .and_then(Value::as_bool)
        == Some(true)
}

pub(super) fn find_binary_in_dirs(names: &[&str], dirs: &[PathBuf]) -> Option<PathBuf> {
    let home = licoup_foundation::platform::paths::user_home_from_env();
    for dir in dirs {
        for name in names {
            for candidate in binary_candidate_paths(dir, name) {
                if scan_paths::denied(&candidate, home.as_deref()) {
                    continue;
                }
                if automatic_path_is_file(&candidate) {
                    return Some(candidate);
                }
            }
        }
    }
    None
}

fn automatic_path_is_file(path: &Path) -> bool {
    !scan_paths::symlink_escapes_denied_location(path) && path.is_file()
}

fn automatic_path_is_dir(path: &Path) -> bool {
    !scan_paths::symlink_escapes_denied_location(path) && path.is_dir()
}

fn binary_search_dirs() -> Vec<PathBuf> {
    scan_paths::binary_dirs(std::env::consts::OS, &HostRoots::from_environment())
}

fn binary_candidate_paths(dir: &Path, name: &str) -> Vec<PathBuf> {
    #[cfg(not(target_os = "windows"))]
    let candidates = vec![dir.join(name)];

    #[cfg(target_os = "windows")]
    let mut candidates = vec![dir.join(name)];
    #[cfg(target_os = "windows")]
    {
        if Path::new(name).extension().is_none() {
            for extension in windows_binary_extensions() {
                candidates.push(dir.join(format!("{}{}", name, extension)));
            }
        }
    }
    dedupe_paths(candidates)
}

#[cfg(target_os = "windows")]
fn windows_binary_extensions() -> Vec<String> {
    let mut extensions = env::var("PATHEXT")
        .unwrap_or_else(|_| ".COM;.EXE;.BAT;.CMD".to_string())
        .split(';')
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(|value| {
            if value.starts_with('.') {
                value.to_string()
            } else {
                format!(".{}", value)
            }
        })
        .collect::<Vec<_>>();
    for extension in [".exe", ".cmd", ".bat", ".com"] {
        if !extensions
            .iter()
            .any(|value| value.eq_ignore_ascii_case(extension))
        {
            extensions.push(extension.to_string());
        }
    }
    extensions
}

fn classify_binary_source(path: &Path) -> &'static str {
    let normalized = path
        .to_string_lossy()
        .replace('\\', "/")
        .to_ascii_lowercase();
    if normalized.contains("/applications/") || normalized.contains("/windowsapps/") {
        return BINARY_SOURCE_APPLICATION_STORE;
    }
    // Editor extension directories are installed from the editor's extension
    // marketplace, which is an application store for discovery purposes.
    if [
        "/.vscode/extensions/",
        "/.vscode-insiders/extensions/",
        "/.cursor/extensions/",
        "/.vscodium/extensions/",
    ]
    .iter()
    .any(|marker| normalized.contains(marker))
    {
        return BINARY_SOURCE_APPLICATION_STORE;
    }
    if [
        "/homebrew/",
        "/winget/",
        "/chocolatey/",
        "/scoop/",
        "/npm/",
        "/pnpm/",
        "/.cargo/",
        "/.bun/",
        "/snap/",
        "/flatpak/",
    ]
    .iter()
    .any(|marker| normalized.contains(marker))
    {
        return BINARY_SOURCE_PACKAGE_MANAGER;
    }
    BINARY_SOURCE_EXECUTABLE_PATH
}

fn dedupe_paths(paths: Vec<PathBuf>) -> Vec<PathBuf> {
    let mut seen = BTreeSet::<String>::new();
    let mut out = Vec::<PathBuf>::new();
    for path in paths {
        let key = path.to_string_lossy().to_ascii_lowercase();
        if seen.insert(key) {
            out.push(path);
        }
    }
    out
}

#[cfg(test)]
pub(super) fn find_macos_app_executable(
    app_bundle: &str,
    executable: &str,
    roots: &[PathBuf],
) -> Option<PathBuf> {
    let home = licoup_foundation::platform::paths::user_home_from_env();
    roots
        .iter()
        .map(|root| {
            root.join(app_bundle)
                .join("Contents")
                .join("MacOS")
                .join(executable)
        })
        .find(|candidate| !scan_paths::denied(candidate, home.as_deref()) && candidate.is_file())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn posix_path(components: &[&str]) -> PathBuf {
        components.iter().fold(
            PathBuf::from(char::from(47).to_string()),
            |path, component| path.join(component),
        )
    }

    #[test]
    fn binary_candidates_preserve_priority_and_dedupe_case_insensitively() {
        let dir = posix_path(&["tools"]);
        let candidates = binary_candidate_paths(&dir, "codex");
        assert_eq!(candidates.first(), Some(&dir.join("codex")));
        let deduped = dedupe_paths(vec![dir.join("Codex"), dir.join("codex")]);
        assert_eq!(deduped.len(), 1);
    }

    #[test]
    fn command_code_never_binds_the_system_command_shell() {
        let windows_cmd = ["C:", "Windows", "System32", "cmd.exe"]
            .iter()
            .collect::<PathBuf>();
        assert!(is_system_command_shell(&windows_cmd));
        assert!(is_system_command_shell(Path::new("/usr/bin/cmd")));
        assert!(!is_system_command_shell(Path::new(
            "/opt/homebrew/bin/command-code"
        )));
        assert!(!is_system_command_shell(&posix_path(&[
            "opt", "local", "bin", "cmd"
        ])));
    }

    #[test]
    fn platform_sources_cover_application_stores_and_package_managers() {
        let roots = HostRoots {
            home: Some(posix_path(&["profile"])),
            appdata: Some(posix_path(&["app-data"])),
            local_appdata: Some(posix_path(&["local-app-data"])),
            xdg_config: Some(posix_path(&["profile", ".config"])),
            xdg_data: Some(posix_path(&["profile", ".local", "share"])),
            program_data: Some(posix_path(&["program-data"])),
            program_files: Some(posix_path(&["program-files"])),
            program_files_x86: None,
            portable: None,
        };
        let windows = scan_paths::binary_dirs("windows", &roots);
        assert!(windows.contains(&posix_path(&["local-app-data", "Microsoft", "WindowsApps"])));
        assert!(windows.contains(&posix_path(&[
            "local-app-data",
            "Microsoft",
            "WinGet",
            "Links"
        ])));
        assert!(windows.contains(&posix_path(&["profile", "scoop", "shims"])));
        assert!(windows.contains(&posix_path(&["program-data", "chocolatey", "bin"])));

        let macos = scan_paths::binary_dirs("macos", &roots);
        assert!(macos.contains(&posix_path(&["opt", "homebrew", "bin"])));
        assert!(
            macos
                .iter()
                .any(|path| path.to_string_lossy().contains("Cursor.app"))
        );

        let linux = scan_paths::binary_dirs("linux", &roots);
        assert!(linux.contains(&posix_path(&["snap", "bin"])));
        assert!(linux.contains(&posix_path(&["var", "lib", "flatpak", "exports", "bin"])));
    }

    #[test]
    fn macos_automatic_search_skips_protected_and_network_locations() {
        let home = posix_path(&["profile"]);
        for protected in [
            posix_path(&["profile", "Downloads", "bin"]),
            posix_path(&["profile", "Desktop", "tools"]),
            posix_path(&["profile", "Documents", "scripts"]),
            posix_path(&["Volumes", "team-share", "bin"]),
        ] {
            assert!(scan_paths::denied(&protected, Some(&home)));
        }
    }

    #[test]
    fn macos_automatic_search_keeps_system_and_hidden_package_locations() {
        let roots = HostRoots {
            home: Some(posix_path(&["profile"])),
            appdata: None,
            local_appdata: None,
            xdg_config: Some(posix_path(&["profile", ".config"])),
            xdg_data: Some(posix_path(&["profile", ".local", "share"])),
            program_data: None,
            program_files: None,
            program_files_x86: None,
            portable: None,
        };
        let dirs = scan_paths::binary_dirs("macos", &roots);
        for allowed in [
            posix_path(&["usr", "local", "bin"]),
            posix_path(&["opt", "homebrew", "bin"]),
            posix_path(&[
                "Applications",
                "Cursor.app",
                "Contents",
                "Resources",
                "app",
                "bin",
            ]),
            posix_path(&["profile", ".local", "bin"]),
            posix_path(&["profile", ".nvm", "current", "bin"]),
            posix_path(&["profile", ".local", "share", "mise", "shims"]),
            posix_path(&["profile", "Library", "pnpm"]),
        ] {
            assert!(dirs.contains(&allowed), "missing {allowed:?}");
        }
    }

    #[test]
    fn discovered_binary_source_is_minimal_and_stable() {
        assert_eq!(
            classify_binary_source(&posix_path(&[
                "Applications",
                "Cursor.app",
                "Contents",
                "Resources",
                "app",
                "bin",
                "cursor",
            ])),
            BINARY_SOURCE_APPLICATION_STORE
        );
        assert_eq!(
            classify_binary_source(&posix_path(&["profile", "scoop", "shims", "codex.exe"])),
            BINARY_SOURCE_PACKAGE_MANAGER
        );
        assert_eq!(
            classify_binary_source(&posix_path(&["custom", "bin", "codex"])),
            BINARY_SOURCE_EXECUTABLE_PATH
        );
    }

    #[cfg(unix)]
    #[test]
    fn codex_bundle_precedes_a_stale_path_wrapper_and_cli_path_remains_a_fallback() {
        use std::os::unix::fs::PermissionsExt;

        let dir = unique_temp_dir("codex-bundle-precedence");
        let path_dir = dir.join("shell-bin");
        let bundled_dir = dir.join("Applications/ChatGPT.app/Contents/Resources/codex-cli/bin");
        fs::create_dir_all(&path_dir).unwrap();
        fs::create_dir_all(&bundled_dir).unwrap();

        let stale_wrapper = path_dir.join("codex");
        fs::write(
            &stale_wrapper,
            "#!/bin/sh\nexec /missing/vendor/codex \"$@\"\n",
        )
        .unwrap();
        fs::set_permissions(&stale_wrapper, fs::Permissions::from_mode(0o755)).unwrap();
        let bundled_cli = bundled_dir.join("codex");
        fs::write(&bundled_cli, "synthetic packaged Codex entrypoint").unwrap();

        let found = find_codex_binary_with_source_from_dirs(
            std::slice::from_ref(&bundled_dir),
            || panic!("bundle hit must not read shell PATH"),
            || panic!("bundle hit must not enumerate fallback paths"),
        )
        .unwrap();
        assert_eq!(found.0, bundled_cli);
        assert_eq!(found.1, BINARY_SOURCE_APPLICATION_STORE);

        fs::remove_file(&bundled_cli).unwrap();
        fs::remove_file(&stale_wrapper).unwrap();
        let independent_cli = path_dir.join("codex");
        fs::write(&independent_cli, "synthetic independent Codex CLI").unwrap();
        let found = find_codex_binary_with_source_from_dirs(
            &[],
            || vec![path_dir.clone()],
            || panic!("PATH hit must not enumerate fallback paths"),
        )
        .unwrap();
        assert_eq!(found.0, independent_cli);
        assert_eq!(found.1, BINARY_SOURCE_EXECUTABLE_PATH);

        let manifest_dir = dir.join("manifest-bin");
        fs::create_dir_all(&manifest_dir).unwrap();
        let manifest_cli = manifest_dir.join("codex");
        fs::write(&manifest_cli, "synthetic manifest Codex candidate").unwrap();
        let found =
            find_codex_binary_with_source_from_dirs(&[], Vec::new, || vec![manifest_dir.clone()])
                .unwrap();
        assert_eq!(found.0, manifest_cli);
        assert_eq!(found.1, BINARY_SOURCE_EXECUTABLE_PATH);

        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn editor_extension_bundled_binary_is_an_application_store_source() {
        for root in [".vscode", ".vscode-insiders", ".cursor", ".vscodium"] {
            let bundled = posix_path(&[
                "profile",
                root,
                "extensions",
                "kilocode.kilo-code-7.4.15",
                "bin",
                "kilo",
            ]);
            assert_eq!(
                classify_binary_source(&bundled),
                BINARY_SOURCE_APPLICATION_STORE
            );
        }
    }

    #[test]
    fn kilo_extension_version_rank_orders_semver_like_names() {
        let newer = kilo_extension_version_rank("kilocode.kilo-code-7.4.15-darwin-arm64");
        let older = kilo_extension_version_rank("kilocode.kilo-code-7.4.2-darwin-arm64");
        let unversioned = kilo_extension_version_rank("kilocode.kilo-code");
        assert!(newer > older);
        assert!(older > unversioned);
        assert_eq!(
            kilo_extension_version_rank("kilocode.kilo-code-7.10.0"),
            kilo_extension_version_rank("kilocode.kilo-code-7.10.0-darwin-arm64")
        );
    }

    #[test]
    fn kilo_code_extension_cli_prefers_newest_version_with_bundled_binary() {
        let dir = unique_temp_dir("kilo-extension-cli");
        let stable_root = dir.join(".vscode").join("extensions");
        let insiders_root = dir.join(".vscode-insiders").join("extensions");
        // Newest install lacks the bundled binary and must be skipped.
        fs::create_dir_all(stable_root.join("kilocode.kilo-code-9.0.0")).unwrap();
        let bundled = stable_root
            .join("kilocode.kilo-code-7.4.15-darwin-arm64")
            .join("bin")
            .join(kilo_bundled_binary_names()[0]);
        fs::create_dir_all(bundled.parent().unwrap()).unwrap();
        fs::write(&bundled, "kilo").unwrap();
        let older = insiders_root
            .join("kilocode.kilo-code-6.0.0")
            .join("bin")
            .join(kilo_bundled_binary_names()[0]);
        fs::create_dir_all(older.parent().unwrap()).unwrap();
        fs::write(&older, "kilo").unwrap();
        // Unrelated extensions must be ignored.
        fs::create_dir_all(stable_root.join("kilocode.other-1.0.0").join("bin")).unwrap();

        let found = find_kilo_code_extension_cli(&[stable_root, insiders_root]).unwrap();

        assert_eq!(found, bundled);
    }

    #[test]
    fn kilo_code_extension_cli_prefers_earlier_root_on_version_tie() {
        let dir = unique_temp_dir("kilo-extension-cli-tie");
        let stable_root = dir.join(".vscode").join("extensions");
        let cursor_root = dir.join(".cursor").join("extensions");
        for root in [&stable_root, &cursor_root] {
            let binary = root
                .join("kilocode.kilo-code-7.4.15")
                .join("bin")
                .join(kilo_bundled_binary_names()[0]);
            fs::create_dir_all(binary.parent().unwrap()).unwrap();
            fs::write(binary, "kilo").unwrap();
        }

        let found = find_kilo_code_extension_cli(&[stable_root.clone(), cursor_root]).unwrap();

        assert!(found.starts_with(&stable_root));
    }

    #[test]
    fn kilo_code_extension_cli_returns_none_without_bundled_binary() {
        let dir = unique_temp_dir("kilo-extension-cli-empty");
        let root = dir.join(".vscode").join("extensions");
        fs::create_dir_all(root.join("kilocode.kilo-code-7.4.15")).unwrap();
        fs::create_dir_all(dir.join("empty-root")).unwrap();

        assert!(find_kilo_code_extension_cli(&[root, dir.join("empty-root")]).is_none());
        assert!(find_kilo_code_extension_cli(&[dir.join("missing-root")]).is_none());
    }

    #[test]
    fn macos_app_executable_resolves_any_verified_bundle() {
        let dir = unique_temp_dir("macos-app-executable");
        let root = dir.join("Applications");
        let executable = root
            .join("ChatGPT.app")
            .join("Contents")
            .join("MacOS")
            .join("ChatGPT");
        fs::create_dir_all(executable.parent().unwrap()).unwrap();
        fs::write(&executable, "chatgpt").unwrap();

        assert_eq!(
            find_macos_app_executable("ChatGPT.app", "ChatGPT", std::slice::from_ref(&root)),
            Some(executable)
        );
        assert!(
            find_macos_app_executable(
                "Antigravity.app",
                "Antigravity",
                std::slice::from_ref(&root),
            )
            .is_none()
        );
        assert!(find_macos_app_executable("ChatGPT.app", "Other", &[root]).is_none());
    }

    #[test]
    fn desktop_app_executable_maps_only_verified_agent_products() {
        // Unmapped agents never report desktop detection.
        for agent in ["claude-code", "opencode", "kimi-code", "pi"] {
            assert!(desktop_app_executable(agent).is_none());
        }
        // Mapped agents read only the verified macOS roots; on non-macOS
        // platforms no roots exist and detection is always empty.
        if std::env::consts::OS != "macos" {
            for agent in ["codex", "antigravity", "cursor"] {
                assert!(desktop_app_executable(agent).is_none());
            }
        }
    }

    #[cfg(unix)]
    #[test]
    fn shell_path_dirs_are_searched_before_manifest_dirs() {
        use std::os::unix::fs::PermissionsExt;

        let shell_dir = unique_temp_dir("shell-path-first");
        let manifest_dir = unique_temp_dir("manifest-path-second");
        let shell_binary = shell_dir.join("fixture-agent-cli");
        fs::write(&shell_binary, "#!/bin/sh\n").unwrap();
        fs::set_permissions(&shell_binary, fs::Permissions::from_mode(0o755)).unwrap();

        // A CLI present only on the user shell PATH is found, and it outranks
        // the same name in the supplementary manifest dirs.
        let found =
            find_binary_with_path_dirs(&["fixture-agent-cli"], std::slice::from_ref(&shell_dir));
        assert_eq!(found.as_deref(), Some(shell_binary.as_path()));

        let manifest_binary = manifest_dir.join("fixture-agent-cli");
        fs::write(&manifest_binary, "#!/bin/sh\n").unwrap();
        let found = find_binary_in_dirs(
            &["fixture-agent-cli"],
            &[shell_dir.clone(), manifest_dir.clone()],
        );
        assert_eq!(found.as_deref(), Some(shell_binary.as_path()));
        let found =
            find_binary_in_dirs(&["fixture-agent-cli"], std::slice::from_ref(&manifest_dir));
        assert_eq!(found.as_deref(), Some(manifest_binary.as_path()));
    }

    #[cfg(unix)]
    #[test]
    fn cursor_agent_binds_even_when_short_capability_probe_fails() {
        use std::os::unix::fs::PermissionsExt;

        let dir = unique_temp_dir("cursor-agent-fallback-bind");
        let agent = dir.join("cursor-agent");
        // Help intentionally omits create-chat so the short probe rejects it.
        fs::write(
            &agent,
            "#!/bin/sh\ncase \"$1\" in\n--version) echo 1; exit 0 ;;\n--help) echo 'Usage: agent'; exit 0 ;;\n*) exit 1 ;;\nesac\n",
        )
        .unwrap();
        fs::set_permissions(&agent, fs::Permissions::from_mode(0o755)).unwrap();

        let found = find_cursor_binary_in_dirs(std::slice::from_ref(&dir), &serde_json::json!({}));
        assert_eq!(found.as_deref(), Some(agent.as_path()));
        assert!(!cursor_binary_supports_acp(
            &crate::port::fixtures::packaged(),
            &agent,
            &serde_json::json!({})
        ));
    }

    #[cfg(unix)]
    #[test]
    fn cursor_agent_discovery_does_not_execute_the_binary() {
        use std::os::unix::fs::PermissionsExt;

        let dir = unique_temp_dir("cursor-agent-no-exec");
        let marker = dir.join("executed");
        let agent = dir.join("cursor-agent");
        fs::write(
            &agent,
            format!("#!/bin/sh\nprintf x > \"{}\"\nexit 0\n", marker.display()),
        )
        .unwrap();
        fs::set_permissions(&agent, fs::Permissions::from_mode(0o755)).unwrap();

        let found = find_cursor_binary_in_dirs(std::slice::from_ref(&dir), &serde_json::json!({}));
        assert_eq!(found.as_deref(), Some(agent.as_path()));
        assert!(!marker.exists());
    }

    fn unique_temp_dir(name: &str) -> PathBuf {
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default();
        let dir = env::temp_dir().join(format!(
            "lico-targets-{}-{}-{}",
            name,
            stamp.as_secs(),
            stamp.subsec_nanos()
        ));
        fs::create_dir_all(&dir).unwrap();
        dir
    }
}
