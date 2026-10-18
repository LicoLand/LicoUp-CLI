use super::registry::runtime_driver_profile;
use super::{RuntimeAdapter, RuntimeAdapterError};
#[cfg(any(test, feature = "test-support"))]
use sha2::{Digest, Sha256};
use std::fs;
#[cfg(any(test, feature = "test-support"))]
use std::fs::{File, Metadata};
#[cfg(any(test, feature = "test-support"))]
use std::io::Read;
use std::path::Path;

#[cfg(any(test, feature = "test-support"))]
pub fn runtime_artifact_digest(executable: &Path) -> Option<String> {
    let mut file = File::open(executable).ok()?;
    let opened_before = file.metadata().ok()?;
    if !opened_before.is_file() {
        return None;
    }
    let mut hasher = Sha256::new();
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        let read = loop {
            match file.read(&mut buffer) {
                Ok(read) => break read,
                Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
                Err(_) => return None,
            }
        };
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    let opened_after = file.metadata().ok()?;
    let current = File::open(executable).ok()?;
    let current_metadata = current.metadata().ok()?;
    if !same_runtime_artifact(&opened_before, &opened_after)
        || !same_runtime_artifact(&opened_after, &current_metadata)
    {
        return None;
    }
    Some(format!("sha256:{:x}", hasher.finalize()))
}

pub fn runtime_executable(
    port: &licoup_agent_targets::port::AgentTargetPort,
    adapter: RuntimeAdapter,
    requested: &str,
) -> Result<String, RuntimeAdapterError> {
    runtime_executable_with_discovery(
        adapter,
        requested,
        |target| {
            licoup_agent_targets::domain::targets::manual_runtime_executable(target)
                .map_err(|_| RuntimeAdapterError::ExecutableUnavailable)
        },
        licoup_agent_targets::domain::targets::agent_cli_executable,
        |target| licoup_agent_targets::domain::targets::available_runtime_executable(port, target),
    )
}

pub(super) fn runtime_executable_with_discovery(
    adapter: RuntimeAdapter,
    requested: &str,
    discover_manual: impl FnOnce(&str) -> Result<Option<std::path::PathBuf>, RuntimeAdapterError>,
    discover_current: impl FnOnce(&str) -> Option<std::path::PathBuf>,
    discover_cached: impl FnOnce(&str) -> Option<std::path::PathBuf>,
) -> Result<String, RuntimeAdapterError> {
    if runtime_driver_profile(adapter.id()).is_none() {
        return Err(RuntimeAdapterError::RuntimeProfileUnavailable);
    }
    let requested_path = Path::new(requested);
    if !requested_path.is_absolute() {
        // Group Conversation turns intentionally persist only the Agent id,
        // never an executable path. When such a turn carries the adapter's
        // default command, recover the exact executable from the same native
        // discovery authority used by one-to-one chat. This is especially
        // important for product-bundled runtimes such as Codex and Kilo Code,
        // whose official CLIs may live inside desktop or editor packages.
        if requested == adapter.default_binary() {
            if let Some(discovered) = default_runtime_discovery(
                adapter,
                adapter.id(),
                discover_manual,
                discover_current,
                discover_cached,
            )? {
                return discovered
                    .to_str()
                    .map(str::to_string)
                    .ok_or(RuntimeAdapterError::ExecutableUnavailable);
            }
        }
        return Ok(requested.to_string());
    }
    let canonical =
        fs::canonicalize(requested_path).map_err(|_| RuntimeAdapterError::ExecutableUnavailable)?;
    if !canonical.is_file() {
        return Err(RuntimeAdapterError::ExecutableUnavailable);
    }
    canonical
        .to_str()
        .map(str::to_string)
        .ok_or(RuntimeAdapterError::ExecutableUnavailable)
}

fn default_runtime_discovery(
    adapter: RuntimeAdapter,
    target: &str,
    discover_manual: impl FnOnce(&str) -> Result<Option<std::path::PathBuf>, RuntimeAdapterError>,
    discover_current: impl FnOnce(&str) -> Option<std::path::PathBuf>,
    discover_cached: impl FnOnce(&str) -> Option<std::path::PathBuf>,
) -> Result<Option<std::path::PathBuf>, RuntimeAdapterError> {
    if adapter == RuntimeAdapter::Codex {
        let manual = discover_manual(target)?
            .filter(|path| path.is_absolute())
            .or_else(|| discover_current(target));
        Ok(manual.or_else(|| discover_cached(target)))
    } else {
        Ok(discover_cached(target).or_else(|| discover_current(target)))
    }
}

#[cfg(unix)]
#[cfg(any(test, feature = "test-support"))]
pub fn same_runtime_artifact(left: &Metadata, right: &Metadata) -> bool {
    use std::os::unix::fs::MetadataExt;
    left.dev() == right.dev()
        && left.ino() == right.ino()
        && left.len() == right.len()
        && left.mtime() == right.mtime()
        && left.mtime_nsec() == right.mtime_nsec()
}

#[cfg(not(unix))]
#[cfg(any(test, feature = "test-support"))]
pub fn same_runtime_artifact(left: &Metadata, right: &Metadata) -> bool {
    left.len() == right.len()
        && left.modified().ok() == right.modified().ok()
        && left.permissions().readonly() == right.permissions().readonly()
}
