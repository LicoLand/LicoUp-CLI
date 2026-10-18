use super::super::binaries;
use super::super::catalog::target_def;
use super::super::platform_paths::{
    default_config_path_for_platform, default_detection_path_for_platform,
    default_detection_paths_for_platform, kilo_code_extension_roots, kimi_code_home_override,
};
use super::super::processes::target_uses_running_process_detection;
use super::super::scan_targets_with_params;
use super::super::support::display_path;
use super::test_support::{port, temp_test_dir};
use serde_json::json;
use std::fs;
use std::path::{Path, PathBuf};

fn windows_test_path(drive: &str, parts: &[&str]) -> PathBuf {
    let separator = char::from(92).to_string();
    PathBuf::from(
        std::iter::once(drive)
            .chain(parts.iter().copied())
            .collect::<Vec<_>>()
            .join(&separator),
    )
}

#[test]
fn windows_default_config_paths_use_appdata_not_macos_application_support() {
    let home = windows_test_path("C:", &["Profile", "lico"]);
    let app_data = home.join("AppData").join("Roaming");
    let code = default_config_path_for_platform("code", "windows", &home, &app_data).unwrap();
    let cursor = default_config_path_for_platform("cursor", "windows", &home, &app_data).unwrap();
    let opencode =
        default_config_path_for_platform("opencode", "windows", &home, &app_data).unwrap();
    let kilo = default_config_path_for_platform("kilo-code", "windows", &home, &app_data).unwrap();
    let codex = default_config_path_for_platform("codex", "windows", &home, &app_data).unwrap();

    for path in [&code, &cursor, &opencode, &kilo] {
        let display = path.to_string_lossy();
        assert!(display.contains("AppData"));
        assert!(!display.contains("Library"));
        assert!(!display.contains("Application Support"));
    }
    assert!(code.ends_with(Path::new("Code").join("User").join("settings.json")));
    assert!(
        cursor.ends_with(
            Path::new("Cursor")
                .join("User")
                .join("globalStorage")
                .join("saoudrizwan.claude-dev")
                .join("settings")
                .join("cline_mcp_settings.json")
        )
    );
    assert!(opencode.ends_with(Path::new("opencode").join("opencode.jsonc")));
    assert!(kilo.ends_with(Path::new("kilo").join("kilo.json")));
    assert_eq!(codex, home.join(".codex").join("config.toml"));
}

#[test]
fn windows_directory_detection_preserves_the_resolved_appdata_root() {
    let home = temp_test_dir("windows-appdata-detection");
    let app_data = home.join("AppData").join("Roaming");
    let cursor_state = app_data.join("Cursor");
    fs::create_dir_all(&cursor_state).unwrap();

    assert_eq!(
        default_detection_path_for_platform("cursor", "windows", &home, &app_data),
        Some(cursor_state)
    );
}

#[test]
fn cursor_detection_keeps_desktop_state_and_acp_cli_candidates_separate() {
    let home = temp_test_dir("cursor-persistent-detection");
    let app_data = home.join("Library").join("Application Support");
    let cursor_state = app_data.join("Cursor");
    fs::create_dir_all(&cursor_state).unwrap();

    assert!(
        default_detection_paths_for_platform("cursor", "macos", &home, &app_data)
            .contains(&cursor_state)
    );
    assert_eq!(
        default_detection_path_for_platform("cursor", "macos", &home, &app_data),
        None
    );
    let cursor = target_def("cursor").unwrap();
    assert_eq!(cursor.label, "Cursor CLI");
    assert_eq!(cursor.binary_names, &["cursor-agent", "cursor"]);
    assert!(!cursor.process_names.contains(&"agent"));
    assert!(cursor.process_names.contains(&"cursor"));
}

#[test]
fn kimi_code_target_uses_official_cli_home_and_binary() {
    let home = temp_test_dir("kimi-code-target");
    let app_data = home.join("Library").join("Application Support");
    let default_root = home.join(".kimi-code");
    fs::create_dir_all(default_root.join("sessions")).unwrap();

    assert_eq!(
        default_config_path_for_platform("kimi-code", "macos", &home, &app_data),
        Some(default_root.join("config.toml"))
    );
    assert_eq!(
        default_detection_path_for_platform("kimi-code", "macos", &home, &app_data),
        Some(default_root.join("sessions"))
    );

    let custom_root = home.join("custom-kimi-code");
    assert_eq!(
        kimi_code_home_override(
            &json!({"kimiCodeHome": custom_root.to_string_lossy()}),
            &home,
        ),
        Some(custom_root)
    );

    let target = target_def("kimi-code").unwrap();
    assert_eq!(target.label, "Kimi Code CLI");
    assert_eq!(target.kind, "cli");
    assert_eq!(target.binary_names, &["kimi"]);
    assert!(target_uses_running_process_detection("kimi-code"));
}

#[test]
fn kilo_code_detection_paths_include_vscode_global_storage() {
    let home = PathBuf::from("<user-home>");
    let app_data = home.join("Library").join("Application Support");
    let paths = default_detection_paths_for_platform("kilo-code", "macos", &home, &app_data);

    assert!(paths.iter().any(|path| {
        path.ends_with(
            Path::new("Code")
                .join("User")
                .join("globalStorage")
                .join("kilocode.kilo-code"),
        )
    }));
    assert!(paths.iter().any(|path| {
        path.ends_with(
            Path::new("Cursor")
                .join("User")
                .join("globalStorage")
                .join("kilocode.kilo-code"),
        )
    }));
}

#[test]
fn kilo_code_detection_path_uses_global_storage_when_present() {
    let home = temp_test_dir("kilo-global-storage");
    let app_data = home.join("Library").join("Application Support");
    let storage = home
        .join("Library")
        .join("Application Support")
        .join("Code")
        .join("User")
        .join("globalStorage")
        .join("kilocode.kilo-code");
    fs::create_dir_all(&storage).unwrap();

    assert!(
        default_detection_paths_for_platform("kilo-code", "macos", &home, &app_data)
            .contains(&storage)
    );
    assert_eq!(
        default_detection_path_for_platform("kilo-code", "macos", &home, &app_data),
        None
    );
}

#[test]
fn kilo_code_detection_path_uses_extension_install_dir_when_present() {
    let home = temp_test_dir("kilo-extension-dir");
    let app_data = home.join(".config");
    let extension = home
        .join(".vscode")
        .join("extensions")
        .join("kilocode.kilo-code-4.0.0");
    fs::create_dir_all(&extension).unwrap();

    let detected =
        default_detection_path_for_platform("kilo-code", "linux", &home, &app_data).unwrap();

    assert_eq!(detected, extension);
}

#[test]
fn kilo_code_extension_install_dir_yields_bundled_cli_binary() {
    let home = temp_test_dir("kilo-extension-bundled-cli");
    let app_data = home.join("Library").join("Application Support");
    let extension = home
        .join(".vscode")
        .join("extensions")
        .join("kilocode.kilo-code-7.4.15-darwin-arm64");
    #[cfg(target_os = "windows")]
    let binary = extension.join("bin").join("kilo.exe");
    #[cfg(not(target_os = "windows"))]
    let binary = extension.join("bin").join("kilo");
    fs::create_dir_all(binary.parent().unwrap()).unwrap();
    fs::write(&binary, "kilo").unwrap();

    let detected =
        default_detection_path_for_platform("kilo-code", "macos", &home, &app_data).unwrap();
    assert_eq!(detected, extension);

    let bundled =
        binaries::find_kilo_code_extension_cli(&kilo_code_extension_roots(&home)).unwrap();
    assert_eq!(bundled, binary);
}

#[test]
fn kilo_code_uses_running_process_detection() {
    assert!(target_uses_running_process_detection("kilo-code"));
}

#[test]
fn scan_uses_running_process_names_as_local_detection_signal() {
    let dir = temp_test_dir("running-process-target-scan");
    let scan = scan_targets_with_params(
        &port(),
        &json!({
            "stateRoot": display_path(dir.join("client-state")),
            "runningProcessNames": ["openclaw.exe"]
        }),
    )
    .unwrap();

    let openclaw = scan["candidates"]
        .as_array()
        .unwrap()
        .iter()
        .find(|item| item["target"] == "openclaw")
        .unwrap();
    assert_eq!(openclaw["status"], "detected");
    assert!(openclaw["detail"].as_str().unwrap().contains("process:"));
}

#[cfg(target_os = "windows")]
#[test]
fn find_binary_in_dirs_accepts_windows_command_wrappers() {
    let dir = temp_test_dir("windows-command-wrapper");
    let wrapper = dir.join("codex.cmd");
    fs::write(&wrapper, "@echo off\r\n").unwrap();

    let found = binaries::find_binary_in_dirs(&["codex"], &[dir]).unwrap();
    assert!(
        found
            .to_string_lossy()
            .eq_ignore_ascii_case(&wrapper.to_string_lossy())
    );
}
