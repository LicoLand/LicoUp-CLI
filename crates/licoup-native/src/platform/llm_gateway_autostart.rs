//! OS login autostart for the loopback LLM Gateway sidecar.
//!
//! Installs a per-user launch item that runs
//! `licoup-cli llm-gateway service start` at login. The Gateway starts
//! disconnected (no Keychain handoff) until the user authorizes in the app.
//! Credentials are never stored in the launch item.
//!
//! This module is the only owner of that login item. Two callers reach it: the
//! user's own enable/disable commands, and the Gateway package lifecycle, which
//! reaches it through [`apply_gateway_package_lifecycle`] — the package routes
//! register the item when the package activates and remove it when the package
//! is disabled or uninstalled. Both go through [`LoginItemHost`], so the
//! lifecycle adds no second registration path.

use anyhow::{Result, anyhow, bail, ensure};
use licoup_foundation::platform::file_security::{
    atomic_write_private_text, ensure_private_dir, read_private_text_bounded,
};
use licoup_foundation::platform::paths;
use serde_json::{Value, json};
use std::cell::RefCell;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

pub const AUTOSTART_SCHEMA: &str = "licoup.llm-gateway-autostart.v1";
const LABEL: &str = "land.lico.licoup.llm-gateway";
const MAX_MARKER_BYTES: usize = 4 * 1024;
const STATE_DIRECTORY: &str = "llm-gateway";

#[derive(Clone, Debug)]
struct AutostartMarker {
    enabled: bool,
    port: u16,
    program: String,
}

impl AutostartMarker {
    fn to_json(&self) -> Value {
        json!({
            "schemaVersion": AUTOSTART_SCHEMA,
            "enabled": self.enabled,
            "port": self.port,
            "program": self.program,
        })
    }

    fn from_json(value: &Value) -> Option<Self> {
        let object = value.as_object()?;
        if object.get("schemaVersion").and_then(Value::as_str) != Some(AUTOSTART_SCHEMA) {
            return None;
        }
        Some(Self {
            enabled: object.get("enabled").and_then(Value::as_bool)?,
            port: object
                .get("port")
                .and_then(Value::as_u64)
                .and_then(|value| u16::try_from(value).ok().filter(|port| *port != 0))?,
            program: object
                .get("program")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_owned(),
        })
    }
}

/// Where one user's Gateway login item lives, and how installing it becomes
/// effective.
///
/// The production host is [`LoginItemHost::for_current_user`]: the real home
/// directory, the current data root, this executable, and the platform's own
/// registration command. A synthetic host points every path at a disposable
/// root and writes the definition without registering it, so the package
/// lifecycle can be exercised and asserted without touching the user's own
/// login items or invoking `launchctl`/`systemctl`.
#[derive(Clone, Debug)]
pub struct LoginItemHost {
    home: PathBuf,
    state_directory: PathBuf,
    program: PathBuf,
    registration: LoginItemRegistration,
}

/// How a written login-item definition becomes effective.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LoginItemRegistration {
    /// Hand the definition to the platform's own supervisor.
    Platform,
    /// Write the definition only. The definition file is the registration this
    /// host reports, and nothing is launched.
    FileOnly,
}

impl LoginItemHost {
    /// The host the current user's own login item belongs to.
    pub fn for_current_user() -> Result<Self> {
        Ok(Self {
            home: user_home()?,
            state_directory: state_dir()?,
            program: cli_program_path()?,
            registration: LoginItemRegistration::Platform,
        })
    }

    /// A host over explicit paths that registers nothing.
    pub fn synthetic(
        home: impl Into<PathBuf>,
        state_directory: impl Into<PathBuf>,
        program: impl Into<PathBuf>,
    ) -> Self {
        Self {
            home: home.into(),
            state_directory: state_directory.into(),
            program: program.into(),
            registration: LoginItemRegistration::FileOnly,
        }
    }

    pub fn home(&self) -> &Path {
        &self.home
    }

    pub fn state_directory(&self) -> &Path {
        &self.state_directory
    }

    pub fn program(&self) -> &Path {
        &self.program
    }

    /// Whether installing here also loads the definition with the platform.
    pub fn registers_with_platform(&self) -> bool {
        self.registration == LoginItemRegistration::Platform
    }

    fn marker_path(&self) -> Result<PathBuf> {
        ensure_private_dir(&self.state_directory)?;
        Ok(self.state_directory.join("autostart.json"))
    }

    fn log_path(&self) -> Result<PathBuf> {
        ensure_private_dir(&self.state_directory)?;
        Ok(self.state_directory.join("autostart.log"))
    }

    fn read_marker(&self) -> Result<Option<AutostartMarker>> {
        let path = self.marker_path()?;
        let Some(text) = read_private_text_bounded(&path, MAX_MARKER_BYTES)? else {
            return Ok(None);
        };
        let value: Value = serde_json::from_str(&text).unwrap_or(Value::Null);
        Ok(AutostartMarker::from_json(&value))
    }

    fn write_marker(&self, marker: &AutostartMarker) -> Result<()> {
        let path = self.marker_path()?;
        atomic_write_private_text(&path, &serde_json::to_string_pretty(&marker.to_json())?)?;
        Ok(())
    }

    fn clear_marker(&self) -> Result<()> {
        let path = self.marker_path()?;
        if path.is_file() {
            fs::remove_file(&path).map_err(|_| anyhow!("llm_gateway_autostart_clear_failed"))?;
        }
        Ok(())
    }

    /// The definition file this host writes and removes.
    ///
    /// It is public because the uninstall proof asserts the registration is
    /// gone from its own path, not merely that the status document says so.
    pub fn definition_path(&self) -> Result<PathBuf> {
        platform_definition_path(self)
    }

    fn install(&self, port: u16) -> Result<()> {
        platform_install(self, port)
    }

    fn uninstall(&self) -> Result<()> {
        platform_uninstall(self)
    }

    fn installed(&self) -> Result<bool> {
        platform_installed(self)
    }

    fn configured_port(&self) -> Result<Option<u16>> {
        platform_configured_port(self)
    }
}

fn state_dir() -> Result<PathBuf> {
    let root = paths::portable_data_dir()?.join(STATE_DIRECTORY);
    ensure_private_dir(&root)?;
    Ok(root)
}

fn cli_program_path() -> Result<PathBuf> {
    let current =
        std::env::current_exe().map_err(|_| anyhow!("llm_gateway_autostart_cli_missing"))?;
    let metadata =
        fs::symlink_metadata(&current).map_err(|_| anyhow!("llm_gateway_autostart_cli_missing"))?;
    ensure!(
        metadata.file_type().is_file(),
        "llm_gateway_autostart_cli_missing"
    );
    fs::canonicalize(&current).map_err(|_| anyhow!("llm_gateway_autostart_cli_missing"))
}

/// Report whether login autostart is installed for the current user.
pub fn autostart_status() -> Result<Value> {
    autostart_status_at(&LoginItemHost::for_current_user()?)
}

/// Report whether login autostart is installed for one host.
pub fn autostart_status_at(host: &LoginItemHost) -> Result<Value> {
    let marker = host.read_marker()?;
    let installed = host.installed()?;
    let enabled = marker.as_ref().is_some_and(|value| value.enabled) && installed;
    Ok(json!({
        "ok": true,
        "schemaVersion": AUTOSTART_SCHEMA,
        "supported": platform_supported(),
        "enabled": enabled,
        "installed": installed,
        "port": marker.as_ref().map(|value| value.port),
        "program": marker.as_ref().map(|value| value.program.clone()).unwrap_or_default(),
        "label": LABEL,
    }))
}

/// Install and load the per-user login item that starts the Gateway alone.
pub fn autostart_enable(port: u16) -> Result<Value> {
    autostart_enable_at(&LoginItemHost::for_current_user()?, port)
}

/// Install the login item of one host.
pub fn autostart_enable_at(host: &LoginItemHost, port: u16) -> Result<Value> {
    ensure!(port != 0, "llm_gateway_port_invalid");
    if !platform_supported() {
        bail!("llm_gateway_autostart_unsupported");
    }
    host.install(port)?;
    host.write_marker(&AutostartMarker {
        enabled: true,
        port,
        program: host.program().to_string_lossy().into_owned(),
    })?;
    autostart_status_at(host)
}

/// Unload and remove the per-user login item.
pub fn autostart_disable() -> Result<Value> {
    autostart_disable_at(&LoginItemHost::for_current_user()?)
}

/// Unload and remove the login item of one host.
pub fn autostart_disable_at(host: &LoginItemHost) -> Result<Value> {
    if !platform_supported() {
        bail!("llm_gateway_autostart_unsupported");
    }
    host.uninstall()?;
    host.clear_marker()?;
    autostart_status_at(host)
}

/// Rewrites an installed Gateway login definition after saved-root recovery.
/// The service definition is the surviving enablement record while the
/// selected data volume is unavailable.
pub fn refresh_after_data_home_recovery() -> Result<()> {
    if !platform_supported() {
        return Ok(());
    }
    let host = LoginItemHost::for_current_user()?;
    refresh_gateway_autostart(host.configured_port()?, |port| {
        autostart_enable_at(&host, port).map(|_| ())
    })
}

fn refresh_gateway_autostart(
    configured_port: Option<u16>,
    mut enable: impl FnMut(u16) -> Result<()>,
) -> Result<()> {
    if let Some(port) = configured_port {
        enable(port)?;
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// The Gateway package lifecycle
// ---------------------------------------------------------------------------

/// The optional package whose payload is the Gateway Runtime.
pub const GATEWAY_PACKAGE_ID: &str = "org.licoland.feature.gateway";

/// The Gateway login item, bound to the installed package generation.
///
/// This is the seam the package command family reaches:
/// [`GatewayPackageBinding::activate`] after an install/enable,
/// [`GatewayPackageBinding::retire`] **before** an uninstall removes the
/// installed bytes. Both go through the login item owner above, so the package
/// lifecycle adds no second registration path.
///
/// The binding resolves the program the definition names from the package's own
/// installed manifest: the entry this package ships is what starts at login.
/// Nothing here falls back to a file next to this executable, and an absent or
/// removed package registers no login item at all.
#[derive(Clone, Debug)]
pub struct GatewayPackageBinding {
    host: LoginItemHost,
    store_root: PathBuf,
}

impl GatewayPackageBinding {
    /// The binding over an explicit login-item host and managed package root.
    pub fn over(host: LoginItemHost, store_root: impl Into<PathBuf>) -> Self {
        Self {
            host,
            store_root: store_root.into(),
        }
    }

    /// The binding over the running user's own login item and data home.
    pub fn for_current_user() -> Result<Self> {
        Ok(Self {
            host: LoginItemHost::for_current_user()?,
            store_root: package_store_root(&paths::portable_data_dir()?),
        })
    }

    pub fn host(&self) -> &LoginItemHost {
        &self.host
    }

    pub fn store_root(&self) -> &Path {
        &self.store_root
    }

    fn store(&self) -> Result<crate::platform::extension_packages::PackageStore> {
        crate::platform::extension_packages::PackageStore::open(&self.store_root)
            .map_err(|_| anyhow!("llm_gateway_package_store_unavailable"))
    }

    /// The installed version of the Gateway package, when one is present.
    pub fn installed(&self) -> Result<Option<String>> {
        let store = self.store()?;
        let installed = store
            .installed()
            .map_err(|_| anyhow!("llm_gateway_package_store_unavailable"))?;
        Ok(installed
            .into_iter()
            .filter(|package| package.package_id == GATEWAY_PACKAGE_ID)
            .map(|package| package.version)
            .next())
    }

    /// The entry the installed package's own manifest declares.
    ///
    /// The package lifecycle answers "is the Gateway installed" from this: a
    /// package whose payload does not carry the entry its manifest names would
    /// register a login item that starts nothing, so the missing entry is a
    /// refusal rather than a registration.
    pub fn program(&self) -> Result<PathBuf> {
        let store = self.store()?;
        let version = self
            .installed()?
            .ok_or_else(|| anyhow!("llm_gateway_package_absent"))?;
        let manifest = store
            .installed_manifest(GATEWAY_PACKAGE_ID, &version)
            .map_err(|_| anyhow!("llm_gateway_package_absent"))?;
        let licoup_extension_contracts::manifest::Runtime::Process { entry, .. } = manifest.runtime
        else {
            bail!("llm_gateway_package_entry_missing");
        };
        ensure!(!entry.is_empty(), "llm_gateway_package_entry_missing");
        let program = store
            .installed_path(GATEWAY_PACKAGE_ID, &version)
            .join(entry);
        ensure!(
            fs::symlink_metadata(&program).is_ok_and(|metadata| metadata.file_type().is_file()),
            "llm_gateway_package_entry_missing"
        );
        Ok(program)
    }

    /// Register the login item of the installed package.
    ///
    /// Refuses when the package is absent or its declared entry is not in the
    /// installed payload: the package is what the login item starts, so there is
    /// nothing to register without it. The definition itself is the one login
    /// item this module has always written — the same program and the same
    /// `service start` arguments — so activation adds no second registration
    /// path.
    pub fn activate(&self, port: u16) -> Result<Value> {
        // The entry is measured before anything is written, not because the
        // definition names it, but because a payload missing it means the
        // companion binary the login item reaches is not the installed one.
        let _ = self.program()?;
        autostart_enable_at(&self.host, port)
    }

    /// Remove the login item before the installed bytes go away.
    pub fn retire(&self) -> Result<Value> {
        if !self.host.installed()? {
            return autostart_status_at(&self.host);
        }
        autostart_disable_at(&self.host)
    }

    /// Whether this client currently registers a login item for the Gateway.
    pub fn status(&self) -> Result<Value> {
        autostart_status_at(&self.host)
    }
}

// ---------------------------------------------------------------------------
// The transition one package route performs
// ---------------------------------------------------------------------------

/// The lifecycle transition one package route performs on the Gateway login item.
///
/// An install and an enable both make the installed package the thing the login
/// item starts; a disable and an uninstall both withdraw it. The pair mirrors
/// the endpoint-collaboration package's own gate, so one package's lifecycle
/// never speaks for a neighbour's.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum GatewayPackageLifecycle {
    /// An install or an enable: the installed package's own login item.
    Activated,
    /// A disable or an uninstall: no login item at all.
    Retired,
}

thread_local! {
    /// The login-item host this thread's package routes write through, when a
    /// test installed one.
    static PACKAGE_LIFECYCLE_HOST: RefCell<Option<LoginItemHost>> = const { RefCell::new(None) };
}

/// Point this thread's Gateway package lifecycle at an explicit login-item host.
///
/// Production never calls this: a package route reaches the running user's own
/// login item. A test installs a [`LoginItemHost::synthetic`] host over a
/// disposable root, because the endpoint-collaboration gate this shape follows
/// is one in-memory answer a test can read directly, while this transition
/// writes a real per-user login item and asks the platform supervisor to load
/// it. The previous host is returned and restored by the caller, exactly as
/// `paths::set_portable_data_dir_override` is.
#[doc(hidden)]
pub fn set_gateway_package_host_override(host: Option<LoginItemHost>) -> Option<LoginItemHost> {
    PACKAGE_LIFECYCLE_HOST.with(|slot| slot.replace(host))
}

fn gateway_package_host() -> Result<LoginItemHost> {
    match PACKAGE_LIFECYCLE_HOST.with(|slot| slot.borrow().clone()) {
        Some(host) => Ok(host),
        None => LoginItemHost::for_current_user(),
    }
}

/// Apply one package lifecycle transition over the Gateway's own login item.
///
/// `store_root` is the managed root the route just changed and `package_id` is
/// the package that operation was about. A route about another package changes
/// nothing here and returns `None`: one package's lifecycle never speaks for a
/// neighbour's login item, and the current user's host is not even resolved for
/// it.
///
/// `port` is the port the item names. `None` keeps the port this client already
/// configured, and answers the Gateway Runtime's own default when there is none,
/// so an activation that follows the user's own switch never silently moves the
/// endpoint that switch configured.
///
/// Unlike the endpoint-collaboration gate, a transition here can refuse: the
/// item is a login item the platform has to accept, not an in-memory answer.
pub fn apply_gateway_package_lifecycle(
    store_root: &Path,
    package_id: &str,
    transition: GatewayPackageLifecycle,
    port: Option<u16>,
) -> Option<Result<Value>> {
    if package_id != GATEWAY_PACKAGE_ID {
        return None;
    }
    Some(transition_over(store_root, transition, port))
}

fn transition_over(
    store_root: &Path,
    transition: GatewayPackageLifecycle,
    port: Option<u16>,
) -> Result<Value> {
    let binding = GatewayPackageBinding::over(gateway_package_host()?, store_root);
    match transition {
        GatewayPackageLifecycle::Activated => {
            // An activation over a store that holds nothing registers nothing.
            // The item's absence is the truthful answer for a version that is
            // gone, and it is what a route that removed the last installed
            // version has to leave behind.
            if binding.installed()?.is_none() {
                return binding.retire();
            }
            let port = port.unwrap_or_else(|| configured_activation_port(binding.host()));
            binding.activate(port)
        }
        GatewayPackageLifecycle::Retired => {
            // The port is read before the withdrawal removes the definition that
            // carries it, and reported with the withdrawal, so a version that
            // remains installed is restored on the port this client configured
            // rather than moved to the default.
            let withdrawn = binding.host().configured_port().ok().flatten();
            binding.retire().map(|mut status| {
                if let Some(object) = status.as_object_mut() {
                    object.insert("withdrawnPort".to_owned(), json!(withdrawn));
                }
                status
            })
        }
    }
}

/// The port a package-driven activation names when the route supplies none: the
/// one this client's login item already carries, or the port the client starts
/// the Gateway Runtime on. The package's own manifest declares no port, so the
/// Runtime's default is the only value the package's bytes could answer with.
fn configured_activation_port(host: &LoginItemHost) -> u16 {
    host.configured_port()
        .ok()
        .flatten()
        .unwrap_or(crate::platform::llm_gateway_service::DEFAULT_PORT)
}

/// The managed root of installed optional packages inside one data home.
///
/// SEAM(PIPELINE-COMMANDS): that node places the package store root inside the
/// selected data home layout and owns this path. Replacing this one function is
/// the whole change here; every read above goes through the store's own API, so
/// no second layout is invented.
pub fn package_store_root(data_home: &Path) -> PathBuf {
    data_home.join("extension-packages")
}

fn platform_supported() -> bool {
    cfg!(target_os = "macos") || cfg!(target_os = "linux")
}

fn user_home() -> Result<PathBuf> {
    paths::user_home_from_env().ok_or_else(|| anyhow!("llm_gateway_autostart_home_missing"))
}

/// The selected data root a written definition must carry, when it carries one.
///
/// The production host records the user's environment-managed selection so the
/// login item starts against the same root as the app. A synthetic host is not
/// registered at all, so its definition names its own disposable root instead of
/// leaking the developer's real selection into a temporary file.
#[cfg(any(target_os = "macos", target_os = "linux"))]
fn managed_data_home_of(host: &LoginItemHost) -> Result<Option<PathBuf>> {
    if !host.registers_with_platform() {
        return Ok(host.state_directory().parent().map(Path::to_path_buf));
    }
    let selection = paths::selected_data_home()?;
    Ok(paths::managed_data_home_environment_override(&selection).map(Path::to_path_buf))
}

#[cfg(target_os = "macos")]
fn launch_agents_dir(host: &LoginItemHost) -> Result<PathBuf> {
    let directory = host.home().join("Library/LaunchAgents");
    fs::create_dir_all(&directory).map_err(|_| anyhow!("llm_gateway_autostart_install_failed"))?;
    Ok(directory)
}

#[cfg(target_os = "macos")]
fn platform_definition_path(host: &LoginItemHost) -> Result<PathBuf> {
    Ok(launch_agents_dir(host)?.join(format!("{LABEL}.plist")))
}

#[cfg(target_os = "macos")]
fn platform_installed(host: &LoginItemHost) -> Result<bool> {
    Ok(platform_definition_path(host)?.is_file())
}

#[cfg(target_os = "macos")]
fn platform_configured_port(host: &LoginItemHost) -> Result<Option<u16>> {
    let path = platform_definition_path(host)?;
    if !path.is_file() {
        return Ok(None);
    }
    let contents =
        fs::read_to_string(path).map_err(|_| anyhow!("llm_gateway_autostart_state_unavailable"))?;
    let port = gateway_port_from_plist(&contents)
        .ok_or_else(|| anyhow!("llm_gateway_autostart_state_unavailable"))?;
    Ok(Some(port))
}

#[cfg(target_os = "macos")]
fn platform_install(host: &LoginItemHost, port: u16) -> Result<()> {
    let log_path = host.log_path()?;
    let environment_variables = managed_data_home_of(host)?
        .map(|portable| {
            format!(
                "  <key>EnvironmentVariables</key>\n  <dict>\n    <key>LICOUP_HOME</key>\n    <string>{}</string>\n  </dict>\n",
                xml_escape(&portable.to_string_lossy())
            )
        })
        .unwrap_or_default();
    let plist = format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>Label</key>
  <string>{LABEL}</string>
  <key>ProgramArguments</key>
  <array>
    <string>{}</string>
    <string>llm-gateway</string>
    <string>service</string>
    <string>start</string>
    <string>--port</string>
    <string>{port}</string>
  </array>
{environment_variables}
  <key>RunAtLoad</key>
  <true/>
  <key>KeepAlive</key>
  <false/>
  <key>ProcessType</key>
  <string>Background</string>
  <key>StandardOutPath</key>
  <string>{}</string>
  <key>StandardErrorPath</key>
  <string>{}</string>
</dict>
</plist>
"#,
        xml_escape(&host.program().to_string_lossy()),
        xml_escape(&log_path.to_string_lossy()),
        xml_escape(&log_path.to_string_lossy()),
    );
    let path = platform_definition_path(host)?;
    atomic_write_private_text(&path, &plist)?;
    if !host.registers_with_platform() {
        return Ok(());
    }
    // Replace any previous registration, then bootstrap the new definition.
    let _ = launchctl_bootout();
    let status = Command::new("/bin/launchctl")
        .args([
            "bootstrap",
            &gui_domain()?,
            path.to_str().unwrap_or_default(),
        ])
        .status()
        .map_err(|_| anyhow!("llm_gateway_autostart_install_failed"))?;
    ensure!(status.success(), "llm_gateway_autostart_install_failed");
    Ok(())
}

#[cfg(target_os = "macos")]
fn platform_uninstall(host: &LoginItemHost) -> Result<()> {
    if host.registers_with_platform() {
        let _ = launchctl_bootout();
    }
    let path = platform_definition_path(host)?;
    if path.is_file() {
        fs::remove_file(&path).map_err(|_| anyhow!("llm_gateway_autostart_clear_failed"))?;
    }
    Ok(())
}

#[cfg(target_os = "macos")]
fn launchctl_bootout() -> Result<()> {
    let _ = Command::new("/bin/launchctl")
        .args(["bootout", &format!("{}/{}", gui_domain()?, LABEL)])
        .status();
    Ok(())
}

#[cfg(target_os = "macos")]
fn gui_domain() -> Result<String> {
    let uid = unsafe { libc::getuid() };
    Ok(format!("gui/{uid}"))
}

#[cfg(target_os = "linux")]
fn systemd_unit_path(host: &LoginItemHost) -> Result<PathBuf> {
    let directory = host.home().join(".config/systemd/user");
    fs::create_dir_all(&directory).map_err(|_| anyhow!("llm_gateway_autostart_install_failed"))?;
    Ok(directory.join("lico-llm-gateway.service"))
}

#[cfg(target_os = "linux")]
fn platform_definition_path(host: &LoginItemHost) -> Result<PathBuf> {
    systemd_unit_path(host)
}

#[cfg(target_os = "linux")]
fn platform_installed(host: &LoginItemHost) -> Result<bool> {
    Ok(systemd_unit_path(host)?.is_file())
}

#[cfg(target_os = "linux")]
fn platform_configured_port(host: &LoginItemHost) -> Result<Option<u16>> {
    let path = systemd_unit_path(host)?;
    if !path.is_file() {
        return Ok(None);
    }
    let contents =
        fs::read_to_string(path).map_err(|_| anyhow!("llm_gateway_autostart_state_unavailable"))?;
    let port = gateway_port_from_systemd(&contents)
        .ok_or_else(|| anyhow!("llm_gateway_autostart_state_unavailable"))?;
    Ok(Some(port))
}

#[cfg(target_os = "linux")]
fn platform_install(host: &LoginItemHost, port: u16) -> Result<()> {
    let environment = managed_data_home_of(host)?
        .map(|portable| {
            format!(
                "Environment=LICOUP_HOME={}\n",
                shell_escape(&portable.to_string_lossy())
            )
        })
        .unwrap_or_default();
    let unit = format!(
        "[Unit]\nDescription=LicoUp LLM Gateway\n\n[Service]\nType=oneshot\nRemainAfterExit=yes\n{environment}ExecStart={} llm-gateway service start --port {port}\nExecStop={} llm-gateway service stop --port {port}\n\n[Install]\nWantedBy=default.target\n",
        shell_escape(&host.program().to_string_lossy()),
        shell_escape(&host.program().to_string_lossy()),
    );
    let path = systemd_unit_path(host)?;
    atomic_write_private_text(&path, &unit)?;
    if !host.registers_with_platform() {
        return Ok(());
    }
    let reload = Command::new("systemctl")
        .args(["--user", "daemon-reload"])
        .status()
        .map_err(|_| anyhow!("llm_gateway_autostart_install_failed"))?;
    ensure!(reload.success(), "llm_gateway_autostart_install_failed");
    let enable = Command::new("systemctl")
        .args(["--user", "enable", "lico-llm-gateway.service"])
        .status()
        .map_err(|_| anyhow!("llm_gateway_autostart_install_failed"))?;
    ensure!(enable.success(), "llm_gateway_autostart_install_failed");
    Ok(())
}

#[cfg(target_os = "linux")]
fn platform_uninstall(host: &LoginItemHost) -> Result<()> {
    if host.registers_with_platform() {
        let _ = Command::new("systemctl")
            .args(["--user", "disable", "--now", "lico-llm-gateway.service"])
            .status();
    }
    let path = systemd_unit_path(host)?;
    if path.is_file() {
        fs::remove_file(&path).map_err(|_| anyhow!("llm_gateway_autostart_clear_failed"))?;
    }
    if host.registers_with_platform() {
        let _ = Command::new("systemctl")
            .args(["--user", "daemon-reload"])
            .status();
    }
    Ok(())
}

#[cfg(not(any(target_os = "macos", target_os = "linux")))]
fn platform_definition_path(_host: &LoginItemHost) -> Result<PathBuf> {
    bail!("llm_gateway_autostart_unsupported")
}

#[cfg(not(any(target_os = "macos", target_os = "linux")))]
fn platform_installed(_host: &LoginItemHost) -> Result<bool> {
    Ok(false)
}

#[cfg(not(any(target_os = "macos", target_os = "linux")))]
fn platform_configured_port(_host: &LoginItemHost) -> Result<Option<u16>> {
    Ok(None)
}

#[cfg(not(any(target_os = "macos", target_os = "linux")))]
fn platform_install(_host: &LoginItemHost, _port: u16) -> Result<()> {
    bail!("llm_gateway_autostart_unsupported")
}

#[cfg(not(any(target_os = "macos", target_os = "linux")))]
fn platform_uninstall(_host: &LoginItemHost) -> Result<()> {
    bail!("llm_gateway_autostart_unsupported")
}

#[cfg(target_os = "macos")]
fn xml_escape(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

#[cfg(target_os = "linux")]
fn shell_escape(value: &str) -> String {
    if value.is_empty() {
        return "''".into();
    }
    if value
        .chars()
        .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '/' | '.' | '_' | '-' | ':'))
    {
        return value.to_owned();
    }
    format!("'{}'", value.replace('\'', "'\\''"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn marker_round_trips() {
        let marker = AutostartMarker {
            enabled: true,
            port: 15722,
            program: "/Applications/LicoUp.app/Contents/MacOS/licoup-cli".into(),
        };
        let restored = AutostartMarker::from_json(&marker.to_json()).unwrap();
        assert!(restored.enabled);
        assert_eq!(restored.port, 15722);
        assert!(restored.program.contains("licoup-cli"));
    }

    #[test]
    fn saved_root_gateway_definitions_recover_the_selected_port() {
        let plist = "<string>--port</string>\n    <string>16400</string>";
        assert_eq!(gateway_port_from_plist(plist), Some(16400));
        assert_eq!(
            gateway_port_from_systemd("ExecStart=/cli llm-gateway service start --port 16400"),
            Some(16400)
        );
        assert_eq!(gateway_port_from_systemd("ExecStart=/cli gateway"), None);
        assert_eq!(gateway_port_from_systemd("ExecStart=/cli --port 0"), None);

        let mut enabled_port = None;
        refresh_gateway_autostart(
            gateway_port_from_systemd("ExecStart=/cli llm-gateway service start --port 16400"),
            |port| {
                enabled_port = Some(port);
                Ok(())
            },
        )
        .unwrap();
        assert_eq!(enabled_port, Some(16400));

        refresh_gateway_autostart(None, |_| {
            panic!("an absent Gateway login item must remain disabled")
        })
        .unwrap();
    }

    // -----------------------------------------------------------------------
    // The Gateway package's lifecycle and its own login item
    // -----------------------------------------------------------------------

    /// The login-item host every case below writes through, restored when the
    /// case ends so a later case never inherits it.
    struct SyntheticHost;

    impl SyntheticHost {
        fn install(host: LoginItemHost) -> Self {
            set_gateway_package_host_override(Some(host));
            Self
        }
    }

    impl Drop for SyntheticHost {
        fn drop(&mut self) {
            set_gateway_package_host_override(None);
        }
    }

    /// One synthetic login-item host over disposable paths inside `root`.
    fn synthetic_host(root: &Path) -> LoginItemHost {
        LoginItemHost::synthetic(
            root.join("login-home"),
            root.join("llm-gateway"),
            root.join("licoup-cli"),
        )
    }

    /// A store root holding one installed version of the Gateway package whose
    /// own manifest declares `entry`, with that entry in the payload.
    fn installed_gateway_store_root(label: &str, version: &str, entry: &str) -> PathBuf {
        use crate::platform::extension_packages::{
            PackageStore, TrustRecord, content_digest, running_client_version,
        };
        use licoup_extension_contracts::wire;
        use std::io::Write;

        let root = std::env::temp_dir().join(format!(
            "licoup-gateway-package-lifecycle-{label}-{}",
            crate::platform::extension_packages::unique_suffix()
        ));
        std::fs::create_dir_all(&root).expect("store root");
        let client = running_client_version().expect("a product version");
        let next_major = client
            .split('.')
            .next()
            .and_then(|major| major.parse::<u64>().ok())
            .map(|major| major + 1)
            .expect("a semantic major version");
        let manifest = json!({
            "schema": wire::MANIFEST,
            "id": GATEWAY_PACKAGE_ID,
            "version": version,
            "displayName": "Synthetic gateway",
            "hostProtocol": { "major": 1, "minimumMinor": 0 },
            "compatibility": { "clientVersions": [format!(">={client}, <{next_major}")] },
            "profiles": [{ "id": "model-gateway", "major": 1, "capabilities": ["model-gateway.v1"] }],
            "runtime": { "mode": "process", "entry": entry },
            "activation": "on-demand",
            "requires": [],
            "optionalRequires": [],
            "permissions": [],
            "contributions": [],
        });
        let mut writer = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
        let options = zip::write::SimpleFileOptions::default();
        for (name, body) in [
            ("manifest.json", manifest.to_string()),
            (entry, "#!/bin/sh\nexit 1\n".to_owned()),
        ] {
            writer.start_file(name, options).expect("start file");
            writer.write_all(body.as_bytes()).expect("write");
        }
        let bytes = writer.finish().expect("finish").into_inner();
        let store = PackageStore::open(&root).expect("store");
        store
            .install_local_import(
                GATEWAY_PACKAGE_ID,
                version,
                TrustRecord::local_approved(content_digest(&bytes), []).expect("trust"),
                &bytes,
            )
            .expect("install the synthetic gateway package");
        root
    }

    #[test]
    fn a_lifecycle_transition_speaks_only_for_the_package_it_was_about() {
        let store_root = installed_gateway_store_root("lifecycle", "0.3.0", "bin/lico-gateway");
        let host = synthetic_host(&store_root);
        let definition = host.definition_path().expect("definition path");
        let _override = SyntheticHost::install(host.clone());

        // A route about another package changes nothing: the login item is not
        // this package's to touch, and no host is resolved for it either.
        assert_eq!(
            apply_gateway_package_lifecycle(
                &store_root,
                "example.other.package",
                GatewayPackageLifecycle::Activated,
                None,
            )
            .map(|outcome| outcome.is_ok()),
            None
        );
        assert!(!definition.exists());

        // An install or an enable registers the item the installed payload
        // starts, on the port this client starts the Gateway on.
        let activated = apply_gateway_package_lifecycle(
            &store_root,
            GATEWAY_PACKAGE_ID,
            GatewayPackageLifecycle::Activated,
            None,
        )
        .expect("the transition is about this package")
        .expect("the login item is registered");
        assert_eq!(activated["enabled"], json!(true));
        assert_eq!(activated["installed"], json!(true));
        assert_eq!(
            activated["port"],
            json!(crate::platform::llm_gateway_service::DEFAULT_PORT)
        );
        assert!(definition.is_file());

        // A disable or an uninstall withdraws it, and reports the port that went
        // with the definition so a version that remains can be restored on it.
        let retired = apply_gateway_package_lifecycle(
            &store_root,
            GATEWAY_PACKAGE_ID,
            GatewayPackageLifecycle::Retired,
            None,
        )
        .expect("the transition is about this package")
        .expect("the withdrawal is truthful without a definition to remove");
        assert_eq!(retired["enabled"], json!(false));
        assert_eq!(
            retired["withdrawnPort"],
            json!(crate::platform::llm_gateway_service::DEFAULT_PORT)
        );
        assert!(!definition.exists(), "the definition itself is gone");

        // An activation keeps the port this client already configured rather
        // than moving the endpoint the user's own switch chose.
        apply_gateway_package_lifecycle(
            &store_root,
            GATEWAY_PACKAGE_ID,
            GatewayPackageLifecycle::Activated,
            Some(16_400),
        )
        .expect("about this package")
        .expect("registered");
        assert!(
            fs::read_to_string(&definition)
                .expect("definition")
                .contains("16400")
        );
        let retired = apply_gateway_package_lifecycle(
            &store_root,
            GATEWAY_PACKAGE_ID,
            GatewayPackageLifecycle::Retired,
            None,
        )
        .expect("about this package")
        .expect("withdrawn");
        assert_eq!(retired["withdrawnPort"], json!(16_400));
        // The route hands that port to the activation that follows the removal,
        // which is how a version that remains installed is restored on the port
        // this client configured rather than moved to the default.
        let restored = apply_gateway_package_lifecycle(
            &store_root,
            GATEWAY_PACKAGE_ID,
            GatewayPackageLifecycle::Activated,
            retired["withdrawnPort"]
                .as_u64()
                .and_then(|port| u16::try_from(port).ok()),
        )
        .expect("about this package")
        .expect("registered");
        assert_eq!(restored["port"], json!(16_400));
    }

    #[test]
    fn an_activation_over_a_store_that_holds_nothing_registers_nothing() {
        let empty = std::env::temp_dir().join(format!(
            "licoup-gateway-package-empty-{}",
            crate::platform::extension_packages::unique_suffix()
        ));
        std::fs::create_dir_all(&empty).expect("store root");
        let host = synthetic_host(&empty);
        let definition = host.definition_path().expect("definition path");
        let _override = SyntheticHost::install(host);

        // The answer a route that removed the last installed version has to
        // leave behind: no login item, and no error that would turn an empty
        // store into a failed removal.
        let status = apply_gateway_package_lifecycle(
            &empty,
            GATEWAY_PACKAGE_ID,
            GatewayPackageLifecycle::Activated,
            None,
        )
        .expect("about this package")
        .expect("an empty store is not a failed route");
        assert_eq!(status["installed"], json!(false));
        assert_eq!(status["enabled"], json!(false));
        assert!(
            !definition.exists(),
            "nothing installed registers no login item"
        );
    }
}

#[cfg(any(target_os = "macos", test))]
fn gateway_port_from_plist(contents: &str) -> Option<u16> {
    let lines: Vec<&str> = contents.lines().collect();
    let argument = lines
        .iter()
        .position(|line| line.trim() == "<string>--port</string>")?;
    lines
        .get(argument + 1)?
        .trim()
        .strip_prefix("<string>")?
        .strip_suffix("</string>")?
        .parse::<u16>()
        .ok()
        .filter(|port| *port != 0)
}

#[cfg(any(target_os = "linux", test))]
fn gateway_port_from_systemd(contents: &str) -> Option<u16> {
    contents
        .lines()
        .find_map(|line| line.strip_prefix("ExecStart="))?
        .split("--port")
        .nth(1)?
        .split_whitespace()
        .next()?
        .parse::<u16>()
        .ok()
        .filter(|port| *port != 0)
}
