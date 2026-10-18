//! Telegram channel process lifecycle: the `org.licoland.channel.telegram`
//! package's declared entry, started and stopped by this host.
//!
//! The channel is an optional package process, not a thread inside the Private
//! Gateway. This module resolves the installed package's own `runtime.entry`,
//! spawns it detached under the channel's state directory and keeps a pid
//! record so `service_stop` signals the process it started — never a stranger.
//! A process is only started while both the package is installed and a bot
//! token is configured; every caller treats an absent package as "not
//! configured", not as a failure.

use anyhow::{Result, anyhow};
use licoup_foundation::platform::file_security::{atomic_write_private_text, ensure_private_dir};
use licoup_foundation::platform::paths;
use licoup_public_gateway::telegram;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::fs::{self, OpenOptions};
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};

use super::extension_packages::PackageStore;

/// The package that ships the channel process.
pub const TELEGRAM_CHANNEL_PACKAGE_ID: &str = "org.licoland.channel.telegram";

const STATE_DIRECTORY: &str = "telegram-gateway";

pub struct ChannelPaths {
    pid: PathBuf,
    log: PathBuf,
}

impl ChannelPaths {
    fn resolve() -> Result<Self> {
        let root = paths::portable_data_dir()?.join(STATE_DIRECTORY);
        ensure_private_dir(&root)?;
        Ok(Self {
            pid: root.join("channel.pid"),
            log: root.join("channel.log"),
        })
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ChannelPidRecord {
    pid: u32,
    program: PathBuf,
    started_at: u64,
}

/// The installed package's declared channel program, or none when the package
/// is absent, disabled, or its entry is missing from the installed payload.
fn installed_program() -> Result<Option<PathBuf>> {
    let store_root = super::llm_gateway_autostart::package_store_root(&paths::portable_data_dir()?);
    let store = PackageStore::open(&store_root)
        .map_err(|_| anyhow!("telegram_channel_package_store_unavailable"))?;
    let installed = store
        .installed()
        .map_err(|_| anyhow!("telegram_channel_package_store_unavailable"))?;
    let Some(version) = installed
        .into_iter()
        .find(|package| package.package_id == TELEGRAM_CHANNEL_PACKAGE_ID)
        .map(|package| package.version)
    else {
        return Ok(None);
    };
    let manifest = store
        .installed_manifest(TELEGRAM_CHANNEL_PACKAGE_ID, &version)
        .map_err(|_| anyhow!("telegram_channel_package_manifest_unavailable"))?;
    let licoup_extension_contracts::manifest::Runtime::Process { entry, .. } = manifest.runtime
    else {
        return Err(anyhow!("telegram_channel_package_entry_missing"));
    };
    if entry.is_empty() {
        return Err(anyhow!("telegram_channel_package_entry_missing"));
    }
    let program = store
        .installed_path(TELEGRAM_CHANNEL_PACKAGE_ID, &version)
        .join(entry);
    if !fs::symlink_metadata(&program).is_ok_and(|m| m.file_type().is_file()) {
        return Err(anyhow!("telegram_channel_package_entry_missing"));
    }
    Ok(Some(program))
}

fn pid_alive(pid: u32) -> bool {
    #[cfg(unix)]
    {
        unsafe { libc::kill(pid as i32, 0) == 0 }
    }
    #[cfg(not(unix))]
    {
        let _ = pid;
        false
    }
}

fn unix_seconds() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_secs())
        .unwrap_or(0)
}

fn read_pid_record(path: &std::path::Path) -> Result<Option<ChannelPidRecord>> {
    let bytes = match fs::read(path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(_) => return Err(anyhow!("telegram_channel_pid_record_unavailable")),
    };
    serde_json::from_slice(&bytes)
        .map(Some)
        .map_err(|_| anyhow!("telegram_channel_pid_record_unavailable"))
}

fn write_pid_record(path: &std::path::Path, record: &ChannelPidRecord) -> Result<()> {
    let text = serde_json::to_string(record)
        .map_err(|_| anyhow!("telegram_channel_pid_record_unavailable"))?;
    atomic_write_private_text(path, &text)
        .map_err(|_| anyhow!("telegram_channel_pid_record_unavailable"))
}

fn clear_pid_record(path: &std::path::Path) -> Result<()> {
    match fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(_) => Err(anyhow!("telegram_channel_pid_record_unavailable")),
    }
}

pub fn spawn_channel(program: &std::path::Path, paths: &ChannelPaths) -> Result<Child> {
    let log = OpenOptions::new()
        .create(true)
        .append(true)
        .open(&paths.log)
        .map_err(|_| anyhow!("telegram_channel_start_failed"))?;
    let log_errors = log
        .try_clone()
        .map_err(|_| anyhow!("telegram_channel_start_failed"))?;
    let mut command = Command::new(program);
    command
        .stdin(Stdio::null())
        .stdout(Stdio::from(log))
        .stderr(Stdio::from(log_errors));
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        unsafe {
            command.pre_exec(|| {
                if libc::setsid() == -1 {
                    return Err(std::io::Error::last_os_error());
                }
                Ok(())
            });
        }
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt as WindowsCommandExt;
        command.creation_flags(0x0000_0008 | 0x0000_0200 | 0x0800_0000);
    }
    command
        .spawn()
        .map_err(|_| anyhow!("telegram_channel_start_failed"))
}

fn running_record(paths: &ChannelPaths) -> Result<Option<ChannelPidRecord>> {
    match read_pid_record(&paths.pid)? {
        Some(record) if pid_alive(record.pid) => Ok(Some(record)),
        Some(_) => {
            clear_pid_record(&paths.pid)?;
            Ok(None)
        }
        None => Ok(None),
    }
}

/// Start the channel process. Refuses when the package or its declared entry
/// is absent; a missing bot token is reported as `notConfigured` rather than a
/// spawn, matching the channel's own contract.
pub fn service_start() -> Result<Value> {
    let paths = ChannelPaths::resolve()?;
    if let Some(record) = running_record(&paths)? {
        return Ok(json!({
            "ok": true,
            "schemaVersion": "licoup.telegram-channel-service.v1",
            "state": "running",
            "pid": record.pid,
            "program": record.program,
        }));
    }
    let Some(program) = installed_program()? else {
        return Ok(json!({
            "ok": true,
            "schemaVersion": "licoup.telegram-channel-service.v1",
            "state": "absent",
            "packageId": TELEGRAM_CHANNEL_PACKAGE_ID,
        }));
    };
    if !telegram::token_configured().unwrap_or(false) {
        return Ok(json!({
            "ok": true,
            "schemaVersion": "licoup.telegram-channel-service.v1",
            "state": "notConfigured",
            "packageId": TELEGRAM_CHANNEL_PACKAGE_ID,
        }));
    }
    let child = spawn_channel(&program, &paths)?;
    let record = ChannelPidRecord {
        pid: child.id(),
        program: program.clone(),
        started_at: unix_seconds(),
    };
    write_pid_record(&paths.pid, &record)?;
    Ok(json!({
        "ok": true,
        "schemaVersion": "licoup.telegram-channel-service.v1",
        "state": "running",
        "pid": record.pid,
        "program": program,
    }))
}

/// Start the channel only when it can run: package installed, token
/// configured, no live process. Every absence is a quiet no-op so callers such
/// as `credentials set` never fail over a channel that is not set up yet.
pub fn ensure_started() -> Result<()> {
    let paths = ChannelPaths::resolve()?;
    if running_record(&paths)?.is_some() {
        return Ok(());
    }
    if installed_program()?.is_none() || !telegram::token_configured().unwrap_or(false) {
        return Ok(());
    }
    service_start().map(|_| ())
}

/// Stop the recorded channel process, if any, and clear the ready marker.
pub fn service_stop() -> Result<Value> {
    let paths = ChannelPaths::resolve()?;
    let record = running_record(&paths)?;
    let mut stopped = false;
    if let Some(record) = record {
        #[cfg(unix)]
        {
            unsafe {
                let _ = libc::kill(record.pid as i32, libc::SIGTERM);
            }
            stopped = true;
        }
        #[cfg(not(unix))]
        {
            let _ = Command::new("taskkill")
                .args(["/PID", &record.pid.to_string(), "/T", "/F"])
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status();
            stopped = true;
        }
    }
    clear_pid_record(&paths.pid)?;
    let _ = telegram::clear_ready();
    Ok(json!({
        "ok": true,
        "schemaVersion": "licoup.telegram-channel-service.v1",
        "state": if stopped { "stopped" } else { "absent" },
    }))
}

/// Report the channel process state without spawning anything.
pub fn service_status() -> Result<Value> {
    let paths = ChannelPaths::resolve()?;
    let record = running_record(&paths)?;
    Ok(json!({
        "ok": true,
        "schemaVersion": "licoup.telegram-channel-service.v1",
        "state": if record.is_some() { "running" } else { "stopped" },
        "pid": record.as_ref().map(|record| record.pid),
        "packageId": TELEGRAM_CHANNEL_PACKAGE_ID,
        "tokenConfigured": telegram::token_configured().unwrap_or(false),
    }))
}
