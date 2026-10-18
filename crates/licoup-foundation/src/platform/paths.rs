use anyhow::{Result, anyhow};
use std::{
    cell::RefCell,
    env,
    ffi::OsString,
    path::{Path, PathBuf},
};

const DATA_HOME_LOCATOR_LIMIT: usize = 32 * 1024;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DataHomeSource {
    Environment,
    LegacyEnvironment,
    Saved,
    Default,
    TestOverride,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DataHomeSelection {
    pub path: PathBuf,
    pub source: DataHomeSource,
}

/// Environment value for an owned login item when the user selected an
/// environment-managed root. Saved and default roots must resolve through the
/// boot locator so a missing volume remains detectable at startup.
pub fn managed_data_home_environment_override(selection: &DataHomeSelection) -> Option<&Path> {
    match selection.source {
        DataHomeSource::Environment | DataHomeSource::LegacyEnvironment => {
            Some(selection.path.as_path())
        }
        DataHomeSource::Saved | DataHomeSource::Default | DataHomeSource::TestOverride => None,
    }
}

thread_local! {
    static PORTABLE_DATA_DIR_OVERRIDE: RefCell<Option<PathBuf>> = const { RefCell::new(None) };
}

#[doc(hidden)]
pub fn set_portable_data_dir_override(path: Option<PathBuf>) -> Option<PathBuf> {
    PORTABLE_DATA_DIR_OVERRIDE.with(|value| value.replace(path))
}

/// The desktop owns the custody helper, while independently supervised
/// sidecars remain in the outer app's executable directory.
pub fn desktop_bundle_for_cli(executable: &Path) -> Option<&Path> {
    if !executable.ends_with("Contents/Helpers/LicoUpCustody.app/Contents/MacOS/licoup-cli") {
        return None;
    }
    executable.ancestors().nth(6).filter(|bundle| {
        bundle
            .extension()
            .is_some_and(|extension| extension == "app")
    })
}

pub fn packaged_binary_directory(executable: &Path) -> Option<PathBuf> {
    match desktop_bundle_for_cli(executable) {
        Some(bundle) => Some(bundle.join("Contents/MacOS")),
        None => executable.parent().map(Path::to_path_buf),
    }
}

/// Resolve the selected LicoUp data root and prepare it for owned writes.
pub fn portable_data_dir() -> Result<PathBuf> {
    let selection = selected_data_home()?;
    if selection.source == DataHomeSource::Saved && !selection.path.is_dir() {
        return Err(anyhow!("saved LicoUp data root is unavailable"));
    }
    prepare_current_root(selection.path)
}

/// Resolve the current LicoUp state root lexically without creating or
/// hardening it. Read-only observers use this before opening existing state.
pub fn portable_data_dir_read_only() -> Result<PathBuf> {
    Ok(selected_data_home()?.path)
}

/// Return the same boot selection used by the GUI, CLI and owned services.
/// The locator is intentionally outside the selected root so it remains
/// available when that root is on an unavailable removable volume.
pub fn selected_data_home() -> Result<DataHomeSelection> {
    if let Some(path) = portable_data_dir_override() {
        return Ok(DataHomeSelection {
            path,
            source: DataHomeSource::TestOverride,
        });
    }
    let explicit = env::var("LICOUP_HOME").ok();
    let legacy = env::var("LICOUP_PORTABLE_DIR").ok();
    if let Some(selection) = environment_data_home(explicit.clone(), legacy.clone()) {
        let current_dir = if selection.path.is_absolute() {
            None
        } else {
            Some(env::current_dir()?)
        };
        return Ok(absolutize_data_home_selection(
            selection,
            current_dir.as_deref(),
        ));
    }
    // Read the locator only when neither process-level selection applies: a
    // broken old locator must not prevent an explicit environment override.
    let saved = read_saved_data_home()?;
    let home =
        user_home_from_env().ok_or_else(|| anyhow!("cannot resolve the LicoUp home directory"))?;
    Ok(select_saved_data_home(
        saved,
        strip_macos_data_volume(&home).join(".lico-up"),
    ))
}

fn environment_data_home(
    explicit: Option<String>,
    legacy: Option<String>,
) -> Option<DataHomeSelection> {
    if let Some(path) = portable_data_dir_from_value(explicit) {
        return Some(DataHomeSelection {
            path,
            source: DataHomeSource::Environment,
        });
    }
    // Published through v0.2.1 and nightly: keep this alias centralized and
    // lower priority than the supported LICOUP_HOME variable.
    if let Some(path) = portable_data_dir_from_value(legacy) {
        return Some(DataHomeSelection {
            path,
            source: DataHomeSource::LegacyEnvironment,
        });
    }
    None
}

fn select_saved_data_home(saved: Option<PathBuf>, default: PathBuf) -> DataHomeSelection {
    if let Some(path) = saved {
        return DataHomeSelection {
            path,
            source: DataHomeSource::Saved,
        };
    }
    DataHomeSelection {
        path: default,
        source: DataHomeSource::Default,
    }
}

fn absolutize_data_home_selection(
    mut selection: DataHomeSelection,
    current_dir: Option<&Path>,
) -> DataHomeSelection {
    if selection.path.is_relative() {
        if let Some(current_dir) = current_dir {
            selection.path = current_dir.join(selection.path);
        }
    }
    selection.path = normalize_path(&selection.path);
    selection
}

fn normalize_path(path: &Path) -> PathBuf {
    use std::path::Component;

    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                if matches!(
                    normalized.components().next_back(),
                    Some(Component::Normal(_))
                ) {
                    normalized.pop();
                } else if !normalized.has_root() {
                    normalized.push(component.as_os_str());
                }
            }
            Component::Prefix(_) | Component::RootDir | Component::Normal(_) => {
                normalized.push(component.as_os_str());
            }
        }
    }
    if normalized.as_os_str().is_empty() {
        PathBuf::from(".")
    } else {
        normalized
    }
}

fn windows_data_home_config(home: &Path, app_data: Option<OsString>) -> PathBuf {
    app_data
        .filter(|path| !path.to_string_lossy().trim().is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| home.join("AppData/Roaming"))
        .join("LicoUp")
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DataHomePlatform {
    Macos,
    Windows,
    Xdg,
}

impl DataHomePlatform {
    pub const fn current() -> Self {
        if cfg!(target_os = "macos") {
            Self::Macos
        } else if cfg!(target_os = "windows") {
            Self::Windows
        } else {
            Self::Xdg
        }
    }
}

/// Pure path policy shared by production resolution and isolated process fixtures.
pub fn data_home_locator_path_for(
    platform: DataHomePlatform,
    home: &Path,
    app_data: Option<OsString>,
    xdg_config_home: Option<OsString>,
) -> PathBuf {
    let config = match platform {
        DataHomePlatform::Macos => home.join("Library/Application Support/LicoUp"),
        DataHomePlatform::Windows => windows_data_home_config(home, app_data),
        DataHomePlatform::Xdg => xdg_config_home
            .map(PathBuf::from)
            .filter(|path| path.is_absolute())
            .unwrap_or_else(|| home.join(".config"))
            .join("licoup"),
    };
    config.join("data-home")
}

/// Location of the small boot locator shared by native processes and Flutter.
pub fn data_home_locator_path() -> Result<PathBuf> {
    let home = user_home_from_env()
        .map(|home| strip_macos_data_volume(&home))
        .ok_or_else(|| anyhow!("cannot resolve the LicoUp home directory"))?;
    Ok(data_home_locator_path_for(
        DataHomePlatform::current(),
        &home,
        env::var_os("APPDATA"),
        env::var_os("XDG_CONFIG_HOME"),
    ))
}

/// Read the private boot locator without creating the selected data root.
pub fn read_saved_data_home() -> Result<Option<PathBuf>> {
    let locator = data_home_locator_path()?;
    read_saved_data_home_at(&locator)
}

fn read_saved_data_home_at(locator: &Path) -> Result<Option<PathBuf>> {
    match std::fs::symlink_metadata(locator) {
        Ok(_) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(_) => return Err(anyhow!("saved LicoUp data root could not be read")),
    }
    let Some(value) = crate::platform::file_security::read_existing_private_text_bounded(
        locator,
        DATA_HOME_LOCATOR_LIMIT,
    )?
    else {
        return Ok(None);
    };
    let value = value.trim();
    if value.is_empty() {
        return Err(anyhow!("saved LicoUp data root is invalid"));
    }
    let path = PathBuf::from(value);
    if !path.is_absolute() {
        return Err(anyhow!("saved LicoUp data root is invalid"));
    }
    Ok(Some(path))
}

/// Atomically select an existing, canonical directory as the LicoUp root.
pub fn save_data_home(path: &Path) -> Result<PathBuf> {
    if !path.is_absolute() || !path.is_dir() {
        return Err(anyhow!("LicoUp data root must be an existing directory"));
    }
    let canonical = path.canonicalize()?;
    let locator = data_home_locator_path()?;
    crate::platform::file_security::atomic_write_private_text_bounded(
        &locator,
        &format!("{}\n", canonical.display()),
        DATA_HOME_LOCATOR_LIMIT,
    )?;
    Ok(canonical)
}

/// Remove only the saved selection. Environment-managed roots remain intact.
pub fn clear_saved_data_home() -> Result<()> {
    let locator = data_home_locator_path()?;
    if locator.exists() {
        crate::platform::file_security::remove_private_state_marker(&locator)?;
    }
    Ok(())
}

fn portable_data_dir_override() -> Option<PathBuf> {
    PORTABLE_DATA_DIR_OVERRIDE.with(|value| value.borrow().clone())
}

#[doc(hidden)]
pub fn portable_data_dir_override_path() -> Option<PathBuf> {
    portable_data_dir_override()
}

fn portable_data_dir_from_value(value: Option<String>) -> Option<PathBuf> {
    let Some(value) = value else {
        return None;
    };
    let trimmed = value.trim();
    if trimmed.is_empty()
        || trimmed.starts_with('$')
        || trimmed.contains("${")
        || trimmed.contains("${env:")
    {
        return None;
    }
    Some(PathBuf::from(trimmed))
}

/// Home from `HOME` / `USERPROFILE` / `HOMEDRIVE`+`HOMEPATH` only.
///
/// Never call `directories::UserDirs` or `directories::BaseDirs` for `$HOME`.
/// Those constructors also assemble Desktop, Documents, Downloads, Pictures,
/// Music, and Movies by joining `$HOME`; that is path construction, not a TCC
/// trigger. Keep this owner anyway so home resolution stays lexical, firmlink-
/// normalized, and independent of that crate.
pub fn user_home_from_env() -> Option<PathBuf> {
    env_home_from(|name| env::var_os(name))
}

/// Drop the macOS data-volume firmlink prefix so a home-relative path and the
/// same path under that prefix classify as the same location.
/// Lexical only; does not stat.
fn macos_data_volume_prefix() -> PathBuf {
    Path::new("/").join("System").join("Volumes").join("Data")
}

pub fn strip_macos_data_volume(path: &Path) -> PathBuf {
    match path.strip_prefix(macos_data_volume_prefix()) {
        Ok(rest) if rest.as_os_str().is_empty() => PathBuf::from("/"),
        Ok(rest) => Path::new("/").join(rest),
        Err(_) => path.to_path_buf(),
    }
}

pub fn env_home_from<F>(var: F) -> Option<PathBuf>
where
    F: Fn(&str) -> Option<OsString>,
{
    if let Some(path) = env_path_from(&var, "HOME") {
        return Some(path);
    }
    if let Some(path) = env_path_from(&var, "USERPROFILE") {
        return Some(path);
    }
    let drive = var("HOMEDRIVE").filter(|value| !value.is_empty())?;
    let path = var("HOMEPATH").filter(|value| !value.is_empty())?;
    let mut combined = drive;
    combined.push(path);
    Some(PathBuf::from(combined))
}

fn env_path_from<F>(var: &F, name: &str) -> Option<PathBuf>
where
    F: Fn(&str) -> Option<OsString>,
{
    var(name)
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
}

#[cfg(test)]
fn home_portable_data_dir_from_home(home: &Path) -> Result<PathBuf> {
    prepare_current_root(home.join(".lico-up"))
}

fn prepare_current_root(path: PathBuf) -> Result<PathBuf> {
    crate::platform::file_security::ensure_private_dir(&path)?;
    Ok(path)
}

/// The directory below the selected data home that holds the managed optional
/// extension package store.
pub const PACKAGE_STORE_DIRECTORY: &str = "extension-packages";

/// The managed package store root inside one already selected data home.
///
/// The layout is owned here so every process that resolves the LicoUp data home
/// resolves the package store the same way, and so a fixture can resolve it over
/// a synthetic root without re-deriving the platform policy. This is pure path
/// policy: it creates and hardens nothing, because the package store applies its
/// own private-directory policy when it opens.
pub fn package_store_root(data_home: &Path) -> PathBuf {
    data_home.join(PACKAGE_STORE_DIRECTORY)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn custody_helper_resolves_outer_app_and_sidecars() {
        let bundle = Path::new("fixture/Custom Client.app");
        let cli = bundle.join("Contents/Helpers/LicoUpCustody.app/Contents/MacOS/licoup-cli");
        assert_eq!(desktop_bundle_for_cli(&cli), Some(bundle));
        assert_eq!(
            packaged_binary_directory(&cli),
            Some(bundle.join("Contents/MacOS"))
        );
        assert_eq!(
            desktop_bundle_for_cli(&bundle.join("Contents/MacOS/licoup-cli")),
            None
        );
        assert_eq!(
            desktop_bundle_for_cli(Path::new(
                "fixture/LicoUpCustody.app/Contents/MacOS/licoup-cli"
            )),
            None
        );
    }

    #[test]
    fn standalone_cli_keeps_its_packaged_siblings() {
        let cli = Path::new("fixture/bin/licoup-cli");
        assert_eq!(desktop_bundle_for_cli(cli), None);
        assert_eq!(
            packaged_binary_directory(cli),
            Some(PathBuf::from("fixture/bin"))
        );
    }

    #[test]
    fn current_override_is_private_and_uses_only_requested_root() {
        let parent = std::env::temp_dir().join(format!("licoup-paths-{}", uuid::Uuid::new_v4()));
        let current = parent.join("current");
        let retired = parent.join("retired");
        std::fs::create_dir_all(&retired).unwrap();
        let sentinel = retired.join("must-not-be-read-or-modified");
        std::fs::write(&sentinel, b"retired").unwrap();
        let _guard = PortableDataDirOverrideGuard::set(current.clone());

        let resolved = portable_data_dir().unwrap();

        assert_eq!(resolved, current);
        assert_eq!(std::fs::read(&sentinel).unwrap(), b"retired");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                resolved.metadata().unwrap().permissions().mode() & 0o777,
                0o700
            );
        }
        let _ = std::fs::remove_dir_all(parent);
    }

    #[test]
    fn current_namespace_is_home_dot_lico_up() {
        let parent = std::env::temp_dir().join(format!("licoup-base-{}", uuid::Uuid::new_v4()));

        let resolved = home_portable_data_dir_from_home(&parent).unwrap();

        assert_eq!(resolved, parent.join(".lico-up"));
        let _ = std::fs::remove_dir_all(parent);
    }

    #[test]
    fn blank_current_override_does_not_select_a_path() {
        assert_eq!(portable_data_dir_from_value(Some("   ".to_string())), None);
    }

    #[test]
    fn data_home_selection_obeys_environment_alias_saved_default_order() {
        let default = PathBuf::from("/fixture/default");
        let saved = PathBuf::from("/fixture/saved");
        let legacy = environment_data_home(None, Some("/fixture/legacy".to_string())).unwrap();
        assert_eq!(legacy.path, PathBuf::from("/fixture/legacy"));
        assert_eq!(legacy.source, DataHomeSource::LegacyEnvironment);

        let explicit = environment_data_home(
            Some("/fixture/explicit".to_string()),
            Some("/fixture/legacy".to_string()),
        )
        .unwrap();
        assert_eq!(explicit.path, PathBuf::from("/fixture/explicit"));
        assert_eq!(explicit.source, DataHomeSource::Environment);

        let stored = select_saved_data_home(Some(saved.clone()), default.clone());
        assert_eq!(stored.path, saved);
        assert_eq!(stored.source, DataHomeSource::Saved);

        let fallback = select_saved_data_home(None, default.clone());
        assert_eq!(fallback.path, default);
        assert_eq!(fallback.source, DataHomeSource::Default);

        assert_eq!(
            managed_data_home_environment_override(&legacy),
            Some(Path::new("/fixture/legacy"))
        );
        assert_eq!(
            managed_data_home_environment_override(&explicit),
            Some(Path::new("/fixture/explicit"))
        );
        assert_eq!(managed_data_home_environment_override(&stored), None);
        assert_eq!(managed_data_home_environment_override(&fallback), None);
    }

    #[test]
    fn relative_environment_roots_become_absolute_without_changing_source() {
        let cwd = std::env::temp_dir()
            .canonicalize()
            .unwrap()
            .join("fixture-current-directory");
        let expected_root = cwd.parent().unwrap().join("licoup-data");
        for (selection, expected, source) in [
            (
                environment_data_home(Some("../licoup-data/./new-root".to_string()), None).unwrap(),
                expected_root.join("new-root"),
                DataHomeSource::Environment,
            ),
            (
                environment_data_home(None, Some("../licoup-data/./legacy-root".to_string()))
                    .unwrap(),
                expected_root.join("legacy-root"),
                DataHomeSource::LegacyEnvironment,
            ),
        ] {
            let selection = absolutize_data_home_selection(selection, Some(&cwd));
            assert!(selection.path.is_absolute());
            assert_eq!(selection.path, expected);
            assert_eq!(selection.source, source);
        }
    }

    #[cfg(unix)]
    #[test]
    fn missing_locator_read_is_read_only_and_existing_locator_keeps_private_validation() {
        use std::os::unix::fs::PermissionsExt;

        let home =
            std::env::temp_dir().join(format!("licoup-home-bootstrap-{}", uuid::Uuid::new_v4()));
        let config = home.join(".config/licoup");

        // A first-start CLI can resolve its default without creating the
        // otherwise absent GUI/configuration namespace.
        let absent_parent_locator = config.join("data-home");
        let absent_parent_saved = read_saved_data_home_at(&absent_parent_locator).unwrap();
        let absent_parent_selection =
            select_saved_data_home(absent_parent_saved, home.join(".lico-up"));
        assert_eq!(absent_parent_selection.source, DataHomeSource::Default);
        assert_eq!(absent_parent_selection.path, home.join(".lico-up"));
        assert!(!config.exists());

        // The GUI may have created its boot lock under the process umask.
        // Resolving a missing locator must neither harden nor otherwise alter
        // that existing directory.
        std::fs::create_dir_all(&config).unwrap();
        std::fs::set_permissions(&config, std::fs::Permissions::from_mode(0o755)).unwrap();
        std::fs::write(config.join("client.instance.lock"), b"").unwrap();
        let locator = config.join("data-home");

        let saved = read_saved_data_home_at(&locator).unwrap();
        let selected = select_saved_data_home(saved, home.join(".lico-up"));

        assert_eq!(selected.source, DataHomeSource::Default);
        assert_eq!(selected.path, home.join(".lico-up"));
        assert_eq!(
            std::fs::metadata(&config).unwrap().permissions().mode() & 0o777,
            0o755
        );

        // Existing locator content remains subject to the established
        // private-parent/file checks.
        std::fs::write(&locator, b"/fixture/saved-root\n").unwrap();
        assert!(read_saved_data_home_at(&locator).is_err());
        assert_eq!(
            std::fs::metadata(&config).unwrap().permissions().mode() & 0o777,
            0o755
        );

        let _ = std::fs::remove_dir_all(home);
    }

    #[test]
    fn blank_windows_appdata_uses_the_home_based_locator_directory() {
        let home = Path::new("/fixture/home");
        assert_eq!(
            windows_data_home_config(home, Some(OsString::new())),
            home.join("AppData/Roaming/LicoUp")
        );
        assert_eq!(
            windows_data_home_config(home, Some(OsString::from("  "))),
            home.join("AppData/Roaming/LicoUp")
        );
        assert_eq!(
            windows_data_home_config(home, Some(OsString::from("/fixture/appdata"))),
            PathBuf::from("/fixture/appdata/LicoUp")
        );
    }

    #[test]
    fn locator_policy_uses_each_platform_owner_directory() {
        let fixture = std::env::temp_dir()
            .canonicalize()
            .unwrap()
            .join("licoup-locator-policy");
        let home = fixture.join("home");
        let home = home.as_path();
        let app_data = fixture.join("appdata");
        let config = fixture.join("config");
        for (platform, expected) in [
            (
                DataHomePlatform::Macos,
                home.join("Library/Application Support/LicoUp/data-home"),
            ),
            (DataHomePlatform::Windows, app_data.join("LicoUp/data-home")),
            (DataHomePlatform::Xdg, config.join("licoup/data-home")),
        ] {
            assert_eq!(
                data_home_locator_path_for(
                    platform,
                    home,
                    Some(app_data.clone().into_os_string()),
                    Some(config.clone().into_os_string())
                ),
                expected
            );
        }
        assert_eq!(
            data_home_locator_path_for(DataHomePlatform::Xdg, home, None, Some("relative".into())),
            home.join(".config/licoup/data-home")
        );
    }

    #[test]
    fn blank_or_unexpanded_environment_values_do_not_override_saved_selection() {
        let saved = PathBuf::from("/fixture/saved");
        for value in ["   ", "$LICOUP_HOME", "${LICOUP_HOME}"] {
            assert_eq!(portable_data_dir_from_value(Some(value.to_string())), None);
        }
        let selection =
            select_saved_data_home(Some(saved.clone()), PathBuf::from("/fixture/default"));
        assert_eq!(selection.path, saved);
        assert_eq!(selection.source, DataHomeSource::Saved);
    }

    #[test]
    fn unexpanded_interpolation_does_not_select_a_path() {
        for value in [
            "${LICOUP_PORTABLE_DIR}",
            "$LICOUP_PORTABLE_DIR",
            "${env:LICOUP_PORTABLE_DIR}",
        ] {
            assert_eq!(portable_data_dir_from_value(Some(value.to_string())), None);
        }
    }

    #[test]
    fn macos_firmlink_prefix_does_not_change_home_relative_classification() {
        fn posix(parts: &[&str]) -> PathBuf {
            PathBuf::from(format!("/{}", parts.join("/")))
        }
        assert_eq!(
            strip_macos_data_volume(&posix(&[
                "System", "Volumes", "Data", "profile", "fixture", "Desktop"
            ])),
            posix(&["profile", "fixture", "Desktop"])
        );
        assert_eq!(
            strip_macos_data_volume(&posix(&["Users", "fixture", "Documents"])),
            posix(&["Users", "fixture", "Documents"])
        );
        assert_eq!(
            strip_macos_data_volume(&posix(&["System", "Volumes", "Data"])),
            PathBuf::from("/")
        );
    }

    #[test]
    fn home_comes_from_environment_variables_not_user_dirs() {
        let separator = char::from(92).to_string();
        let home_path = ["", "Profile", "Arc"].join(&separator);
        let home = env_home_from(|name| match name {
            "HOMEDRIVE" => Some(OsString::from("C:")),
            "HOMEPATH" => Some(OsString::from(&home_path)),
            _ => None,
        });
        assert_eq!(
            home,
            Some(PathBuf::from(["C:", "Profile", "Arc"].join(&separator)))
        );
        assert_eq!(env_home_from(|_| None), None);
    }

    /// The package store is a layout decision of this owner, so a synthetic
    /// data home resolves it without re-deriving platform policy, and resolving
    /// it creates nothing.
    #[test]
    fn the_package_store_root_is_one_directory_below_the_data_home() {
        let data_home = Path::new("fixture/data-home");
        assert_eq!(
            package_store_root(data_home),
            data_home.join("extension-packages")
        );
        assert_eq!(PACKAGE_STORE_DIRECTORY, "extension-packages");
        assert!(!package_store_root(data_home).exists());
    }

    struct PortableDataDirOverrideGuard {
        previous: Option<PathBuf>,
    }

    impl PortableDataDirOverrideGuard {
        fn set(path: PathBuf) -> Self {
            let previous = set_portable_data_dir_override(Some(path));
            Self { previous }
        }
    }

    impl Drop for PortableDataDirOverrideGuard {
        fn drop(&mut self) {
            set_portable_data_dir_override(self.previous.take());
        }
    }
}
