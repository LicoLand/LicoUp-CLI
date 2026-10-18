use super::catalog::normalize_target;
use super::discovery::scan_targets_with_store;
use super::manual::manual_targets_read_only;
use super::target_cache::cached_runtime_executable;
use crate::port::AgentTargetPort;
use licoup_client_state::ClientStateStore;
use serde_json::json;
use std::collections::VecDeque;
use std::path::PathBuf;
use std::sync::{Mutex, OnceLock};

/// Why the saved local binary path for one Agent could not be read.
///
/// The saved path is an explicit user choice, so a failure to read it must stop
/// the fallback chain rather than silently degrade to automatic discovery. The
/// caller that owns the launcher's error vocabulary maps this to its own
/// "executable unavailable" outcome.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ManualRuntimeError {
    /// The portable client-state store could not be opened or read.
    StateUnavailable,
}

const MAX_TARGET_STORE_ROOTS: usize = 4;
static PORTABLE_TARGET_STORES: OnceLock<Mutex<VecDeque<(PathBuf, ClientStateStore)>>> =
    OnceLock::new();

/// Resolve the single local executable advertised by target discovery for a
/// runtime that has a conversation driver. Local agents are client-accessible
/// by default, so parity evidence no longer gates the binding; callers still
/// revalidate immediately before launch, which prevents a remote command from
/// choosing a PATH entry or supplying a local execution path.
pub fn available_runtime_executable(port: &AgentTargetPort, target: &str) -> Option<PathBuf> {
    (port.runtime_driver_profile)(target)?;
    let store = portable_target_store()?;
    if let Some(executable) = cached_runtime_executable(&store, target) {
        return Some(executable);
    }
    // Cache miss: refresh discovery through the same client-state owner and
    // re-read the coherent projection instead of scanning the response.
    scan_targets_with_store(port, &json!({}), &store).ok()?;
    cached_runtime_executable(&store, target)
}

/// A saved local binary path is an explicit user choice. It is kept separate
/// from the automatic discovery-cache routes so a stale automatic route cannot
/// be mistaken for a manual override.
pub fn manual_runtime_executable(target: &str) -> Result<Option<PathBuf>, ManualRuntimeError> {
    let store =
        ClientStateStore::portable_read_only().map_err(|_| ManualRuntimeError::StateUnavailable)?;
    manual_runtime_executable_from_store(&store, target)
}

/// The saved local binary path as read from a caller-supplied store. The
/// launcher resolves it against the same store instance it validates the rest
/// of the launch against, and the crate's own tests pin an explicit root.
pub fn manual_runtime_executable_from_store(
    store: &ClientStateStore,
    target: &str,
) -> Result<Option<PathBuf>, ManualRuntimeError> {
    let normalized = normalize_target(target);
    let manuals =
        manual_targets_read_only(store).map_err(|_| ManualRuntimeError::StateUnavailable)?;
    Ok(manuals
        .into_iter()
        .find(|manual| manual.target == normalized && manual.location == "local")
        .and_then(|manual| manual.binary_path)
        .filter(|path| path.is_absolute()))
}

fn portable_target_store() -> Option<ClientStateStore> {
    let root = licoup_foundation::platform::paths::portable_data_dir().ok()?;
    let stores = PORTABLE_TARGET_STORES.get_or_init(|| Mutex::new(VecDeque::new()));
    let mut stores = stores
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    if let Some(position) = stores.iter().position(|(candidate, _)| candidate == &root) {
        let entry = stores
            .remove(position)
            .expect("target store position exists");
        let store = entry.1.clone();
        stores.push_back(entry);
        return Some(store);
    }
    let store = ClientStateStore::portable().ok()?;
    if stores.len() == MAX_TARGET_STORE_ROOTS {
        stores.pop_front();
    }
    stores.push_back((root, store.clone()));
    Some(store)
}
